//! Private, bounded browser action journals.

use std::fs::OpenOptions;
use std::io::{BufRead, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use horizon_browser::BrowserAuditEntry;
use serde::{Deserialize, Serialize};

use super::ManifestLock;
use crate::horizon_home::{HorizonHome, safe_local_id};

const MAX_AUDIT_SEGMENT_BYTES: u64 = 8 * 1024 * 1024;
/// Default `browser_audit` page size when the caller omits `limit`.
pub const DEFAULT_AUDIT_PAGE_LIMIT: usize = 100;
/// Maximum `browser_audit` page size. Larger journals are read with a cursor.
pub const MAX_AUDIT_PAGE_LIMIT: usize = 500;

/// Retained audit records plus loss counters for one panel journal.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuditJournal {
    /// Ordered retained entries, oldest first, across the live and rotated segments.
    pub entries: Vec<BrowserAuditEntry>,
    /// JSONL lines that could not be decoded and were skipped.
    pub malformed_records: u64,
    /// Valid records overwritten out of the rotated segment and no longer retained.
    pub older_records_dropped: u64,
}

/// Bounded read of a retained audit journal.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuditPageRequest {
    /// Exclusive resume cursor. When set, `from_start` is ignored.
    pub after_event_id: Option<String>,
    /// When true and `after_event_id` is absent, start at the oldest retained match.
    pub from_start: bool,
    /// Optional action-id filter applied before paging.
    pub action_id: Option<String>,
    /// Page size after clamping to `1..=MAX_AUDIT_PAGE_LIMIT`.
    pub limit: usize,
}

/// One bounded page of retained audit records plus pagination and loss metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditPage {
    /// Matching records in this page, oldest first.
    pub entries: Vec<BrowserAuditEntry>,
    /// `event_id` of the last returned record; reuse as `after_event_id`.
    pub next_event_id: Option<String>,
    /// True when newer matching retained records exist after this page.
    pub has_more: bool,
    /// Number of records in `entries`.
    pub records_returned: u64,
    /// Matching retained records across every kept segment.
    pub records_retained: u64,
    /// Journal-level malformed JSONL lines.
    pub malformed_records: u64,
    /// Journal-level records rotated out of retention.
    pub older_records_dropped: u64,
    /// True when `after_event_id` was not found in the retained matching records.
    pub cursor_lost: bool,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct AuditDropMeta {
    older_records_dropped: u64,
}

impl AuditPageRequest {
    /// Build a page request, dropping empty identifiers and clamping `limit`.
    #[must_use]
    pub fn new(
        after_event_id: Option<String>,
        from_start: bool,
        action_id: Option<String>,
        limit: Option<usize>,
    ) -> Self {
        Self {
            after_event_id: nonempty(after_event_id),
            from_start,
            action_id: nonempty(action_id),
            limit: limit.unwrap_or(DEFAULT_AUDIT_PAGE_LIMIT).clamp(1, MAX_AUDIT_PAGE_LIMIT),
        }
    }
}

/// Select one bounded page from a retained journal.
#[must_use]
pub fn page_audit(journal: &AuditJournal, request: &AuditPageRequest) -> AuditPage {
    let limit = request.limit.clamp(1, MAX_AUDIT_PAGE_LIMIT);
    let matching: Vec<&BrowserAuditEntry> = journal
        .entries
        .iter()
        .filter(|entry| {
            request
                .action_id
                .as_ref()
                .is_none_or(|action_id| entry.action_id == *action_id)
        })
        .collect();
    let records_retained = count(matching.len());
    let (start, cursor_lost) = page_start(&matching, request, limit);
    let has_more = start.saturating_add(limit) < matching.len();
    let entries: Vec<BrowserAuditEntry> = matching.into_iter().skip(start).take(limit).cloned().collect();
    AuditPage {
        next_event_id: entries.last().map(|entry| entry.event_id.clone()),
        has_more,
        records_returned: count(entries.len()),
        records_retained,
        malformed_records: journal.malformed_records,
        older_records_dropped: journal.older_records_dropped,
        cursor_lost,
        entries,
    }
}

fn page_start(matching: &[&BrowserAuditEntry], request: &AuditPageRequest, limit: usize) -> (usize, bool) {
    if let Some(after) = request.after_event_id.as_deref() {
        return match matching.iter().position(|entry| entry.event_id == after) {
            Some(index) => (index.saturating_add(1), false),
            None => (0, true),
        };
    }
    if request.from_start {
        (0, false)
    } else {
        (matching.len().saturating_sub(limit), false)
    }
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

fn count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

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
        record_rotated_out(path)?;
        copy_private(path, &rotated_path(path))?;
        file.set_len(0)?;
    }
    file.write_all(&encoded)
}

