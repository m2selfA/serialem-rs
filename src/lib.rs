pub mod client;
pub mod error;
pub mod generated;
pub mod image;
pub mod protocol;
pub mod transport;

pub use client::{CommandResult, SerialEmClient};
pub use error::Error;
pub use image::{
    BufferImage, BufferImageMeta, BufferIndex, BufferSelector, MAX_IMAGE_CHUNKS, PutImageOptions,
};
pub use protocol::{MrcMode, RegularResponse, ReportValue, ScriptItem};
pub use transport::{
    DEFAULT_MAX_IMAGE_BYTES, PROTOCOL_MAX_FRAME_BYTES, PROTOCOL_MAX_IMAGE_BYTES, TcpTransport,
    TransportConfig,
};
