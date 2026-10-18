#![forbid(unsafe_code)]

//! Shared filesystem coordination for embedded and standalone browser hosts.

pub mod atomic_file;
pub mod attachments;
pub mod manifest;
pub mod paths;
pub mod remote_authorization;
pub mod remote_slots;

pub use attachments::{AttachmentPolicy, AttachmentPolicyError, AuthorizedFile, MAX_ATTACHMENT_BYTES};
pub use paths::BrowserRuntimePaths;