fn record_rotated_out(path: &Path) -> std::io::Result<()> {
    let rotated = rotated_path(path);
    if !rotated.exists() {
        return Ok(());
    }
    let dropped = count_records(&rotated)?;
    if dropped == 0 {
        return Ok(());
    }
    let meta_path = drop_meta_path(path);
    let meta = AuditDropMeta {
        older_records_dropped: read_drop_meta(&meta_path)?.saturating_add(dropped),
    };
    write_drop_meta(&meta_path, &meta)
}

fn count_records(path: &Path) -> std::io::Result<u64> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut records = 0_u64;
    for line in std::io::BufReader::new(file).lines() {
        if !line?.trim().is_empty() {
            records = records.saturating_add(1);
        }
    }
    Ok(records)
}

fn drop_meta_path(path: &Path) -> PathBuf {
    let mut meta = path.as_os_str().to_os_string();
    meta.push(".meta");
    PathBuf::from(meta)
}

fn read_drop_meta(path: &Path) -> std::io::Result<u64> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    if bytes.is_empty() {
        return Ok(0);
    }
    let meta: AuditDropMeta =
        serde_json::from_slice(&bytes).map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    Ok(meta.older_records_dropped)
}

fn write_drop_meta(path: &Path, meta: &AuditDropMeta) -> std::io::Result<()> {
    let mut encoded = serde_json::to_vec(meta).map_err(std::io::Error::other)?;
    encoded.push(b'\n');
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path)?;
    #[cfg(unix)]
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(&encoded)?;
    file.flush()
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
/// Malformed JSONL lines are skipped; use [`read_audit_journal`] when loss
/// counters are required.
///
/// # Errors
/// Returns an I/O or invalid-data error when a persisted journal or its drop
/// metadata cannot be read.
pub fn read_audit(panel_local_id: &str) -> std::io::Result<Vec<BrowserAuditEntry>> {
    read_at(&default_audit_path(panel_local_id))
}

/// Read retained audit records and loss counters for one panel identity.
///
/// # Errors
/// Returns an I/O or invalid-data error when a persisted journal or its drop
/// metadata cannot be read.
pub fn read_audit_journal(panel_local_id: &str) -> std::io::Result<AuditJournal> {
    read_journal_at(&default_audit_path(panel_local_id))
}

pub(super) fn read_at(path: &Path) -> std::io::Result<Vec<BrowserAuditEntry>> {
    Ok(read_journal_at(path)?.entries)
}

pub(super) fn read_journal_at(path: &Path) -> std::io::Result<AuditJournal> {
    // Writers serialize one complete JSONL record while holding this same
    // inter-process lock. Taking it for the read prevents a live auditor from
    // mistaking an in-progress final append for journal corruption.
    let rotated = rotated_path(path);
    let meta_path = drop_meta_path(path);
    if !path.exists() && !rotated.exists() && !meta_path.exists() {
        return Ok(AuditJournal::default());
    }
    let _lock = ManifestLock::acquire(path)?;
    let mut journal = AuditJournal {
        older_records_dropped: read_drop_meta(&meta_path)?,
        ..AuditJournal::default()
    };
    read_segment(&rotated, &mut journal)?;
    read_segment(path, &mut journal)?;
    Ok(journal)
}

