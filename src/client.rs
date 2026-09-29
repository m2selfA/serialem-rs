use std::env;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;
use std::time::{Duration, Instant};

use crate::error::Error;
use crate::image::{
    BufferImage, BufferSelector, PutImageOptions, chunk_count, chunk_size,
    decode_buffer_image_meta, decode_put_image_ack, encode_get_buffer_image,
    encode_put_image_request, image_send_ranges, validate_buffer_image_meta,
};
use crate::protocol::{
    DEFAULT_EXTERNAL_PORT, PSS_CHUNK_HANDSHAKE, PYTHONMODULE_SUPER_CHUNK_SIZE, RegularResponse,
    ReportValue, SCRIPT_EXIT_NO_EXC, SCRIPT_NORMAL_EXIT, SCRIPT_USER_STOP, ScriptItem,
    decode_fixed_or_error_frame, decode_regular_response, encode_frame,
    encode_ok_to_run_external_script, encode_regular_command_with_limit,
};
use crate::transport::{TcpTransport, TransportConfig};

#[derive(Debug, Clone, PartialEq)]
pub enum CommandResult {
    None,
    Number(f64),
    Text(String),
    Tuple(Vec<ReportValue>),
}

#[derive(Debug)]
pub struct SerialEmClient {
    transport: TcpTransport,
    script_initialized: bool,
    always_return_tuples: bool,
    buffer_image_timeout: Option<Duration>,
}

impl SerialEmClient {
    pub fn connect(address: SocketAddr) -> Result<Self, Error> {
        Self::connect_with_config(address, TransportConfig::default())
    }

    pub fn connect_default() -> Result<Self, Error> {
        let ip = match env::var("PY_SERIALEM_IP") {
            Ok(value) => IpAddr::from_str(&value).map_err(|error| {
                Error::InvalidArgument(format!("PY_SERIALEM_IP '{value}' is invalid: {error}"))
            })?,
            Err(env::VarError::NotPresent) => IpAddr::V4(Ipv4Addr::LOCALHOST),
            Err(error) => {
                return Err(Error::InvalidArgument(format!(
                    "could not read PY_SERIALEM_IP: {error}"
                )));
            }
        };
        let port = match env::var("PY_SERIALEM_PORT") {
            Ok(value) => value.parse::<u16>().map_err(|error| {
                Error::InvalidArgument(format!("PY_SERIALEM_PORT '{value}' is invalid: {error}"))
            })?,
            Err(env::VarError::NotPresent) => DEFAULT_EXTERNAL_PORT,
            Err(error) => {
                return Err(Error::InvalidArgument(format!(
                    "could not read PY_SERIALEM_PORT: {error}"
                )));
            }
        };
        Self::connect_at(ip, port)
    }

    pub fn connect_at(ip: IpAddr, port: u16) -> Result<Self, Error> {
        Self::connect(SocketAddr::new(ip, port))
    }

