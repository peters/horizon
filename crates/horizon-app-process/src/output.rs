//! Anchored log capabilities share one physical cap and one append transaction policy.
use crate::diagnostic::{Cause, DiagnosticError, Retention};
use std::time::Duration;

#[cfg(unix)]
pub(crate) const MAX_BYTES: u64 = 4 * 1024 * 1024;
#[cfg(unix)]
const DIAGNOSTIC_BYTES: u64 = 1024;

#[cfg(unix)]
mod unix {
    use super::{Cause, DIAGNOSTIC_BYTES, DiagnosticError, Duration, MAX_BYTES, Retention};
    use crate::diagnostic::Diagnostic;
    use crate::{Spec, storage::Directory};
    use serde::{Deserialize, Serialize};
    use std::{
        fs::File,
        io::{Read, Write},
        os::unix::fs::{FileExt, MetadataExt},
        path::PathBuf,
        sync::{Arc, Mutex},
        time::Instant,
    };
    use uuid::Uuid;

    type Recorded = std::result::Result<Retention, DiagnosticError>;
    type Attempt = Arc<Mutex<Option<Recorded>>>;
    #[derive(Clone)]
    pub struct DiagnosticLog {
        writer: Arc<Mutex<Writer>>,
        attempt: Arc<Mutex<Option<Attempt>>>,
    }
    struct Writer {
        directory: Directory,
        state: PathBuf,
        root: PathBuf,
        root_identity: (u64, u64),
        log: File,
        identity: (u64, u64),
        operation: Uuid,
        guardian: u32,
        #[cfg(test)]
        sync_count: std::sync::atomic::AtomicUsize,
        #[cfg(test)]
        fail_sync_at: std::sync::atomic::AtomicUsize,
    }
    #[derive(Deserialize)]
    struct Receipt {
        operation: Uuid,
        guardian_pid: u32,
    }
    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct Marker {
        operation: Uuid,
        offset: u64,
        cause: Cause,
    }
    struct Locked<'a>(&'a File);
    impl Drop for Locked<'_> {
        fn drop(&mut self) {
            let _ = rustix::fs::flock(self.0, rustix::fs::FlockOperation::Unlock);
        }
    }
    impl Writer {
        fn validate(&self) -> std::result::Result<(), DiagnosticError> {
            self.directory
                .matches_path(&self.state)
                .map_err(|_| DiagnosticError::Unavailable)?;
            if self.root.canonicalize()? != self.root {
                return Err(DiagnosticError::Unavailable);
            }
            let root = File::open(&self.root)?.metadata()?;
            if (root.dev(), root.ino()) != self.root_identity {
                return Err(DiagnosticError::Unavailable);
            }
            let visible = self.directory.existing_file("output.log", false)?.metadata()?;
            let held = self.log.metadata()?;
            if (visible.dev(), visible.ino()) != self.identity
                || (held.dev(), held.ino()) != self.identity
                || held.nlink() != 1
                || held.uid() != rustix::process::getuid().as_raw()
                || held.mode() & 0o077 != 0
            {
                return Err(DiagnosticError::Unavailable);
            }
            let receipt: Receipt = self.directory.receipt().map_err(|_| DiagnosticError::Unavailable)?;
            if receipt.operation != self.operation || receipt.guardian_pid != self.guardian {
                return Err(DiagnosticError::Unavailable);
            }
            Ok(())
        }
        fn lock(&self, deadline: Instant) -> std::result::Result<Locked<'_>, DiagnosticError> {
            loop {
                if Instant::now() >= deadline {
                    return Err(DiagnosticError::Timeout);
                }
                match rustix::fs::flock(&self.log, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
                    Ok(()) => return Ok(Locked(&self.log)),
                    Err(rustix::io::Errno::WOULDBLOCK) => (),
                    Err(error) => return Err(std::io::Error::from(error).into()),
                }
                if Instant::now() >= deadline {
                    return Err(DiagnosticError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        fn marker(&self) -> std::result::Result<Option<(Marker, Vec<u8>)>, DiagnosticError> {
            let file = match self.directory.existing_file("host-diagnostic.json", false) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            let mut bytes = Vec::new();
            file.take(DIAGNOSTIC_BYTES + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > DIAGNOSTIC_BYTES {
                return Err(DiagnosticError::Unavailable);
            }
            let marker: Marker = serde_json::from_slice(&bytes).map_err(|_| DiagnosticError::Unavailable)?;
            if marker.operation != self.operation || marker.offset > MAX_BYTES - DIAGNOSTIC_BYTES {
                return Err(DiagnosticError::Unavailable);
            }
            let line = line(Diagnostic::Host(marker.cause))?;
            let end = marker.offset + line.len() as u64;
            if self.log.metadata()?.len() < end {
                return Err(DiagnosticError::Unavailable);
            }
            let mut retained = vec![0; line.len()];
            self.log.read_exact_at(&mut retained, marker.offset)?;
            if retained != line {
                return Err(DiagnosticError::Unavailable);
            }
            Ok(Some((marker, line)))
        }
        fn durable(&self, sync: impl FnOnce() -> std::io::Result<()>) -> std::result::Result<(), DiagnosticError> {
            self.validate()?;
            #[cfg(test)]
            {
                use std::sync::atomic::Ordering;
                let count = self.sync_count.fetch_add(1, Ordering::Relaxed) + 1;
                if self.fail_sync_at.load(Ordering::Relaxed) == count {
                    return Err(DiagnosticError::Io(std::io::ErrorKind::Other));
                }
            }
            sync().map_err(DiagnosticError::from)
        }
        fn retain_durably(&self) -> std::result::Result<(), DiagnosticError> {
            let marker = self.directory.existing_file("host-diagnostic.json", true)?;
            self.durable(|| marker.sync_all())?;
            self.durable(|| self.log.sync_all())?;
            self.durable(|| self.directory.sync_held())?;
            self.validate()?;
            self.marker()?.ok_or(DiagnosticError::Unavailable)?;
            Ok(())
        }
        fn record(&self, cause: Cause, deadline: Instant) -> Recorded {
            let _lock = self.lock(deadline)?;
            self.validate()?;
            if self.marker()?.is_some() {
                self.retain_durably()?;
                return Ok(Retention::AlreadyRecorded);
            }
            let offset = self.log.metadata()?.len();
            if offset > MAX_BYTES - DIAGNOSTIC_BYTES {
                return Err(DiagnosticError::Unavailable);
            }
            let bytes = line(Diagnostic::Host(cause))?;
            let marker = Marker {
                operation: self.operation,
                offset,
                cause,
            };
            let encoded = serde_json::to_vec(&marker).map_err(|_| DiagnosticError::Invalid)?;
            if encoded.len() as u64 > DIAGNOSTIC_BYTES {
                return Err(DiagnosticError::Invalid);
            }
            let mut intent = self
                .directory
                .new_file("host-diagnostic.json")
                .map_err(|_| DiagnosticError::Unavailable)?;
            intent.write_all(&encoded)?;
            self.durable(|| intent.sync_all())?;
            let mut output = &self.log;
            output.write_all(&bytes)?;
            self.durable(|| self.log.sync_all())?;
            self.durable(|| self.directory.sync_held())?;
            self.validate()?;
            self.marker()?.ok_or(DiagnosticError::Unavailable)?;
            Ok(Retention::Recorded)
        }
        fn append(&self, bytes: &[u8]) -> std::result::Result<(), DiagnosticError> {
            let _lock = self.lock(Instant::now() + Duration::from_secs(1))?;
            self.validate()?;
            let retained = self.marker()?.map_or(0, |(_, line)| line.len() as u64);
            let length = self.log.metadata()?.len();
            let limit = MAX_BYTES - DIAGNOSTIC_BYTES + retained;
            let available = usize::try_from(limit.saturating_sub(length)).map_err(|_| DiagnosticError::Unavailable)?;
            let count = bytes.len().min(available);
            let mut output = &self.log;
            output.write_all(&bytes[..count])?;
            Ok(())
        }
    }
    fn line(diagnostic: Diagnostic) -> std::result::Result<Vec<u8>, DiagnosticError> {
        let mut bytes = serde_json::to_vec(&serde_json::json!({"native_host_diagnostic":1,
            "cause":diagnostic,"message":diagnostic.message()}))
        .map_err(|_| DiagnosticError::Invalid)?;
        bytes.insert(0, b'\n');
        bytes.push(b'\n');
        if bytes.len() as u64 > DIAGNOSTIC_BYTES {
            return Err(DiagnosticError::Invalid);
        }
        Ok(bytes)
    }
    impl DiagnosticLog {
        pub(crate) fn create(spec: &Spec, guardian: u32, directory: Directory) -> crate::Result<Self> {
            directory
                .new_file("output.log")?
                .sync_all()
                .map_err(|_| crate::Error::StateUnavailable)?;
            Self::capture(spec, guardian, directory)
        }
        pub(crate) fn capture(spec: &Spec, guardian: u32, directory: Directory) -> crate::Result<Self> {
            let log = directory
                .existing_file("output.log", true)
                .map_err(|_| crate::Error::StateUnavailable)?;
            let identity = log.metadata().map_err(|_| crate::Error::StateUnavailable)?;
            let root_file = File::open(&spec.root).map_err(|_| crate::Error::StateUnavailable)?;
            if crate::root_identity(&root_file)? != (spec.root_device, spec.root_inode) {
                return Err(crate::Error::StateUnavailable);
            }
            let root = root_file.metadata().map_err(|_| crate::Error::StateUnavailable)?;
            let writer = Writer {
                directory,
                state: spec.state.clone(),
                root: spec.root.clone(),
                root_identity: (root.dev(), root.ino()),
                identity: (identity.dev(), identity.ino()),
                log,
                operation: spec.operation,
                guardian,
                #[cfg(test)]
                sync_count: std::sync::atomic::AtomicUsize::new(0),
                #[cfg(test)]
                fail_sync_at: std::sync::atomic::AtomicUsize::new(0),
            };
            writer.validate().map_err(|_| crate::Error::StateUnavailable)?;
            Ok(Self {
                writer: Arc::new(Mutex::new(writer)),
                attempt: Arc::new(Mutex::new(None)),
            })
        }
        /// Retain the first finite cause without creating, reopening or recovering foreign state.
        /// # Errors
        /// Identity, partial-write and deadline failures do not acknowledge diagnostic retention.
        pub fn record(&self, cause: Cause, timeout: Duration) -> Recorded {
            if timeout.is_zero() || timeout > Duration::from_secs(2) {
                return Err(DiagnosticError::Invalid);
            }
            let deadline = Instant::now() + timeout;
            let mut attempt = self.attempt.lock().map_err(|_| DiagnosticError::Unavailable)?;
            let pending = match attempt.as_ref() {
                Some(result) if result.lock().map_err(|_| DiagnosticError::Unavailable)?.is_none() => {
                    Some(Arc::clone(result))
                }
                _ => None,
            };
            let initiated = pending.is_none();
            let result = if let Some(result) = pending {
                result
            } else {
                let result = Arc::new(Mutex::new(None));
                let retained = Arc::clone(&result);
                let writer = Arc::clone(&self.writer);
                std::thread::Builder::new()
                    .name("native-log-diagnostic".into())
                    .spawn(move || {
                        let outcome = writer
                            .lock()
                            .map_err(|_| DiagnosticError::Unavailable)
                            .and_then(|writer| writer.record(cause, deadline));
                        if let Ok(mut result) = retained.lock() {
                            *result = Some(outcome);
                        }
                    })?;
                *attempt = Some(Arc::clone(&result));
                result
            };
            drop(attempt);
            loop {
                if let Some(outcome) = *result.lock().map_err(|_| DiagnosticError::Unavailable)? {
                    return if initiated {
                        outcome
                    } else {
                        outcome.map(|_| Retention::AlreadyRecorded)
                    };
                }
                if Instant::now() >= deadline {
                    return Err(DiagnosticError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        #[cfg(test)]
        pub(crate) fn fail_sync_at(&self, count: usize) {
            self.writer
                .lock()
                .unwrap()
                .fail_sync_at
                .store(count, std::sync::atomic::Ordering::Relaxed);
        }
        #[cfg(test)]
        pub(crate) fn sync_count(&self) -> usize {
            self.writer
                .lock()
                .unwrap()
                .sync_count
                .load(std::sync::atomic::Ordering::Relaxed)
        }
        pub(crate) fn append(&self, bytes: &[u8]) -> crate::Result<()> {
            self.writer
                .lock()
                .map_err(|_| crate::Error::StateUnavailable)?
                .append(bytes)
                .map_err(|_| crate::Error::StateUnavailable)
        }
        pub(crate) fn guardian(&self, reason: crate::diagnostic::GuardianReason) {
            if let Ok(bytes) = line(Diagnostic::Guardian(reason)) {
                let _ = self.append(&bytes);
            }
        }
        pub(crate) fn sync(&self) -> crate::Result<()> {
            self.writer
                .lock()
                .map_err(|_| crate::Error::StateUnavailable)?
                .log
                .sync_all()
                .map_err(|_| crate::Error::StateUnavailable)
        }
    }
}
#[cfg(unix)]
pub use unix::DiagnosticLog;
#[cfg(not(unix))]
#[derive(Clone)]
pub struct DiagnosticLog;
#[cfg(not(unix))]
impl DiagnosticLog {
    pub(crate) fn capture(
        _spec: &crate::Spec,
        _guardian: u32,
        _directory: crate::storage::Directory,
    ) -> crate::Result<Self> {
        Err(crate::Error::StateUnavailable)
    }
    /// # Errors
    /// Native process diagnostics require a supported Unix host.
    pub fn record(&self, _cause: Cause, _timeout: Duration) -> Result<Retention, DiagnosticError> {
        Err(DiagnosticError::Unavailable)
    }
}
#[cfg(all(test, unix))]
mod tests;