fn read_segment(path: &Path, journal: &mut AuditJournal) -> std::io::Result<()> {
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
        match serde_json::from_str(&line) {
            Ok(entry) => journal.entries.push(entry),
            Err(_) => journal.malformed_records = journal.malformed_records.saturating_add(1),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_browser::{
        BrowserAuditAction, BrowserAuditActor, BrowserAuditEntry, BrowserAuditStatus, new_action_id,
    };

    fn entry(event_id: &str, action_id: &str) -> BrowserAuditEntry {
        BrowserAuditEntry {
            schema_version: 1,
            event_id: event_id.to_string(),
            action_id: action_id.to_string(),
            at_millis: 0,
            actor: BrowserAuditActor::User,
            status: BrowserAuditStatus::Dispatched,
            action: BrowserAuditAction::Reload,
        }
    }

    fn journal(entries: Vec<BrowserAuditEntry>) -> AuditJournal {
        AuditJournal {
            entries,
            ..AuditJournal::default()
        }
    }

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

        let retained = read_journal_at(&path).unwrap();
        assert_eq!(retained.entries, entries[1..]);
        assert_eq!(retained.older_records_dropped, 1);
        assert_eq!(retained.malformed_records, 0);
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
        assert!(std::fs::metadata(rotated_path(&path)).unwrap().len() > 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn journal_skips_malformed_lines_and_counts_them() {
        let root = tempfile::tempdir().unwrap();
        let path = audit_path_for_root(root.path(), "panel");
        let valid = entry("event-2", "action-a");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!("{{not json}}\n{}\n\n", serde_json::to_string(&valid).unwrap()),
        )
        .unwrap();

        let journal = read_journal_at(&path).unwrap();
        assert_eq!(journal.entries, [valid]);
        assert_eq!(journal.malformed_records, 1);
    }

    #[test]
    fn page_returns_newest_matching_window_by_default() {
        let journal = journal(vec![
            entry("e1", "a"),
            entry("e2", "a"),
            entry("e3", "b"),
            entry("e4", "a"),
        ]);
        let page = page_audit(&journal, &AuditPageRequest::new(None, false, None, Some(2)));
        assert_eq!(
            page.entries
                .iter()
                .map(|entry| entry.event_id.as_str())
                .collect::<Vec<_>>(),
            ["e3", "e4"]
        );
        assert_eq!(page.next_event_id.as_deref(), Some("e4"));
        assert!(!page.has_more);
        assert_eq!(page.records_returned, 2);
        assert_eq!(page.records_retained, 4);
        assert!(!page.cursor_lost);
    }

    #[test]
    fn page_from_start_walks_every_retained_record_with_the_cursor() {
        let journal = journal((1..=5).map(|index| entry(&format!("e{index}"), "a")).collect());
        let first = page_audit(&journal, &AuditPageRequest::new(None, true, None, Some(2)));
        assert_eq!(
            first
                .entries
                .iter()
                .map(|entry| entry.event_id.as_str())
                .collect::<Vec<_>>(),
            ["e1", "e2"]
        );
        assert!(first.has_more);
        assert_eq!(first.next_event_id.as_deref(), Some("e2"));

        let second = page_audit(
            &journal,
            &AuditPageRequest::new(first.next_event_id.clone(), false, None, Some(2)),
        );
        assert_eq!(
            second
                .entries
                .iter()
                .map(|entry| entry.event_id.as_str())
                .collect::<Vec<_>>(),
            ["e3", "e4"]
        );
        assert!(second.has_more);

        let third = page_audit(
            &journal,
            &AuditPageRequest::new(second.next_event_id.clone(), true, None, Some(2)),
        );
        assert_eq!(
            third
                .entries
                .iter()
                .map(|entry| entry.event_id.as_str())
                .collect::<Vec<_>>(),
            ["e5"]
        );
        assert!(!third.has_more);
        assert!(!third.cursor_lost);
        assert_eq!(third.records_retained, 5);
    }

    #[test]
    fn page_filters_by_action_id_before_paging() {
        let journal = journal(vec![
            entry("e1", "keep"),
            entry("e2", "drop"),
            entry("e3", "keep"),
            entry("e4", "keep"),
        ]);
        let page = page_audit(
            &journal,
            &AuditPageRequest::new(None, true, Some("keep".to_string()), Some(2)),
        );
        assert_eq!(
            page.entries
                .iter()
                .map(|entry| entry.event_id.as_str())
                .collect::<Vec<_>>(),
            ["e1", "e3"]
        );
        assert!(page.has_more);
        assert_eq!(page.records_retained, 3);
    }

    #[test]
    fn page_reports_cursor_lost_and_continues_from_retained_records() {
        let journal = AuditJournal {
            entries: vec![entry("e3", "a"), entry("e4", "a")],
            older_records_dropped: 2,
            malformed_records: 1,
        };
        let page = page_audit(
            &journal,
            &AuditPageRequest::new(Some("e1".to_string()), false, None, Some(10)),
        );
        assert!(page.cursor_lost);
        assert_eq!(
            page.entries
                .iter()
                .map(|entry| entry.event_id.as_str())
                .collect::<Vec<_>>(),
            ["e3", "e4"]
        );
        assert!(!page.has_more);
        assert_eq!(page.older_records_dropped, 2);
        assert_eq!(page.malformed_records, 1);
    }

    #[test]
    fn page_request_clamps_limit_and_ignores_empty_identifiers() {
        let request = AuditPageRequest::new(Some(String::new()), true, Some(String::new()), Some(0));
        assert_eq!(request.after_event_id, None);
        assert_eq!(request.action_id, None);
        assert_eq!(request.limit, 1);
        let wide = AuditPageRequest::new(None, false, None, Some(10_000));
        assert_eq!(wide.limit, MAX_AUDIT_PAGE_LIMIT);
    }
}
