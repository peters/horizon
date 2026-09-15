//! Process-scoped leases for Horizon-owned skill directories.
//!
//! Coordination is per target skill directory so `CODEX_HOME` / `GROK_HOME`
//! overrides share a lock even when `HOME` differs across Horizon processes.
//! Liveness is per skill name so a notify-only host cannot strand a sibling
//! `horizon-browser` directory leased by another host.

use std::ffi::OsStr;
use std::fs::{OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

pub(super) const HORIZON_NOTIFY_SKILL: &str = "horizon-notify";
pub(super) const HORIZON_BROWSER_SKILL: &str = "horizon-browser";
pub(super) const HORIZON_OFFLOAD_SKILL: &str = "horizon-offload";
const LEASES_DIR: &str = ".horizon-leases";

pub(super) struct SkillRootLease {
    skill_dir: PathBuf,
    install: bool,
    live_path: PathBuf,
    live_lock: Option<std::fs::File>,
}

impl SkillRootLease {
    pub(super) fn covers_skill_dir(&self, skill_dir: &Path) -> bool {
        self.install && self.skill_dir == skill_dir
    }
}

pub(super) fn bind_skill_roots(host_id: &OsStr, dirs: &[PathBuf], extra_cleanup: &[PathBuf]) -> Vec<SkillRootLease> {
    let mut leases = Vec::new();
    let mut claimed = Vec::new();
    for dir in dirs {
        if !claim_dir(&mut claimed, dir) {
            continue;
        }
        push_acquired(&mut leases, host_id, dir, true);
    }
    for dir in extra_cleanup {
        if !claim_dir(&mut claimed, dir) {
            continue;
        }
        push_acquired(&mut leases, host_id, dir, false);
    }
    leases
}

pub(super) fn release_skill_roots(leases: &mut [SkillRootLease]) {
    for lease in leases {
        let Some(coord_dir) = skill_coord_dir(&lease.skill_dir) else {
            continue;
        };
        let coord = match lock_coord(&coord_dir) {
            Ok(file) => file,
            Err(error) => {
                tracing::warn!(path = %coord_dir.display(), %error, "failed to lock skill root cleanup");
                continue;
            }
        };
        drop(lease.live_lock.take());
        if let Err(error) = std::fs::remove_file(&lease.live_path)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(path = %lease.live_path.display(), %error, "failed to remove skill root live lock");
        }
        let live_dir = lease.live_path.parent().unwrap_or(coord_dir.as_path());
        match another_live_host(live_dir, &lease.live_path) {
            Ok(true) => {}
            Ok(false) => {
                remove_horizon_skill_dir(&lease.skill_dir);
                // Keep `.horizon-leases` and `.lock`. Unlinking the directory
                // while this lock is held lets a starter block on the old inode,
                // then fail to create its `.live` marker in the gone directory.
            }
            Err(error) => {
                tracing::warn!(path = %live_dir.display(), %error, "failed to inspect skill root leases");
            }
        }
        drop(coord);
    }
}

impl Drop for SkillRootLease {
    fn drop(&mut self) {
        drop(self.live_lock.take());
        if let Err(error) = std::fs::remove_file(&self.live_path)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(path = %self.live_path.display(), %error, "failed to drop skill root live lock");
        }
    }
}

fn claim_dir(claimed: &mut Vec<PathBuf>, dir: &Path) -> bool {
    if claimed.iter().any(|existing| existing == dir) {
        return false;
    }
    claimed.push(dir.to_path_buf());
    true
}

fn push_acquired(leases: &mut Vec<SkillRootLease>, host_id: &OsStr, skill_dir: &Path, install: bool) {
    match acquire_skill_root(host_id, skill_dir.to_path_buf(), install) {
        Ok(lease) => leases.push(lease),
        Err(error) => {
            tracing::warn!(path = %skill_dir.display(), %error, "failed to lease Horizon skill root");
        }
    }
}

fn acquire_skill_root(host_id: &OsStr, skill_dir: PathBuf, install: bool) -> io::Result<SkillRootLease> {
    let coord_dir = skill_coord_dir(&skill_dir).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("skill directory has no parent: {}", skill_dir.display()),
        )
    })?;
    let live_dir = skill_live_dir(&skill_dir).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("skill directory has no name: {}", skill_dir.display()),
        )
    })?;
    std::fs::create_dir_all(&live_dir)?;
    let coord = lock_coord(&coord_dir)?;
    let live_path = {
        let mut name = host_id.to_os_string();
        name.push(".live");
        live_dir.join(name)
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
                    format!("skill root is already leased: {}", skill_dir.display()),
                ),
                TryLockError::Error(error) => error,
            });
        }
    }
    drop(coord);
    Ok(SkillRootLease {
        skill_dir,
        install,
        live_path,
        live_lock: Some(live_lock),
    })
}

fn skill_coord_dir(skill_dir: &Path) -> Option<PathBuf> {
    skill_dir.parent().map(|parent| parent.join(LEASES_DIR))
}

fn skill_live_dir(skill_dir: &Path) -> Option<PathBuf> {
    let name = skill_dir.file_name()?;
    Some(skill_coord_dir(skill_dir)?.join(name))
}

fn lock_coord(leases_dir: &Path) -> io::Result<std::fs::File> {
    std::fs::create_dir_all(leases_dir)?;
    let file = open_lock_file(&leases_dir.join(".lock"))?;
    file.lock()?;
    Ok(file)
}

fn another_live_host(live_dir: &Path, current: &Path) -> io::Result<bool> {
    let entries = match std::fs::read_dir(live_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path == current || !is_live_lock(entry.file_name().as_os_str()) {
            continue;
        }
        let file = open_lock_file(&path)?;
        match file.try_lock() {
            Ok(()) => {
                drop(file);
                let _ = std::fs::remove_file(&path);
            }
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
    if name != HORIZON_NOTIFY_SKILL && name != HORIZON_BROWSER_SKILL && name != HORIZON_OFFLOAD_SKILL {
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
