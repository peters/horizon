//! Media helpers shared by the casting transports. Platform-independent and
//! free of capture and encoding; the only network code is optional mDNS browsing.
#![forbid(unsafe_code)]

#[cfg(feature = "discovery")]
pub mod discovery;
pub mod h264;
