use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use crate::error::Error;

/// Conservative process-safety default; callers may raise it up to the protocol limit.
pub const DEFAULT_MAX_IMAGE_BYTES: usize = 256 * 1024 * 1024;
/// The protocol carries image byte counts in a signed 32-bit LONG/int.
pub const PROTOCOL_MAX_IMAGE_BYTES: usize = i32::MAX as usize;
/// SerialEM stores framed lengths in signed 32-bit integers.
pub const PROTOCOL_MAX_FRAME_BYTES: usize = crate::protocol::PROTOCOL_MAX_FRAME_BYTES;

#[derive(Debug, Clone)]
pub struct TransportConfig {
    pub connect_timeout: Duration,
    pub read_timeout: Option<Duration>,
    pub write_timeout: Option<Duration>,
    /// Optional absolute deadline for framed command exchanges.
    pub command_timeout: Option<Duration>,
    /// Optional absolute deadline for the raw bytes of one image write.
    pub image_write_timeout: Option<Duration>,
    pub max_frame_bytes: usize,
    /// Safety limit for a single image transfer allocation.
    pub max_image_bytes: usize,
    /// Only used by the explicit idempotent retry API.
    pub retry_window: Duration,
}

impl TransportConfig {
    pub fn validate(&self) -> Result<(), Error> {
        if self.connect_timeout.is_zero() {
            return Err(Error::InvalidArgument(
                "connect_timeout must be greater than zero".to_string(),
            ));
        }
        if self.max_frame_bytes < 4 || self.max_frame_bytes > PROTOCOL_MAX_FRAME_BYTES {
            return Err(Error::InvalidArgument(format!(
                "max_frame_bytes must be between 4 and {PROTOCOL_MAX_FRAME_BYTES}"
            )));
        }
        if self.max_image_bytes > PROTOCOL_MAX_IMAGE_BYTES {
            return Err(Error::InvalidArgument(format!(
                "max_image_bytes cannot exceed protocol limit {PROTOCOL_MAX_IMAGE_BYTES}"
            )));
        }
        if self
            .command_timeout
            .is_some_and(|timeout| timeout.is_zero())
        {
            return Err(Error::InvalidArgument(
                "command_timeout must be greater than zero when configured".to_string(),
            ));
        }
        if self
            .image_write_timeout
            .is_some_and(|timeout| timeout.is_zero())
        {
            return Err(Error::InvalidArgument(
                "image_write_timeout must be greater than zero when configured".to_string(),
            ));
        }
        Ok(())
    }
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            read_timeout: None,
            write_timeout: None,
            command_timeout: None,
            image_write_timeout: None,
            max_frame_bytes: 64 * 1024 * 1024,
            max_image_bytes: DEFAULT_MAX_IMAGE_BYTES,
            retry_window: Duration::from_millis(200),
        }
    }
}

#[derive(Debug)]
pub struct TcpTransport {
    address: SocketAddr,
    stream: TcpStream,
    pending: Vec<u8>,
    config: TransportConfig,
    needs_reconnect: bool,
    closed: bool,
}

impl TcpTransport {
    pub fn connect(address: SocketAddr) -> Result<Self, Error> {
        Self::connect_with_config(address, TransportConfig::default())
    }

