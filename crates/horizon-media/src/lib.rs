//! Media helpers shared by the casting transports: H.264 handling, canvas
//! formats, optional encoder input and optional mDNS browsing. Capture stays
//! with the host.
#![forbid(unsafe_code)]

#[cfg(feature = "discovery")]
pub mod discovery;
#[cfg(feature = "encoder")]
pub mod encoder;
pub mod format;
pub mod h264;
