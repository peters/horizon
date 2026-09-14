//! Process-scoped leases for Horizon-owned skill directories.
//!
//! Coordination is per target `skills` directory so `CODEX_HOME` / `GROK_HOME`
//! overrides share a lock even when `HOME` differs across Horizon processes.

use std::ffi::OsStr;
use std::fs::{OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

pub(super) const HORIZON_NOTIFY_SKILL: &str = "horizon-notify";
pub(super) const HORIZON_BROWSER_SKILL: &str = "horizon-browser";
const LEASES_DIR: &str = ".horizon-leases";

pub(super) struct SkillRootLease {
    parent: PathBuf,
    live_path: PathBuf,
    live_lock: Option<std::fs::File>,
}

pub(super) fn bind_skill_roots(host_id: &OsStr, dirs: &[PathBuf]) -> io::Result<Vec<SkillRootLease>> {
    let mut leases = Vec::new();
    for parent in unique_parents(dirs) {
        leases.push(acquire_skill_root(host_id, parent)?);
    }
    Ok(leases)
}

pub(super) fn release_skill_roots(leases: &mut [SkillRootLease]) {
    for lease in leases {
        let leases_dir = lease.parent.join(LEASES_DIR);
        let coord = match lock_coord(&leases_dir) {
            Ok(file) => file,
            Err(error) => {
                tracing::warn!(path = %leases_dir.display(), %error, "failed to lock skill root cleanup");
                continue;
            }
        };
        drop(lease.live_lock.take());
        if let Err(error) = std::fs::remove_file(&lease.live_path)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(path = %lease.live_path.display(), %error, "failed to remove skill root live lock");
        }
        match another_live_host(&leases_dir, &lease.live_path) {
            Ok(true) => {}
            Ok(false) => {
                remove_horizon_skill_dir(&lease.parent.join(HORIZON_NOTIFY_SKILL));
                remove_horizon_skill_dir(&lease.parent.join(HORIZON_BROWSER_SKILL));
            }
            Err(error) => {
                tracing::warn!(path = %leases_dir.display(), %error, "failed to inspect skill root leases");
            }
        }
        drop(coord);
    }
}

fn unique_parents(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut parents = Vec::new();
    for dir in dirs {
        let Some(parent) = dir.parent() else {
            continue;
        };
        if !parents.iter().any(|existing| existing == parent) {
            parents.push(parent.to_path_buf());
        }
    }
    parents
}

fn acquire_skill_root(host_id: &OsStr, parent: PathBuf) -> io::Result<SkillRootLease> {
    let leases_dir = parent.join(LEASES_DIR);
    std::fs::create_dir_all(&leases_dir)?;
    let coord = lock_coord(&leases_dir)?;
    let live_path = {
        let mut name = host_id.to_os_string();
        name.push(".live");
        leases_dir.join(name)
    };
    let live_lock = open_lock_file(&live_path)?;
    match live_lock.try_lock() {
        Ok(()) => {}
        Err(error) => {
            drop(coord);
            let _ = std::fs::remove_file(&live_path);
            return Err(match error {
                TryLockError::WouldBlock => io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!("skill root is already leased: {}", parent.display()),
                ),
                TryLockError::Error(error) => error,
            });
        }
    }
    drop(coord);
    Ok(SkillRootLease {
        parent,
        live_path,
        live_lock: Some(live_lock),
    })
}

fn lock_coord(leases_dir: &Path) -> io::Result<std::fs::File> {
    std::fs::create_dir_all(leases_dir)?;
    let file = open_lock_file(&leases_dir.join(".lock"))?;
    file.lock()?;
    Ok(file)
}

fn another_live_host(leases_dir: &Path, current: &Path) -> io::Result<bool> {
    for entry in std::fs::read_dir(leases_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path == current || !is_live_lock(entry.file_name().as_os_str()) {
            continue;
        }
        let file = open_lock_file(&path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(true),
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
    Ok(false)
}

fn is_live_lock(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    !name.starts_with('.')
        && Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("live"))
}

pub(super) fn remove_horizon_skill_dir(path: &Path) {
    let Some(name) = path.file_name() else {
        return;
    };
    if name != HORIZON_NOTIFY_SKILL && name != HORIZON_BROWSER_SKILL {
        return;
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "failed to inspect Horizon skill directory");
            return;
        }
    };
    let result = if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    if let Err(error) = result
        && error.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), %error, "failed to remove Horizon skill directory");
    }
}

fn open_lock_file(path: &Path) -> io::Result<std::fs::File> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
}
