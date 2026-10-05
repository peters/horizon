//! Modern authenticated Apple TV transport. Capture and UI belong to the host.
#![forbid(unsafe_code)]

mod cancellation;
mod connection;
mod credentials;
mod crypto;
mod discovery;
mod encoder;
mod mirror;
mod pairing;
mod session;
mod srp;
mod tlv;
mod video;

pub use credentials::{PairedDevice, PairingCredentials, PairingStore};
pub use discovery::{Receiver, discover};
pub use encoder::{EncoderBackend, EncoderSelection};
pub use horizon_media::format::{Orientation, Resolution, VideoFormat};
pub use mirror::MirrorSession;
pub use pairing::{PairedReceiver, Pairing};
pub use session::{CastSession, CastStatus};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("this Apple TV already has a casting or pairing session")]
    AlreadyCasting,
    #[error("casting backend: {0}")]
    Backend(String),
    #[error("casting I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid receiver message: {0}")]
    Protocol(&'static str),
    #[error("missing or invalid receiver integer: {0}")]
    ReceiverInteger(&'static str),
    #[error("receiver rejected request with status {0}")]
    Status(u16),
    #[error("receiver rejected {phase} at step {step} with code {code}")]
    Pairing { phase: &'static str, step: u8, code: u8 },
    #[error("cryptographic authentication failed")]
    Authentication,
    #[error("invalid binary property list: {0}")]
    Plist(#[from] plist::Error),
}

impl From<horizon_media::encoder::FrameError> for Error {
    fn from(error: horizon_media::encoder::FrameError) -> Self {
        Self::Protocol(error.as_str())
    }
}

impl From<horizon_media::h264::H264Error> for Error {
    fn from(error: horizon_media::h264::H264Error) -> Self {
        Self::Protocol(error.as_str())
    }
}

type Result<T> = std::result::Result<T, Error>;