    pub fn connect_with_config(
        address: SocketAddr,
        config: TransportConfig,
    ) -> Result<Self, Error> {
        Ok(Self {
            transport: TcpTransport::connect_with_config(address, config)?,
            script_initialized: false,
            always_return_tuples: false,
            buffer_image_timeout: None,
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.transport.address()
    }

    /// Matches the pinned PythonModule's `ReportNumModuleFuncs`, which returns
    /// the complete CME enum length, including enum-only entries.
    pub fn report_num_module_funcs(&self) -> usize {
        crate::generated::command_ids::CME_ENUM_LENGTH
    }

    /// Number of externally generated Python/Rust command wrappers.
    pub fn report_num_external_funcs(&self) -> usize {
        crate::generated::command_ids::EXTERNAL_COMMAND_SPECS.len()
    }

    /// Advanced state injection compatible with PythonModule's
    /// `ScriptIsInitialized`; it deliberately bypasses the readiness handshake.
    pub fn mark_script_initialized(&mut self) {
        self.script_initialized = true;
    }

    #[deprecated(note = "use mark_script_initialized; this bypasses the readiness handshake")]
    pub fn script_is_initialized(&mut self) {
        self.mark_script_initialized();
    }

    pub fn return_all_values_as_tuples(&mut self, enabled: bool) {
        self.always_return_tuples = enabled;
    }

    pub fn always_return_tuples(&self) -> bool {
        self.always_return_tuples
    }

    pub fn set_buffer_image_timeout(&mut self, seconds: f32) -> Result<(), Error> {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(Error::InvalidArgument(
                "buffer image timeout must be a finite non-negative number".to_string(),
            ));
        }
        self.buffer_image_timeout = if seconds == 0.0 {
            None
        } else {
            Some(Duration::try_from_secs_f32(seconds).map_err(|error| {
                Error::InvalidArgument(format!(
                    "buffer image timeout {seconds} seconds is out of range: {error}"
                ))
            })?)
        };
        Ok(())
    }

    pub fn ok_to_run_external_script(&mut self) -> Result<bool, Error> {
        let request = encode_ok_to_run_external_script()?;
        let response = self.send_frame(&request)?;
        let decoded = match decode_fixed_or_error_frame(&response, 1, 1, 0) {
            Ok(decoded) => decoded,
            Err(error) => {
                self.invalidate_for_protocol_error(&error);
                return Err(error);
            }
        };
        let return_code = decoded.longs[0];
        if return_code < 0 {
            self.script_initialized = false;
            return Err(Error::from_server_code(return_code));
        }
        let ready = decoded.bools[0] != 0;
        if !ready {
            self.script_initialized = false;
        }
        Ok(ready)
    }

    pub fn ensure_external_control(&mut self) -> Result<(), Error> {
        if self.script_initialized && !self.transport.needs_reconnect() {
            return Ok(());
        }
        self.script_initialized = false;
        if !self.ok_to_run_external_script()? {
            return Err(Error::Busy);
        }
        self.script_initialized = true;
        Ok(())
    }

    pub fn execute_regular(
        &mut self,
        function_code: i32,
        items: &[ScriptItem],
        last_non_empty_index: usize,
    ) -> Result<RegularResponse, Error> {
        let request = encode_regular_command_with_limit(
            function_code,
            items,
            last_non_empty_index,
            self.transport.max_frame_bytes(),
        )?;
        self.ensure_external_control()?;
        let response = self.send_frame(&request)?;
        let decoded = match decode_regular_response(&response) {
            Ok(decoded) => decoded,
            Err(error) => {
                self.invalidate_for_protocol_error(&error);
                return Err(error);
            }
        };
        if decoded.return_code < 0 {
            return Err(Error::from_server_code(decoded.return_code));
        }
        Ok(decoded)
    }

    pub fn buffer_image(&mut self, specification: &str) -> Result<BufferImage, Error> {
        let selector = BufferSelector::parse(specification)?;
        self.get_buffer_image(selector.buffer, selector.fft)
    }

    pub fn get_buffer_image(
        &mut self,
        buffer: crate::image::BufferIndex,
        if_fft: bool,
    ) -> Result<BufferImage, Error> {
        buffer.validate_for_fft(if_fft)?;
        let request = encode_get_buffer_image(buffer, if_fft)?;
        let response = self.send_frame(&request)?;
        let meta = match decode_buffer_image_meta(&response) {
            Ok(meta) => meta,
            Err(error) => {
                self.invalidate_for_protocol_error(&error);
                return Err(error);
            }
        };
        if let Err(error) = validate_buffer_image_meta(&meta, self.transport.max_image_bytes()) {
            self.invalidate_connection();
            return Err(error);
        }
        let chunk_bytes = match chunk_size(meta.byte_count, meta.chunks) {
            Ok(chunk_bytes) => chunk_bytes,
            Err(error) => {
                self.invalidate_connection();
                return Err(error);
            }
        };
        let previous_timeout = self.transport.read_timeout();
        let mut current_timeout = previous_timeout;
        let mut timeout_changed = false;
        let transfer = (|| {
            let mut bytes = reserve_image_bytes(meta.byte_count)?;
            for chunk_index in 0..meta.chunks {
                if chunk_index > 0 {
                    let handshake = encode_frame(&[PSS_CHUNK_HANDSHAKE], &[], &[], &[])?;
                    self.transport.write_frame(&handshake)?;
                }
                let remaining = meta.byte_count - bytes.len();
                let count = remaining.min(chunk_bytes);
                let desired_timeout = if count > 1024 {
                    self.buffer_image_timeout.or(previous_timeout)
                } else {
                    previous_timeout
                };
                if desired_timeout != current_timeout {
                    self.transport.set_read_timeout(desired_timeout)?;
                    current_timeout = desired_timeout;
                    timeout_changed = true;
                }
                self.transport.read_exact_into(&mut bytes, count)?;
            }
            Ok(BufferImage { meta, bytes })
        })();
        let restore = if timeout_changed {
            self.transport.set_read_timeout(previous_timeout)
        } else {
            Ok(())
        };
        match (transfer, restore) {
            (Err(error), Ok(())) => {
                self.invalidate_connection();
                Err(error)
            }
            (Err(original), Err(Error::Io(source))) => {
                self.invalidate_connection();
                Err(Error::TransportStateRestore {
                    source,
                    original: Some(Box::new(original)),
                })
            }
            (Err(original), Err(restore_error)) => {
                self.invalidate_connection();
                Err(Error::TransportStateRestore {
                    source: io::Error::other(restore_error.to_string()),
                    original: Some(Box::new(original)),
                })
            }
            (Ok(_image), Err(Error::Io(source))) => {
                self.invalidate_connection();
                Err(Error::TransportStateRestore {
                    source,
                    original: None,
                })
            }
            (Ok(_image), Err(restore_error)) => {
                self.invalidate_connection();
                Err(Error::TransportStateRestore {
                    source: io::Error::other(restore_error.to_string()),
                    original: None,
                })
            }
            (Ok(image), Ok(())) => Ok(image),
        }
    }

    pub fn put_image_in_buffer(
        &mut self,
        image: &[u8],
        options: PutImageOptions,
    ) -> Result<(), Error> {
        if options.size_x == 0 || options.size_y == 0 {
            return Err(Error::InvalidArgument(
                "image width and height must be positive".to_string(),
            ));
        }
        if image.is_empty() {
            return Err(Error::InvalidArgument(
                "image data must not be empty".to_string(),
            ));
        }
        if image.len() > self.transport.max_image_bytes() {
            return Err(Error::ResourceLimit(format!(
                "image contains {} bytes, configured maximum is {}",
                image.len(),
                self.transport.max_image_bytes()
            )));
        }
        let expected_bytes = options
            .size_x
            .checked_mul(options.size_y)
            .and_then(|pixels| pixels.checked_mul(options.mode.item_size()))
            .ok_or_else(|| Error::InvalidArgument("image dimensions overflow".to_string()))?;
        if expected_bytes != image.len() {
            return Err(Error::InvalidArgument(format!(
                "image has {} bytes, expected {expected_bytes}",
                image.len()
            )));
        }
        let chunks = chunk_count(image.len(), PYTHONMODULE_SUPER_CHUNK_SIZE)?;
        let ranges = image_send_ranges(image.len())?;
        let expected_chunks = chunk_count(image.len(), PYTHONMODULE_SUPER_CHUNK_SIZE)?;
        if chunks != expected_chunks {
            return Err(Error::InvalidFrame(format!(
                "planned image chunk count {chunks} does not match expected {expected_chunks}"
            )));
        }
        let request = encode_put_image_request(options, image.len(), chunks)?;
        let response = self.send_frame(&request)?;
        if let Err(error) = decode_put_image_ack(&response) {
            self.invalidate_for_protocol_error(&error);
            return Err(error);
        }
        let previous_write_timeout = self.transport.write_timeout();
        let image_deadline = match self
            .transport
            .image_write_timeout()
            .map(|timeout| {
                Instant::now().checked_add(timeout).ok_or_else(|| {
                    Error::InvalidArgument("image_write_timeout cannot be represented".to_string())
                })
            })
            .transpose()
        {
            Ok(deadline) => deadline,
            Err(error) => {
                self.invalidate_connection();
                return Err(error);
            }
        };
        let transfer = (|| {
            for range in ranges {
                self.transport
                    .write_raw_with_deadline(&image[range], image_deadline)?;
            }
            Ok(())
        })();
        let restore = if image_deadline.is_some() {
            match self.transport.set_write_timeout(previous_write_timeout) {
                Ok(()) => None,
                Err(Error::Io(source)) => Some(source),
                Err(error) => Some(io::Error::other(error.to_string())),
            }
        } else {
            None
        };
        if let Some(source) = restore {
            self.invalidate_connection();
            return Err(Error::TransportStateRestore {
                source,
                original: transfer.err().map(Box::new),
            });
        }
        if let Err(error) = transfer {
            self.invalidate_connection();
            return Err(error);
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn execute_generated(
        &mut self,
        function_code: i32,
        items: Vec<ScriptItem>,
        last_non_empty_index: usize,
    ) -> Result<CommandResult, Error> {
        let response = self.execute_regular(function_code, &items, last_non_empty_index)?;
        self.convert_response(response)
    }

    #[allow(dead_code)]
    fn convert_response(&mut self, response: RegularResponse) -> Result<CommandResult, Error> {
        match response.error_occurred {
            0 => {}
            SCRIPT_NORMAL_EXIT => {
                self.script_initialized = false;
                return Err(Error::ScriptExited("normal exit"));
            }
            SCRIPT_USER_STOP => {
                self.script_initialized = false;
                return Err(Error::ScriptExited("user STOP"));
            }
            SCRIPT_EXIT_NO_EXC => {
                self.script_initialized = false;
                return Ok(CommandResult::None);
            }
            code => {
                let message = response.reports.first().and_then(|report| match report {
                    ReportValue::Text(text) => Some(text.clone()),
                    ReportValue::Number(_) => None,
                });
                return Err(message
                    .map(Error::SerialEmMessage)
                    .unwrap_or(Error::SerialEm { code }));
            }
        }

        match response.reports.len() {
            0 => Ok(CommandResult::None),
            1 => match response.reports[0].clone() {
                ReportValue::Text(text) => Ok(CommandResult::Text(text)),
                ReportValue::Number(number) if self.always_return_tuples => {
                    Ok(CommandResult::Tuple(vec![ReportValue::Number(number)]))
                }
                ReportValue::Number(number) => Ok(CommandResult::Number(number)),
            },
            _ => Ok(CommandResult::Tuple(response.reports)),
        }
    }

    pub fn invalidate_connection(&mut self) {
        self.transport.invalidate();
        self.script_initialized = false;
    }

    pub fn close(&mut self) -> Result<(), Error> {
        self.script_initialized = false;
        self.transport.close()
    }

    pub fn reconnect(&mut self) -> Result<(), Error> {
        self.transport.reconnect()?;
        self.script_initialized = false;
        Ok(())
    }

    pub fn needs_reconnect(&self) -> bool {
        self.transport.needs_reconnect()
    }

    fn send_frame(&mut self, frame: &[u8]) -> Result<Vec<u8>, Error> {
        match self.transport.send_frame(frame) {
            Ok(response) => Ok(response),
            Err(error) => {
                self.script_initialized = false;
                Err(error)
            }
        }
    }

    fn invalidate_for_protocol_error(&mut self, error: &Error) {
        if error.is_transport_failure()
            || matches!(
                error,
                Error::Busy | Error::UserStop | Error::SerialEm { .. }
            )
        {
            self.invalidate_connection();
        }
    }
}

#[allow(dead_code)]
pub(crate) fn script_item_from_int(value: i32) -> ScriptItem {
    ScriptItem {
        int_value: value,
        double_value: value as f64,
        string_value: value.to_string(),
    }
}

#[allow(dead_code)]
pub(crate) fn script_item_from_double(value: f64) -> ScriptItem {
    ScriptItem {
        int_value: 0,
        double_value: value,
        string_value: format_double_for_serialem(value),
    }
}

#[allow(dead_code)]
pub(crate) fn script_item_from_string(value: &str) -> ScriptItem {
    ScriptItem {
        int_value: 0,
        double_value: value
            .parse::<f64>()
            .ok()
            .filter(|parsed| parsed.is_finite())
            .unwrap_or(0.0),
        string_value: value.to_string(),
    }
}

#[allow(dead_code)]
fn format_double_for_serialem(value: f64) -> String {
    value.to_string()
}

fn reserve_image_bytes(byte_count: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(byte_count).map_err(|error| {
        Error::ResourceLimit(format!(
            "cannot reserve {byte_count} image bytes: {error:?}"
        ))
    })?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn reports_upstream_module_count_and_external_wrapper_count_separately() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = SerialEmClient::connect(listener.local_addr().unwrap()).unwrap();
        assert_eq!(client.report_num_module_funcs(), 843);
        assert_eq!(client.report_num_external_funcs(), 793);
    }

    #[test]
    fn preserves_high_precision_floating_point_string_arguments() {
        assert_eq!(
            format_double_for_serialem(1.2345678901234567),
            "1.2345678901234567"
        );
        assert_eq!(
            format_double_for_serialem(-0.000000123456789),
            "-0.000000123456789"
        );
    }

    #[test]
    fn rejects_non_finite_floating_point_arguments() {
        let item = script_item_from_double(f64::NAN);
        assert!(matches!(
            crate::protocol::encode_regular_command(42, &[ScriptItem::default(), item], 1),
            Err(Error::InvalidArgument(message)) if message.contains("finite")
        ));
        assert!(script_item_from_string("NaN").double_value.is_finite());
    }

    #[test]
    fn rejects_impossible_image_timeout_without_panicking() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = SerialEmClient::connect(listener.local_addr().unwrap()).unwrap();
        assert!(matches!(
            client.set_buffer_image_timeout(f32::MAX),
            Err(Error::InvalidArgument(_))
        ));
    }

    #[test]
    fn reports_impossible_image_allocation_without_panicking() {
        assert!(matches!(
            reserve_image_bytes(usize::MAX),
            Err(Error::ResourceLimit(_))
        ));
    }

    #[test]
    fn image_write_deadline_invalidates_and_restores_socket_timeout() {
        use std::io::{Read, Write};
        use std::thread;
        use std::time::Duration;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut header = [0u8; 4];
            stream.read_exact(&mut header).unwrap();
            let length = u32::from_le_bytes(header) as usize;
            let mut request = vec![0u8; length - 4];
            stream.read_exact(&mut request).unwrap();
            stream
                .write_all(&encode_frame(&[0], &[], &[], &[]).unwrap())
                .unwrap();
            thread::sleep(Duration::from_millis(100));
        });

        let config = TransportConfig {
            write_timeout: Some(Duration::from_millis(250)),
            image_write_timeout: Some(Duration::from_nanos(1)),
            ..TransportConfig::default()
        };
        let mut client = SerialEmClient::connect_with_config(address, config).unwrap();
        let error = client
            .put_image_in_buffer(
                b"x",
                PutImageOptions::new(crate::protocol::MrcMode::Byte, 1, 1),
            )
            .unwrap_err();
        assert!(error.is_timeout() || matches!(error, Error::TransportStateRestore { .. }));
        assert!(client.needs_reconnect());
        assert_eq!(
            client.transport.socket_write_timeout().unwrap(),
            Some(Duration::from_millis(250))
        );
        worker.join().unwrap();
    }
}
