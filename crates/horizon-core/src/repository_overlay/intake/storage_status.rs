//! Point-in-time qualification of the fixed worker root, independent of intake claims.

/// A read-only observation, never a retained permission or durability guarantee.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerStorageStatus {
    /// The existing private root passed the current confinement and filesystem checks.
    Qualified,
    /// The platform, filesystem or required kernel metadata is unsupported.
    Unsupported,
    /// The root could not be safely opened or its identity changed during inspection.
    Unavailable,
}

/// Inspect only `/workspace/.horizon-worker`; never create, synchronize or repair it.
/// Requires trusted kernel metadata and stable, exclusively controlled ancestry and
/// mount configuration. Does not inspect intake/setup records or repository contents.
/// Qualification grants no writes and proves neither capacity, fsync success, remote
/// persistence, backup nor retention through provider Stop/Delete. Every later write
/// must still perform its own admission checks. Run off the UI thread: filesystem
/// operations have no hard deadline, and the result can become stale immediately.
#[must_use]
pub fn inspect_worker_storage() -> WorkerStorageStatus {
    #[cfg(target_os = "linux")]
    return super::linux::inspect_storage();
    #[cfg(not(target_os = "linux"))]
    WorkerStorageStatus::Unsupported
}

#[cfg(all(test, not(target_os = "linux")))]
mod tests {
    #[test]
    fn unsupported_platform_never_opens_worker_storage() {
        assert_eq!(super::inspect_worker_storage(), super::WorkerStorageStatus::Unsupported);
    }
}
