//! Recovery is available only for an existing pinned, pre-admission bootstrap.
#[cfg(target_os = "linux")]
mod recovery;
#[cfg(target_os = "linux")]
mod store;
use std::io;

pub(super) fn run() -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        recovery::run()
    }
    #[cfg(not(target_os = "linux"))]
    Err(io::Error::other(
        "Allocation recovery requires a qualified Linux worker",
    ))
}
