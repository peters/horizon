//! Sends this Horizon's current prices to its ready workers, so agents there can rank
//! cloud offers. Only prices travel: the provider account never leaves this computer.
use super::{Cancellation, Result, companions::transport::Live, settings::Settings};
pub use horizon_cloud_protocol::offers::{Snapshot, VERSION};
use std::path::Path;

/// What happened to a snapshot sent to a cloud's worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Published {
    Sent,
    /// The worker is not ready, so nothing was sent.
    NotReady,
}

/// Sends `snapshot` to the worker of `cloud` when it is ready.
/// # Errors
/// Fails on unreadable settings, a cloud another operation holds, an unreachable worker
/// or a worker image without cloud offer support.
pub fn publish(root: &Path, cloud: &str, snapshot: &Snapshot, cancel: &Cancellation) -> Result<Published> {
    let settings = Settings::load(&root.join("settings.json"))?;
    Live::new(root, &settings, cancel).send_offers(cloud, snapshot)
}