    pub fn connect_with_config(
        address: SocketAddr,
        config: TransportConfig,
    ) -> Result<Self, Error> {
        config.validate()?;
        let stream = TcpStream::connect_timeout(&address, config.connect_timeout)?;
        stream.set_read_timeout(config.read_timeout)?;
        stream.set_write_timeout(config.write_timeout)?;
        Ok(Self {
            address,
            stream,
            pending: Vec::new(),
            config,
            needs_reconnect: false,
            closed: false,
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Advanced framed transport API. Normal clients should use `SerialEmClient`
    /// so external-control handshakes and raw-image sequencing cannot be bypassed.
    pub fn send_frame(&mut self, frame: &[u8]) -> Result<Vec<u8>, Error> {
        self.send_frame_with_retry(frame, false)
    }

    /// Retry exactly once only when the caller has established that the request is idempotent.
    /// This is an advanced API and does not perform SerialEM readiness handshakes.
    /// A lost response is otherwise ambiguous: SerialEM may already have performed the action.
    pub fn send_frame_idempotent(&mut self, frame: &[u8]) -> Result<Vec<u8>, Error> {
        self.send_frame_with_retry(frame, true)
    }

    fn send_frame_with_retry(&mut self, frame: &[u8], allow_retry: bool) -> Result<Vec<u8>, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        self.config.validate()?;
        self.validate_outgoing_frame(frame)?;
        let command_deadline = self
            .config
            .command_timeout
            .map(|timeout| {
                Instant::now().checked_add(timeout).ok_or_else(|| {
                    Error::InvalidArgument("command_timeout cannot be represented".to_string())
                })
            })
            .transpose()?;
        let previous_read_timeout = self.config.read_timeout;
        let previous_write_timeout = self.config.write_timeout;
        let result = self.send_frame_loop(frame, allow_retry, command_deadline);
        let restore_read = self.stream.set_read_timeout(previous_read_timeout).err();
        let restore_write = self.stream.set_write_timeout(previous_write_timeout).err();
        if let Some(source) = restore_read.or(restore_write) {
            self.invalidate();
            return Err(Error::TransportStateRestore {
                source,
                original: result.err().map(Box::new),
            });
        }
        result
    }

    fn send_frame_loop(
        &mut self,
        frame: &[u8],
        allow_retry: bool,
        command_deadline: Option<Instant>,
    ) -> Result<Vec<u8>, Error> {
        let retry_deadline = if allow_retry && !self.config.retry_window.is_zero() {
            Instant::now()
                .checked_add(self.config.retry_window)
                .ok_or_else(|| {
                    Error::InvalidArgument("retry_window cannot be represented".to_string())
                })
                .map(Some)?
        } else {
            None
        };
        let mut attempt = 0usize;
        let mut retry_error = None;
        loop {
            if let Some(deadline) = command_deadline {
                ensure_before_deadline(deadline)?;
            }
            if attempt > 0
                && let Some(deadline) = retry_deadline
            {
                ensure_before_deadline(deadline).map_err(|_| {
                    retry_error
                        .take()
                        .expect("retry error must exist before a second attempt")
                })?;
            }
            if self.needs_reconnect {
                let reconnect_deadline = if attempt > 0 {
                    min_deadline(command_deadline, retry_deadline)
                } else {
                    command_deadline
                };
                self.reconnect_inner(reconnect_deadline)?;
                if attempt > 0
                    && let Some(deadline) = retry_deadline
                {
                    ensure_before_deadline(deadline).map_err(|_| {
                        retry_error
                            .take()
                            .expect("retry error must exist before a second attempt")
                    })?;
                }
            }
            self.pending.clear();
            let result = self
                .write_all_with_deadline(frame, command_deadline)
                .and_then(|_| self.read_frame_with_deadline(command_deadline));
            match result {
                Ok(response) => return Ok(response),
                Err(error) if is_transport_failure(&error) => {
                    self.invalidate();
                    if allow_retry
                        && retry_deadline.is_some()
                        && attempt == 0
                        && is_reconnectable(&error)
                    {
                        if retry_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                            return Err(error);
                        }
                        retry_error = Some(error);
                        attempt = 1;
                        continue;
                    }
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        }
    }

    pub(crate) fn write_frame(&mut self, frame: &[u8]) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        self.validate_outgoing_frame(frame)?;
        self.write_all(frame)
    }

    pub(crate) fn write_all(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        match self.stream.write_all(bytes) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.invalidate();
                Err(error.into())
            }
        }
    }

    pub(crate) fn write_raw_with_deadline(
        &mut self,
        bytes: &[u8],
        deadline: Option<Instant>,
    ) -> Result<(), Error> {
        self.write_all_with_deadline(bytes, deadline)
    }

