//! Process-scoped leases for Horizon-owned skill directories.
//!
//! Coordination is per target `skills` directory so `CODEX_HOME` / `GROK_HOME`
//! overrides share a lock even when `HOME` differs across Horizon processes.

use std::ffi::{OsStr, OsString};
use std::fs::{OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

pub(super) const HORIZON_NOTIFY_SKILL: &str = "horizon-notify";
pub(super) const HORIZON_BROWSER_SKILL: &str = "horizon-browser";
const LEASES_DIR: &str = ".horizon-leases";

pub(super) struct SkillRootLease {
    parent: PathBuf,
    leased_names: Vec<OsString>,
    cleanup_names: Vec<OsString>,
    live_path: PathBuf,
    live_lock: Option<std::fs::File>,
}

impl SkillRootLease {
    pub(super) fn covers_skill_dir(&self, skill_dir: &Path) -> bool {
        skill_dir.parent().is_some_and(|parent| parent == self.parent)
            && skill_dir
                .file_name()
                .is_some_and(|name| self.leased_names.iter().any(|leased| leased == name))
    }
}

struct SkillRootSpec {
    parent: PathBuf,
    leased_names: Vec<OsString>,
    cleanup_names: Vec<OsString>,
}

pub(super) fn bind_skill_roots(host_id: &OsStr, dirs: &[PathBuf], extra_cleanup: &[PathBuf]) -> Vec<SkillRootLease> {
    let mut leases = Vec::new();
    for spec in group_skill_roots(dirs, extra_cleanup) {
        let parent = spec.parent.clone();
        match acquire_skill_root(host_id, spec) {
            Ok(lease) => leases.push(lease),
            Err(error) => {
                tracing::warn!(path = %parent.display(), %error, "failed to lease Horizon skill root");
            }
        }
    }
    leases
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
                for name in &lease.cleanup_names {
                    remove_horizon_skill_dir(&lease.parent.join(name));
                }
                // Keep `.horizon-leases` and `.lock`. Unlinking the directory
                // while this lock is held lets a starter block on the old inode,
                // then fail to create its `.live` marker in the gone directory.
            }
            Err(error) => {
                tracing::warn!(path = %leases_dir.display(), %error, "failed to inspect skill root leases");
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

fn group_skill_roots(dirs: &[PathBuf], extra_cleanup: &[PathBuf]) -> Vec<SkillRootSpec> {
    let mut groups: Vec<SkillRootSpec> = Vec::new();
    for dir in dirs {
        let Some(parent) = dir.parent() else {
            continue;
        };
        let Some(name) = dir.file_name() else {
            continue;
        };
        match groups.iter_mut().find(|existing| existing.parent == parent) {
            Some(existing) => {
                push_unique_name(&mut existing.leased_names, name);
                push_unique_name(&mut existing.cleanup_names, name);
            }
            None => groups.push(SkillRootSpec {
                parent: parent.to_path_buf(),
                leased_names: vec![name.to_os_string()],
                cleanup_names: vec![name.to_os_string()],
            }),
        }
    }
    for extra in extra_cleanup {
        let Some(parent) = extra.parent() else {
            continue;
        };
        let Some(name) = extra.file_name() else {
            continue;
        };
        if let Some(existing) = groups.iter_mut().find(|existing| existing.parent == parent) {
            push_unique_name(&mut existing.cleanup_names, name);
        }
    }
    groups
}

fn push_unique_name(names: &mut Vec<OsString>, name: &OsStr) {
    if !names.iter().any(|existing| existing == name) {
        names.push(name.to_os_string());
    }
}

fn acquire_skill_root(host_id: &OsStr, spec: SkillRootSpec) -> io::Result<SkillRootLease> {
    let SkillRootSpec {
        parent,
        leased_names,
        cleanup_names,
    } = spec;
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
        leased_names,
        cleanup_names,
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
