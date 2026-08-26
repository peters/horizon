//! Private append-only browser action journals.

use std::fs::OpenOptions;
use std::io::{BufRead, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use horizon_browser::BrowserAuditEntry;

use super::ManifestLock;
use crate::horizon_home::{HorizonHome, safe_local_id};

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

fn append_at(path: &Path, entry: &BrowserAuditEntry) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _lock = ManifestLock::acquire(path)?;
    let mut file = open_private_append(path)?;
    write_entry(&mut file, entry)
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
            |writer| write_entry(&mut writer.file, entry),
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
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = file.metadata()?.permissions();
        permissions.set_mode(0o600);
        file.set_permissions(permissions)?;
    }
    Ok(file)
}

fn write_entry(file: &mut std::fs::File, entry: &BrowserAuditEntry) -> std::io::Result<()> {
    let mut encoded = serde_json::to_vec(entry).map_err(std::io::Error::other)?;
    encoded.push(b'\n');
    file.write_all(&encoded)
}

/// Read the ordered action journal for one panel identity.
///
/// # Errors
/// Returns an I/O or invalid-data error when a persisted journal cannot be
/// read as complete JSONL records.
pub fn read_audit(panel_local_id: &str) -> std::io::Result<Vec<BrowserAuditEntry>> {
    read_at(&default_audit_path(panel_local_id))
}

fn read_at(path: &Path) -> std::io::Result<Vec<BrowserAuditEntry>> {
    // Writers serialize one complete JSONL record while holding this same
    // inter-process lock. Taking it for the read prevents a live auditor from
    // mistaking an in-progress final append for journal corruption.
    if !path.exists() {
        return Ok(Vec::new());
    }
    let _lock = ManifestLock::acquire(path)?;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut entries = Vec::new();
    for line in std::io::BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        entries.push(
            serde_json::from_str(&line).map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?,
        );
    }
    Ok(entries)
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
}