    fn write_all_with_deadline(
        &mut self,
        bytes: &[u8],
        deadline: Option<Instant>,
    ) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        write_all_with_deadline_to(&mut self.stream, bytes, deadline, self.config.write_timeout)
    }

    #[allow(dead_code)]
    pub(crate) fn read_frame(&mut self) -> Result<Vec<u8>, Error> {
        self.read_frame_with_deadline(None)
    }

    fn read_frame_with_deadline(&mut self, deadline: Option<Instant>) -> Result<Vec<u8>, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        let result = self.read_frame_inner(deadline);
        if result.is_err() {
            self.invalidate();
        }
        result
    }

    #[allow(dead_code)]
    pub(crate) fn read_exact_bytes(&mut self, length: usize) -> Result<Vec<u8>, Error> {
        let mut output = Vec::new();
        self.read_exact_into(&mut output, length)?;
        Ok(output)
    }

    pub(crate) fn read_exact_into(
        &mut self,
        destination: &mut Vec<u8>,
        length: usize,
    ) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        let original_len = destination.len();
        let result = self.read_exact_into_inner(destination, length);
        if result.is_err() {
            destination.truncate(original_len);
            self.invalidate();
        }
        result
    }

    pub fn pending_bytes(&self) -> usize {
        self.pending.len()
    }

    pub fn max_image_bytes(&self) -> usize {
        self.config.max_image_bytes
    }

    pub fn max_frame_bytes(&self) -> usize {
        self.config.max_frame_bytes
    }

    pub fn needs_reconnect(&self) -> bool {
        self.needs_reconnect && !self.closed
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    pub fn invalidate(&mut self) {
        if self.closed {
            return;
        }
        let _ = self.stream.shutdown(Shutdown::Both);
        self.pending.clear();
        self.needs_reconnect = true;
    }

    pub fn read_timeout(&self) -> Option<Duration> {
        self.config.read_timeout
    }

    pub fn write_timeout(&self) -> Option<Duration> {
        self.config.write_timeout
    }

    #[cfg(test)]
    pub(crate) fn socket_write_timeout(&self) -> io::Result<Option<Duration>> {
        self.stream.write_timeout()
    }

    pub fn command_timeout(&self) -> Option<Duration> {
        self.config.command_timeout
    }

    pub fn image_write_timeout(&self) -> Option<Duration> {
        self.config.image_write_timeout
    }

    pub fn set_image_write_timeout(&mut self, timeout: Option<Duration>) -> Result<(), Error> {
        let mut candidate = self.config.clone();
        candidate.image_write_timeout = timeout;
        candidate.validate()?;
        self.config.image_write_timeout = timeout;
        Ok(())
    }

    pub fn set_command_timeout(&mut self, timeout: Option<Duration>) -> Result<(), Error> {
        let mut candidate = self.config.clone();
        candidate.command_timeout = timeout;
        candidate.validate()?;
        self.config.command_timeout = timeout;
        Ok(())
    }

    pub fn set_read_timeout(&mut self, timeout: Option<Duration>) -> Result<(), Error> {
        self.stream.set_read_timeout(timeout)?;
        self.config.read_timeout = timeout;
        Ok(())
    }

    pub fn set_write_timeout(&mut self, timeout: Option<Duration>) -> Result<(), Error> {
        self.stream.set_write_timeout(timeout)?;
        self.config.write_timeout = timeout;
        Ok(())
    }

    /// Permanently closes this transport. Subsequent sends return `Error::Closed`.
    pub fn close(&mut self) -> Result<(), Error> {
        let result = self.stream.shutdown(Shutdown::Both).map_err(Error::from);
        self.closed = true;
        self.needs_reconnect = false;
        self.pending.clear();
        result
    }

    /// Compatibility alias for `close`; use `invalidate` for recoverable failures.
    #[deprecated(note = "use close for permanent shutdown or invalidate for recoverable failures")]
    pub fn shutdown(&mut self) -> Result<(), Error> {
        self.close()
    }

    /// Explicitly opens a new socket, including after `close`.
    pub fn reconnect(&mut self) -> Result<(), Error> {
        self.reconnect_inner(None)
    }

    fn reconnect_inner(&mut self, deadline: Option<Instant>) -> Result<(), Error> {
        self.reconnect_with_connector(deadline, |address, timeout| {
            TcpStream::connect_timeout(&address, timeout)
        })
    }

    fn reconnect_with_connector<F>(
        &mut self,
        deadline: Option<Instant>,
        connector: F,
    ) -> Result<(), Error>
    where
        F: FnOnce(SocketAddr, Duration) -> io::Result<TcpStream>,
    {
        let connect_timeout = match deadline {
            Some(deadline) => effective_timeout(Some(self.config.connect_timeout), Some(deadline))?,
            None => self.config.connect_timeout,
        };
        let _ = self.stream.shutdown(Shutdown::Both);
        let stream = connector(self.address, connect_timeout)?;
        stream.set_read_timeout(self.config.read_timeout)?;
        stream.set_write_timeout(self.config.write_timeout)?;
        self.stream = stream;
        self.pending.clear();
        self.needs_reconnect = false;
        self.closed = false;
        Ok(())
    }

    fn validate_outgoing_frame(&self, frame: &[u8]) -> Result<(), Error> {
        if frame.len() < 4 {
            return Err(Error::InvalidFrame(
                "outgoing frame is shorter than its length header".to_string(),
            ));
        }
        let declared_len = u32::from_le_bytes(frame[..4].try_into().unwrap()) as usize;
        if declared_len != frame.len() {
            return Err(Error::InvalidFrame(format!(
                "outgoing frame declares {declared_len} bytes but contains {}",
                frame.len()
            )));
        }
        if declared_len > self.config.max_frame_bytes {
            return Err(Error::ResourceLimit(format!(
                "outgoing frame is {declared_len} bytes, configured maximum is {}",
                self.config.max_frame_bytes
            )));
        }
        Ok(())
    }

    fn read_frame_inner(&mut self, deadline: Option<Instant>) -> Result<Vec<u8>, Error> {
        self.ensure_pending(4, deadline)?;
        let declared_len = u32::from_le_bytes(self.pending[0..4].try_into().unwrap()) as usize;
        if declared_len < 4 {
            return Err(Error::InvalidFrame(format!(
                "frame length {declared_len} is smaller than its header"
            )));
        }
        if declared_len > self.config.max_frame_bytes {
            return Err(Error::InvalidFrame(format!(
                "frame length {declared_len} exceeds configured limit {}",
                self.config.max_frame_bytes
            )));
        }
        self.ensure_pending(declared_len, deadline)?;
        Ok(self.pending.drain(..declared_len).collect())
    }

    fn read_exact_into_inner(
        &mut self,
        destination: &mut Vec<u8>,
        length: usize,
    ) -> Result<(), Error> {
        let start = destination.len();
        let end = start
            .checked_add(length)
            .ok_or_else(|| Error::ResourceLimit("raw-transfer length overflow".to_string()))?;
        destination.try_reserve(length).map_err(|error| {
            Error::ResourceLimit(format!(
                "cannot reserve {length} raw-transfer bytes: {error:?}"
            ))
        })?;
        destination.resize(end, 0);
        let mut copied = 0usize;
        if !self.pending.is_empty() {
            let from_pending = length.min(self.pending.len());
            destination[start..start + from_pending].copy_from_slice(&self.pending[..from_pending]);
            self.pending.drain(..from_pending);
            copied = from_pending;
        }

        while copied < length {
            let count = self
                .stream
                .read(&mut destination[start + copied..start + length])?;
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "SerialEM socket closed during raw transfer",
                )
                .into());
            }
            copied += count;
        }
        Ok(())
    }

    fn ensure_pending(&mut self, length: usize, deadline: Option<Instant>) -> Result<(), Error> {
        let needed = length.saturating_sub(self.pending.len());
        self.pending.try_reserve(needed).map_err(|error| {
            Error::ResourceLimit(format!("cannot reserve {needed} frame bytes: {error:?}"))
        })?;
        while self.pending.len() < length {
            if let Some(deadline) = deadline {
                let timeout = effective_timeout(self.config.read_timeout, Some(deadline))?;
                self.stream.set_read_timeout(Some(timeout))?;
            }
            let mut buffer = [0u8; 64 * 1024];
            let count = self.stream.read(&mut buffer)?;
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "SerialEM socket closed before the complete frame arrived",
                )
                .into());
            }
            self.pending.extend_from_slice(&buffer[..count]);
        }
        Ok(())
    }
}

