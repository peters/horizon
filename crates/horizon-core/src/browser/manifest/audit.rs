//! Private, bounded browser action journals.

use std::fs::OpenOptions;
use std::io::{BufRead, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use horizon_browser::BrowserAuditEntry;

use super::ManifestLock;
use crate::horizon_home::{HorizonHome, safe_local_id};

const MAX_AUDIT_SEGMENT_BYTES: u64 = 8 * 1024 * 1024;

#[must_use]
pub fn audit_path_for_root(root: &Path, panel_local_id: &str) -> PathBuf {
    root.join("audit")
        .join("browsers")
        .join(format!("{}.jsonl", safe_local_id(panel_local_id)))
}

#[must_use]
pub fn default_audit_path(panel_local_id: &str) -> PathBuf {
    HorizonHome::resolve()
        .browser_audit_dir()
        .join(format!("{}.jsonl", safe_local_id(panel_local_id)))
}

pub(super) fn append(entry: &BrowserAuditEntry, panel_local_id: &str) -> std::io::Result<()> {
    append_at(&default_audit_path(panel_local_id), entry)
}

pub(super) fn append_at_path(path: &Path, entry: &BrowserAuditEntry) -> std::io::Result<()> {
    append_at(path, entry)
}

fn append_at(path: &Path, entry: &BrowserAuditEntry) -> std::io::Result<()> {
    append_at_with_limit(path, entry, MAX_AUDIT_SEGMENT_BYTES)
}

fn append_at_with_limit(path: &Path, entry: &BrowserAuditEntry, max_segment_bytes: u64) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _lock = ManifestLock::acquire(path)?;
    let mut file = open_private_append(path)?;
    write_entry(&mut file, path, entry, max_segment_bytes)
}

#[derive(Debug, Default)]
pub(super) struct AuditSink {
    writer: Mutex<Option<AuditWriter>>,
}

impl AuditSink {
    pub(super) fn append(&self, entry: &BrowserAuditEntry, panel_local_id: &str) -> std::io::Result<()> {
        let path = default_audit_path(panel_local_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _lock = ManifestLock::acquire(&path)?;
        let mut cached = self.writer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if cached.as_ref().is_none_or(|writer| writer.path != path) {
            *cached = Some(AuditWriter {
                file: open_private_append(&path)?,
                path,
            });
        }
        let result = cached.as_mut().map_or_else(
            || Err(std::io::Error::other("audit writer unavailable")),
            |writer| write_entry(&mut writer.file, &writer.path, entry, MAX_AUDIT_SEGMENT_BYTES),
        );
        if result.is_err() {
            cached.take();
        }
        result
    }
}

#[derive(Debug)]
struct AuditWriter {
    path: PathBuf,
    file: std::fs::File,
}

fn open_private_append(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        let mut permissions = file.metadata()?.permissions();
        permissions.set_mode(0o600);
        file.set_permissions(permissions)?;
    }
    Ok(file)
}

fn write_entry(
    file: &mut std::fs::File,
    path: &Path,
    entry: &BrowserAuditEntry,
    max_segment_bytes: u64,
) -> std::io::Result<()> {
    let mut encoded = serde_json::to_vec(entry).map_err(std::io::Error::other)?;
    encoded.push(b'\n');
    let encoded_len = u64::try_from(encoded.len()).unwrap_or(u64::MAX);
    let current_len = file.metadata()?.len();
    if current_len > 0 && current_len.saturating_add(encoded_len) > max_segment_bytes {
        copy_private(path, &rotated_path(path))?;
        file.set_len(0)?;
    }
    file.write_all(&encoded)
}

fn copy_private(source_path: &Path, destination_path: &Path) -> std::io::Result<()> {
    let mut source = std::fs::File::open(source_path)?;
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut destination = options.open(destination_path)?;
    #[cfg(unix)]
    destination.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    std::io::copy(&mut source, &mut destination)?;
    destination.flush()
}

fn rotated_path(path: &Path) -> PathBuf {
    let mut rotated = path.as_os_str().to_os_string();
    rotated.push(".1");
    PathBuf::from(rotated)
}

/// Read the ordered action journal for one panel identity.
///
/// # Errors
/// Returns an I/O or invalid-data error when a persisted journal cannot be
/// read as complete JSONL records.
pub fn read_audit(panel_local_id: &str) -> std::io::Result<Vec<BrowserAuditEntry>> {
    read_at(&default_audit_path(panel_local_id))
}

pub(super) fn read_at(path: &Path) -> std::io::Result<Vec<BrowserAuditEntry>> {
    // Writers serialize one complete JSONL record while holding this same
    // inter-process lock. Taking it for the read prevents a live auditor from
    // mistaking an in-progress final append for journal corruption.
    let rotated = rotated_path(path);
    if !path.exists() && !rotated.exists() {
        return Ok(Vec::new());
    }
    let _lock = ManifestLock::acquire(path)?;
    let mut entries = Vec::new();
    read_segment(&rotated, &mut entries)?;
    read_segment(path, &mut entries)?;
    Ok(entries)
}

fn read_segment(path: &Path, entries: &mut Vec<BrowserAuditEntry>) -> std::io::Result<()> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for line in std::io::BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        entries.push(
            serde_json::from_str(&line).map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_browser::{
        BrowserAuditAction, BrowserAuditActor, BrowserAuditEntry, BrowserAuditStatus, new_action_id,
    };

    #[test]
    fn audit_is_append_only_private_jsonl() {
        let root = std::env::temp_dir().join(format!("horizon-audit-{}", std::process::id()));
        let path = audit_path_for_root(&root, "panel/unsafe");
        let first = BrowserAuditEntry::new(
            new_action_id(),
            BrowserAuditActor::User,
            BrowserAuditStatus::Dispatched,
            BrowserAuditAction::Reload,
        );
        let second = BrowserAuditEntry::new(
            new_action_id(),
            BrowserAuditActor::System,
            BrowserAuditStatus::Dispatched,
            BrowserAuditAction::Stop,
        );

        append_at(&path, &first).unwrap();
        append_at(&path, &second).unwrap();

        assert_eq!(read_at(&path).unwrap(), [first, second]);
        assert!(path.starts_with(&root));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn audit_rotation_bounds_storage_and_keeps_the_newest_segments() {
        let root = std::env::temp_dir().join(format!("horizon-audit-rotation-{}", std::process::id()));
        let path = audit_path_for_root(&root, "panel");
        let entries = [
            BrowserAuditAction::Back,
            BrowserAuditAction::Forward,
            BrowserAuditAction::Reload,
        ]
        .map(|action| {
            BrowserAuditEntry::new(
                new_action_id(),
                BrowserAuditActor::User,
                BrowserAuditStatus::Dispatched,
                action,
            )
        });

        for entry in &entries {
            append_at_with_limit(&path, entry, 1).unwrap();
        }

        assert_eq!(read_at(&path).unwrap(), entries[1..]);
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
        assert!(std::fs::metadata(rotated_path(&path)).unwrap().len() > 0);
        let _ = std::fs::remove_dir_all(root);
    }
}
