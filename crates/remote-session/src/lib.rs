//! Independent remote command transport. The caller owns trust, credentials and runtime.
#![forbid(unsafe_code)]

#[cfg(feature = "ssh")]
mod command;
#[cfg(feature = "ssh")]
mod control;
#[cfg(feature = "ssh")]
mod protocol;
#[cfg(feature = "ssh")]
mod types;

#[cfg(feature = "ssh")]
pub use command::{Command, CommandInput, execute};
#[cfg(feature = "ssh")]
pub use control::Cancellation;
#[cfg(feature = "ssh")]
pub use types::{Completion, Credentials, Error, Event, ExecRequest, Exit, HostKeyPin, Limits};
