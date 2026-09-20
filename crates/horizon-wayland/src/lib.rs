//! Wayland protocol support that winit 0.30 does not provide.
//!
//! The crate exists to quarantine one `unsafe` call — adopting a foreign
//! `wl_display` — behind a safe API, so every other Horizon crate can keep
//! `#![forbid(unsafe_code)]`.

#![deny(unsafe_code)]

#[cfg(target_os = "linux")]
mod pinch;

#[cfg(target_os = "linux")]
pub use pinch::{Pinch, PinchBridge};