trait RawWriter {
    fn set_write_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()>;
    fn write_bytes(&mut self, bytes: &[u8]) -> io::Result<usize>;
}

impl RawWriter for TcpStream {
    fn set_write_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        TcpStream::set_write_timeout(self, timeout)
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write(bytes)
    }
}

fn write_all_with_deadline_to<W: RawWriter>(
    writer: &mut W,
    bytes: &[u8],
    deadline: Option<Instant>,
    base_timeout: Option<Duration>,
) -> Result<(), Error> {
    let mut offset = 0usize;
    while offset < bytes.len() {
        if let Some(deadline) = deadline {
            let timeout = effective_timeout(base_timeout, Some(deadline))?;
            writer.set_write_timeout(Some(timeout))?;
        }
        let count = writer.write_bytes(&bytes[offset..])?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "SerialEM socket wrote zero bytes",
            )
            .into());
        }
        offset += count;
    }
    Ok(())
}

fn min_deadline(first: Option<Instant>, second: Option<Instant>) -> Option<Instant> {
    match (first, second) {
        (Some(first), Some(second)) => Some(first.min(second)),
        (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
        (None, None) => None,
    }
}

fn ensure_before_deadline(deadline: Instant) -> Result<(), Error> {
    if Instant::now() >= deadline {
        Err(Error::Io(io::Error::new(
            io::ErrorKind::TimedOut,
            "SerialEM command deadline expired",
        )))
    } else {
        Ok(())
    }
}

fn effective_timeout(base: Option<Duration>, deadline: Option<Instant>) -> Result<Duration, Error> {
    let Some(deadline) = deadline else {
        return base.ok_or_else(|| {
            Error::InvalidArgument(
                "an effective timeout requires a deadline or base timeout".to_string(),
            )
        });
    };
    let now = Instant::now();
    if now >= deadline {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::TimedOut,
            "SerialEM command deadline expired",
        )));
    }
    let remaining = deadline.duration_since(now);
    Ok(base.map_or(remaining, |timeout| timeout.min(remaining)))
}

