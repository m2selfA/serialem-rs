use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    InvalidArgument(String),
    InvalidFrame(String),
    ResourceLimit(String),
    Closed,
    TransportStateRestore {
        source: io::Error,
        original: Option<Box<Error>>,
    },
    SerialEm {
        code: i32,
    },
    SerialEmMessage(String),
    Busy,
    UserStop,
    ScriptExited(&'static str),
    Utf8(std::string::FromUtf8Error),
}

impl Error {
    pub fn from_server_code(code: i32) -> Self {
        match code {
            -9 => Self::Busy,
            -10 => Self::UserStop,
            _ => Self::SerialEm { code },
        }
    }

    pub fn is_timeout(&self) -> bool {
        matches!(
            self,
            Self::Io(error)
                if matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
        )
    }

    pub fn is_connection_lost(&self) -> bool {
        match self {
            Self::Closed | Self::TransportStateRestore { .. } => true,
            Self::Io(error) => matches!(
                error.kind(),
                io::ErrorKind::BrokenPipe
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::NotConnected
                    | io::ErrorKind::UnexpectedEof
            ),
            _ => false,
        }
    }

    pub fn is_transport_failure(&self) -> bool {
        self.is_timeout() || self.is_connection_lost() || matches!(self, Self::InvalidFrame(_))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::InvalidArgument(message) => write!(formatter, "invalid argument: {message}"),
            Self::InvalidFrame(message) => write!(formatter, "invalid SerialEM frame: {message}"),
            Self::ResourceLimit(message) => {
                write!(formatter, "SerialEM resource limit exceeded: {message}")
            }
            Self::Closed => formatter.write_str("SerialEM transport is permanently closed"),
            Self::TransportStateRestore { source, original } => {
                write!(
                    formatter,
                    "failed to restore SerialEM socket state: {source}"
                )?;
                if let Some(original) = original {
                    write!(formatter, " (original operation failed: {original})")?;
                }
                Ok(())
            }
            Self::SerialEm { code } => write!(formatter, "SerialEM returned error code {code}"),
            Self::SerialEmMessage(message) => {
                write!(formatter, "SerialEM command error: {message}")
            }
            Self::Busy => {
                formatter.write_str("SerialEM is busy or not ready for an external script")
            }
            Self::UserStop => formatter.write_str("SerialEM reported a user STOP"),
            Self::ScriptExited(reason) => write!(formatter, "SerialEM script exited: {reason}"),
            Self::Utf8(error) => write!(formatter, "invalid UTF-8 in SerialEM string: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::TransportStateRestore { source, .. } => Some(source),
            Self::Utf8(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<std::string::FromUtf8Error> for Error {
    fn from(error: std::string::FromUtf8Error) -> Self {
        Self::Utf8(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_io_and_utf8_sources() {
        let io_error = Error::from(io::Error::new(io::ErrorKind::BrokenPipe, "broken"));
        assert!(std::error::Error::source(&io_error).is_some());
        assert!(io_error.is_connection_lost());
        assert!(!io_error.is_timeout());

        let timeout_error = Error::from(io::Error::new(io::ErrorKind::TimedOut, "timeout"));
        assert!(timeout_error.is_timeout());
        assert!(!timeout_error.is_connection_lost());

        let utf8_error = Error::from(String::from_utf8(vec![0xff]).unwrap_err());
        assert!(std::error::Error::source(&utf8_error).is_some());
        assert!(!utf8_error.is_connection_lost());
        assert!(Error::Closed.is_connection_lost());
        let restore_error = Error::TransportStateRestore {
            source: io::Error::new(io::ErrorKind::BrokenPipe, "restore"),
            original: Some(Box::new(Error::Closed)),
        };
        assert!(restore_error.is_connection_lost());
        assert!(std::error::Error::source(&restore_error).is_some());
    }
}
