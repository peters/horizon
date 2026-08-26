use thiserror::Error;

/// Synchronous engine startup failures. Browser and protocol failures after
/// the driver thread starts are delivered through [`crate::BrowserEvent`].
#[derive(Debug, Error)]
pub enum BrowserError {
    #[error("invalid browser launch configuration: {0}")]
    LaunchConfig(#[source] std::io::Error),
    #[error("failed to spawn browser driver thread: {0}")]
    DriverThread(#[source] std::io::Error),
}
