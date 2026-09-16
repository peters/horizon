#![forbid(unsafe_code)]

//! Shared filesystem coordination for embedded and standalone browser hosts.

pub mod manifest;
pub mod paths;

pub use paths::BrowserRuntimePaths;