fn is_transport_failure(error: &Error) -> bool {
    error.is_transport_failure()
}

fn is_reconnectable(error: &Error) -> bool {
    matches!(
        error,
        Error::Io(io_error)
            if matches!(
                io_error.kind(),
                io::ErrorKind::BrokenPipe
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::NotConnected
                    | io::ErrorKind::UnexpectedEof
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn rejects_invalid_transport_configuration() {
        let invalid_frame = TransportConfig {
            max_frame_bytes: 3,
            ..TransportConfig::default()
        };
        assert!(matches!(
            invalid_frame.validate(),
            Err(Error::InvalidArgument(message)) if message.contains("max_frame_bytes")
        ));

        let oversized_frame = TransportConfig {
            max_frame_bytes: PROTOCOL_MAX_FRAME_BYTES + 1,
            ..TransportConfig::default()
        };
        assert!(oversized_frame.validate().is_err());

        let invalid_command_timeout = TransportConfig {
            command_timeout: Some(Duration::ZERO),
            ..TransportConfig::default()
        };
        assert!(invalid_command_timeout.validate().is_err());

        let invalid_image_write_timeout = TransportConfig {
            image_write_timeout: Some(Duration::ZERO),
            ..TransportConfig::default()
        };
        assert!(invalid_image_write_timeout.validate().is_err());

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut transport = TcpTransport::connect(listener.local_addr().unwrap()).unwrap();
        assert!(
            transport
                .set_image_write_timeout(Some(Duration::from_secs(1)))
                .is_ok()
        );
        assert_eq!(
            transport.image_write_timeout(),
            Some(Duration::from_secs(1))
        );

        let invalid_image_limit = TransportConfig {
            max_image_bytes: PROTOCOL_MAX_IMAGE_BYTES + 1,
            ..TransportConfig::default()
        };
        assert!(invalid_image_limit.validate().is_err());
    }

    #[test]
    fn retries_one_early_disconnect_within_the_pythonmodule_window() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let mut request = [0u8; 1];
            first.read_exact(&mut request).unwrap();
            drop(first);

            let (mut second, _) = listener.accept().unwrap();
            second.read_exact(&mut request).unwrap();
            second.write_all(&[8u8, 0, 0, 0, 7, 0, 0, 0]).unwrap();
        });

        let mut transport = TcpTransport::connect(address).unwrap();
        let response = transport
            .send_frame_idempotent(&[8, 0, 0, 0, 7, 0, 0, 0])
            .unwrap();
        assert_eq!(response, vec![8, 0, 0, 0, 7, 0, 0, 0]);
        worker.join().unwrap();
    }

    #[test]
    fn marks_a_dropped_response_connection_for_reconnect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1];
            stream.read_exact(&mut request).unwrap();
            stream.write_all(&[24, 0]).unwrap();
            drop(stream);
            thread::sleep(Duration::from_millis(50));
            listener.set_nonblocking(true).unwrap();
            assert!(
                matches!(listener.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
            );
        });

        // The default policy must not repeat a command after a response is lost.
        let mut transport = TcpTransport::connect(address).unwrap();
        let error = transport.send_frame(&[8, 0, 0, 0, 7, 0, 0, 0]).unwrap_err();
        assert!(matches!(error, Error::Io(_) | Error::InvalidFrame(_)));
        assert!(transport.needs_reconnect());
        worker.join().unwrap();
    }

    #[test]
    fn next_send_reconnects_after_a_response_loss() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let mut request = [0u8; 1];
            first.read_exact(&mut request).unwrap();
            drop(first);

            let (mut second, _) = listener.accept().unwrap();
            second.read_exact(&mut request).unwrap();
            second.write_all(&[8u8, 0, 0, 0, 7, 0, 0, 0]).unwrap();
        });

        let mut transport = TcpTransport::connect(address).unwrap();
        assert!(transport.send_frame(&[8, 0, 0, 0, 7, 0, 0, 0]).is_err());
        let response = transport.send_frame(&[8, 0, 0, 0, 8, 0, 0, 0]).unwrap();
        assert_eq!(response, vec![8, 0, 0, 0, 7, 0, 0, 0]);
        worker.join().unwrap();
    }

    #[test]
    fn close_is_permanent_until_explicit_reconnect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (first, _) = listener.accept().unwrap();
            drop(first);
            let (mut second, _) = listener.accept().unwrap();
            let mut request = [0u8; 8];
            second.read_exact(&mut request).unwrap();
            second.write_all(&[8u8, 0, 0, 0, 7, 0, 0, 0]).unwrap();
        });

        let mut transport = TcpTransport::connect(address).unwrap();
        transport.close().unwrap();
        assert!(transport.is_closed());
        assert!(matches!(
            transport.send_frame(&[8, 0, 0, 0, 7, 0, 0, 0]),
            Err(Error::Closed)
        ));
        transport.reconnect().unwrap();
        assert!(!transport.is_closed());
        let response = transport.send_frame(&[8, 0, 0, 0, 7, 0, 0, 0]).unwrap();
        assert_eq!(response, vec![8, 0, 0, 0, 7, 0, 0, 0]);
        worker.join().unwrap();
    }

    #[test]
    fn idempotent_request_can_exceed_retry_window_when_first_attempt_is_healthy() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 8];
            stream.read_exact(&mut request).unwrap();
            thread::sleep(Duration::from_millis(250));
            stream.write_all(&[8u8, 0, 0, 0, 7, 0, 0, 0]).unwrap();
        });

        let mut transport = TcpTransport::connect(address).unwrap();
        let response = transport
            .send_frame_idempotent(&[8, 0, 0, 0, 7, 0, 0, 0])
            .unwrap();
        assert_eq!(response, vec![8, 0, 0, 0, 7, 0, 0, 0]);
        worker.join().unwrap();
    }

    #[test]
    fn command_timeout_bounds_a_framed_exchange() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 8];
            stream.read_exact(&mut request).unwrap();
            thread::sleep(Duration::from_millis(100));
        });

        let config = TransportConfig {
            command_timeout: Some(Duration::from_millis(20)),
            ..TransportConfig::default()
        };
        let mut transport = TcpTransport::connect_with_config(address, config).unwrap();
        let error = transport.send_frame(&[8, 0, 0, 0, 7, 0, 0, 0]).unwrap_err();
        assert!(matches!(
            error,
            Error::Io(ref io_error)
                if matches!(io_error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
        ));
        assert!(transport.needs_reconnect());
        worker.join().unwrap();
    }

    #[test]
    fn command_timeout_is_an_absolute_deadline_across_drip_reads() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 8];
            stream.read_exact(&mut request).unwrap();
            let response = [8u8, 0, 0, 0, 7, 0, 0, 0];
            for byte in response {
                if stream.write_all(&[byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        });

        let config = TransportConfig {
            command_timeout: Some(Duration::from_millis(35)),
            ..TransportConfig::default()
        };
        let mut transport = TcpTransport::connect_with_config(address, config).unwrap();
        let started = Instant::now();
        let error = transport.send_frame(&[8, 0, 0, 0, 7, 0, 0, 0]).unwrap_err();
        assert!(matches!(
            error,
            Error::Io(ref io_error)
                if matches!(io_error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
        ));
        assert!(started.elapsed() < Duration::from_millis(90));
        worker.join().unwrap();
    }

    #[test]
    fn retry_reconnect_deadline_is_never_later_than_retry_eligibility_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut transport = TcpTransport::connect(listener.local_addr().unwrap()).unwrap();
        let retry_deadline = Instant::now() + Duration::from_millis(20);
        let started = Instant::now();
        let error = transport
            .reconnect_with_connector(Some(retry_deadline), |_address, timeout| {
                assert!(timeout <= Duration::from_millis(20));
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "simulated reconnect timeout",
                ))
            })
            .unwrap_err();
        assert!(error.is_timeout());
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn idempotent_retry_does_not_reconnect_after_retry_window_expires_before_response_loss() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let mut request = [0u8; 8];
            first.read_exact(&mut request).unwrap();
            thread::sleep(Duration::from_millis(60));
            drop(first);
            listener.set_nonblocking(true).unwrap();
            let started = Instant::now();
            while started.elapsed() < Duration::from_millis(80) {
                if let Ok((_, _)) = listener.accept() {
                    panic!("retry connected after retry deadline expired");
                }
                thread::sleep(Duration::from_millis(5));
            }
        });

        let config = TransportConfig {
            retry_window: Duration::from_millis(20),
            ..TransportConfig::default()
        };
        let mut transport = TcpTransport::connect_with_config(address, config).unwrap();
        let error = transport
            .send_frame_idempotent(&[8, 0, 0, 0, 7, 0, 0, 0])
            .unwrap_err();
        assert!(error.is_connection_lost() || matches!(error, Error::Io(_)));
        worker.join().unwrap();
    }

    #[test]
    fn rejects_malformed_outgoing_frame_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut transport = TcpTransport::connect(listener.local_addr().unwrap()).unwrap();
        assert!(matches!(
            transport.send_frame(&[8, 0, 0, 0, 7]),
            Err(Error::InvalidFrame(message)) if message.contains("declares 8")
        ));
    }

    #[test]
    fn enforces_max_frame_bytes_on_outgoing_frames() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let config = TransportConfig {
            max_frame_bytes: 4,
            ..TransportConfig::default()
        };
        let mut transport =
            TcpTransport::connect_with_config(listener.local_addr().unwrap(), config).unwrap();
        assert!(matches!(
            transport.send_frame(&[5, 0, 0, 0, 1]),
            Err(Error::ResourceLimit(message)) if message.contains("outgoing frame")
        ));
    }

    struct ScriptedWriter {
        writes: usize,
        first_write_delay: Duration,
    }

    impl RawWriter for ScriptedWriter {
        fn set_write_timeout(&mut self, _timeout: Option<Duration>) -> io::Result<()> {
            Ok(())
        }

        fn write_bytes(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            if self.writes == 1 {
                thread::sleep(self.first_write_delay);
                Ok(bytes.len().min(1))
            } else {
                Ok(bytes.len())
            }
        }
    }

    #[test]
    fn raw_image_writer_enforces_one_absolute_deadline_across_multiple_writes() {
        let mut writer = ScriptedWriter {
            writes: 0,
            first_write_delay: Duration::from_millis(20),
        };
        let error = write_all_with_deadline_to(
            &mut writer,
            &[1, 2],
            Some(Instant::now() + Duration::from_millis(5)),
            None,
        )
        .unwrap_err();
        assert!(error.is_timeout());
        assert_eq!(writer.writes, 1);
    }

    #[test]
    fn preserves_bytes_read_after_a_frame_for_the_next_raw_transfer() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1];
            stream.read_exact(&mut request).unwrap();
            let frame = [8u8, 0, 0, 0, 7, 0, 0, 0];
            stream.write_all(&frame[..2]).unwrap();
            thread::sleep(Duration::from_millis(5));
            stream.write_all(&frame[2..5]).unwrap();
            thread::sleep(Duration::from_millis(5));
            stream.write_all(&frame[5..]).unwrap();
            stream.write_all(b"raw").unwrap();
        });

        let mut transport = TcpTransport::connect(address).unwrap();
        transport.write_all(&[1]).unwrap();
        let frame = transport.read_frame().unwrap();
        assert_eq!(frame, vec![8, 0, 0, 0, 7, 0, 0, 0]);
        assert_eq!(transport.read_exact_bytes(3).unwrap(), b"raw");
        worker.join().unwrap();
    }
}
