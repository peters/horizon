//! The disk bound of a proof's clones, and the sweep of folders a stopped Horizon left.
use horizon_core::cloud_runtime::Cancellation;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A clone folder older than this was left by a Horizon that stopped mid-proof.
const STALE: Duration = Duration::from_hours(24);

/// Watches the size of a folder from a thread and cancels the proof once it passes a
/// bound, which ends the running clone's process group.
pub(super) struct SizeWatch {
    folder: PathBuf,
    bound: u64,
    cancel: Cancellation,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    passed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SizeWatch {
    pub(super) fn start(folder: &Path, bound: u64, cancel: Cancellation) -> Self {
        use std::sync::atomic::Ordering;
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let passed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (watched, watching, over, stopping) = (folder.to_path_buf(), done.clone(), passed.clone(), cancel.clone());
        let thread = std::thread::spawn(move || {
            while !watching.load(Ordering::Relaxed) {
                if size_of(&watched) > bound {
                    over.store(true, Ordering::Relaxed);
                    stopping.cancel();
                    return;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        });
        Self {
            folder: folder.to_path_buf(),
            bound,
            cancel,
            done,
            passed,
            thread: Some(thread),
        }
    }

    /// Stops watching and says whether the folder passed the bound, measuring it once
    /// more now that no clone writes, so one that grew between two looks is caught.
    pub(super) fn stop(mut self) -> bool {
        use std::sync::atomic::Ordering;
        self.done.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let passed = self.passed.load(Ordering::Relaxed) || size_of(&self.folder) > self.bound;
        // Past the bound the proof stops, whichever look saw it.
        if passed {
            self.cancel.cancel();
        }
        passed
    }
}

/// A watch left without `stop`, on an early return, still ends its thread.
impl Drop for SizeWatch {
    fn drop(&mut self) {
        self.done.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The bytes the files under `folder` hold, never following a link.
fn size_of(folder: &Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![folder.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).into_iter().flatten().flatten() {
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => pending.push(entry.path()),
                Ok(kind) if kind.is_file() => total += entry.metadata().map_or(0, |metadata| metadata.len()),
                _ => {}
            }
        }
    }
    total
}

/// Removes clone folders that a Horizon which stopped mid-proof left behind.
pub(super) fn sweep(parent: &Path) {
    for entry in std::fs::read_dir(parent).into_iter().flatten().flatten() {
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > STALE);
        let ours = entry.file_name().to_string_lossy().starts_with("crates-");
        if old && ours && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}
