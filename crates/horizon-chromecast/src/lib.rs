//! Google Cast sender. Discovers receivers, controls them over Cast v2 and
//! serves live H.264 to them. Capture, encoding and UI belong to the host.
#![forbid(unsafe_code)]

mod channel;
mod client;
#[cfg(feature = "discovery")]
mod discovery;
mod live;
mod media;
mod proto;
mod receiver;
#[cfg(test)]
mod tests;

pub use client::{CastClient, Event};
#[cfg(feature = "discovery")]
pub use discovery::{Receiver, discover};
pub use live::{LiveCast, LiveOptions, LiveState, avcc_to_annexb};
pub use media::{MediaController, MediaLoad, MediaStatus, StreamType};
pub use receiver::{Application, DEFAULT_MEDIA_RECEIVER, ReceiverStatus, Volume};

/// Port Cast receivers listen on unless discovery reports another one.
pub const DEFAULT_PORT: u16 = 8009;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cast I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("cast TLS: {0}")]
    Tls(#[from] rustls::Error),
    #[cfg(feature = "discovery")]
    #[error("receiver discovery: {0}")]
    Discovery(#[from] horizon_media::discovery::DiscoveryError),
    #[error("invalid receiver message: {0}")]
    Protocol(&'static str),
    #[error("invalid receiver JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("receiver did not answer {0} in time")]
    Timeout(String),
    #[error("the receiver connection is closed")]
    Closed,
    #[error("invalid live options: {0}")]
    InvalidOptions(&'static str),
    #[error("invalid H.264: {0}")]
    H264(#[from] horizon_media::h264::H264Error),
    #[error("receiver rejected the request: {kind}{}", reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default())]
    Rejected { kind: String, reason: Option<String> },
}

pub type Result<T> = std::result::Result<T, Error>;
