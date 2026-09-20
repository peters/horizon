//! Wayland protocol support that winit 0.30 does not provide.
//!
//! The crate exists to confine the platform FFI Horizon needs for trackpad
//! pinch: adopting the toolkit's `wl_display` through libwayland's
//! foreign-display entry point.
//!
//! That adoption cannot be made safe here. libwayland requires the display to
//! outlive the adopted backend, and winit's `run_app` takes the event loop by
//! value, so no borrow of it survives into the bridge for the type system to
//! check. [`PinchBridge::start`] is therefore an explicit `unsafe fn`, and its
//! one caller in `horizon-ui` discharges the contract by owning the bridge for
//! the life of the event loop and holding an `OwnedDisplayHandle` that outlives
//! it on both the normal and the unwinding path.
//!
//! This crate, its `horizon-ui` call site and `horizon-cursor` (whose Windows
//! cursor query has no safe equivalent) use `#![deny(unsafe_code)]` with
//! narrowly scoped `#[allow]`s rather than `forbid`. Every other Horizon crate
//! keeps `forbid`.

#![deny(unsafe_code)]

#[cfg(target_os = "linux")]
mod pinch;

#[cfg(target_os = "linux")]
pub use pinch::{Pinch, PinchBridge};
