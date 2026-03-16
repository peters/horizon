use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use crate::git_status::{GitStatus, compute_git_status};

const POLL_INTERVAL: Duration = Duration::from_secs(2);

pub struct GitWatcher {
    receiver: mpsc::Receiver<Arc<GitStatus>>,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl GitWatcher {
    /// Start a background git watcher for the given repo path.
    /// Polls `.git/index` mtime every ~2 seconds.
    #[must_use]
    pub fn start(repo_path: PathBuf) -> Self {
        let (sender, receiver) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_flag = Arc::clone(&shutdown);

        let thread = thread::Builder::new()
            .name(format!("git-watcher-{}", short_path(&repo_path)))
            .spawn(move || watcher_loop(&repo_path, &sender, &shutdown_flag))
            .ok();

        Self {
            receiver,
            shutdown,
            thread,
        }
    }

    /// Non-blocking receive. Returns the latest status if available.
    #[must_use]
    pub fn try_recv(&self) -> Option<Arc<GitStatus>> {
        let mut latest = None;
        // Drain to get the most recent status.
        while let Ok(status) = self.receiver.try_recv() {
            latest = Some(status);
        }
        latest
    }

    /// Signal the watcher thread to stop and wait for it to finish.
    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for GitWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

fn watcher_loop(repo_path: &Path, sender: &mpsc::Sender<Arc<GitStatus>>, shutdown: &AtomicBool) {
    let index_path = resolve_git_index_path(repo_path);
    let mut last_mtime: Option<SystemTime> = None;

    // Always do an initial scan.
    if let Some(status) = try_compute_status(repo_path) {
        last_mtime = index_path.as_deref().and_then(file_mtime);
        let _ = sender.send(Arc::new(status));
    }

    loop {
        thread::sleep(POLL_INTERVAL);

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        let current_mtime = index_path.as_deref().and_then(file_mtime);

        // The index mtime changes on every `git add` or `git commit`.
        let changed = match (last_mtime, current_mtime) {
            (Some(prev), Some(curr)) => prev != curr,
            (None, Some(_)) => true,
            _ => false,
        };

        if !changed {
            continue;
        }

        if let Some(status) = try_compute_status(repo_path) {
            last_mtime = current_mtime;
            if sender.send(Arc::new(status)).is_err() {
                break;
            }
        }
    }
}

fn try_compute_status(repo_path: &Path) -> Option<GitStatus> {
    match compute_git_status(repo_path) {
        Ok(status) => Some(status),
        Err(error) => {
            tracing::warn!(path = %repo_path.display(), %error, "git status failed");
            None
        }
    }
}

fn resolve_git_index_path(repo_path: &Path) -> Option<PathBuf> {
    if let Ok(repo) = git2::Repository::discover(repo_path) {
        Some(repo.path().join("index"))
    } else {
        let candidate = repo_path.join(".git/index");
        candidate.exists().then_some(candidate)
    }
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn short_path(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| "unknown".to_string(), |n| n.to_string_lossy().to_string())
}
