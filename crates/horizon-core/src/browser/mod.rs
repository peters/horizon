//! Browser-engine infrastructure for future Horizon browser panels:
//! headless `Chrome` driven over the `CDP` protocol.
//!
//! Layout follows the module-boundary rules in
//! `docs/architecture/maintainability.md`:
//!
//! - `cdp` — `CDP` transport (websocket JSON-RPC)
//! - `process` — Chrome binary discovery, spawn, kill
//! - `frames` — JPEG decode + shared frame slot

pub mod cdp;
pub mod frames;
pub mod process;

pub use frames::FrameSlot;
