#![forbid(unsafe_code)]

//! Shared filesystem coordination for embedded and standalone browser hosts.

pub mod attachments;
pub mod manifest;
pub mod paths;

pub use attachments::{AttachmentPolicy, AttachmentPolicyError};
pub use paths::BrowserRuntimePaths;
