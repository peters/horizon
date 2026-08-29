//! Horizon-owned retention for explicit browser network exports.

use std::fs::DirEntry;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::horizon_home::safe_local_id;

const MAX_RETAINED_CAPTURE_FILES: usize = 64;
const MAX_RETAINED_CAPTURE_BYTES: u64 = 1024 * 1024 * 1024;
const CAPTURE_RETENTION: Duration = Duration::from_hours(168);

struct CaptureFile {
    path: PathBuf,
    modified: SystemTime,
    bytes: u64,
}

pub(super) fn prepare(panel_local_id: &str, directory: &Path, requested_max_file_bytes: u64) -> std::io::Result<()> {
    validate_panel_capture_directory(panel_local_id, directory)?;
    if requested_max_file_bytes > MAX_RETAINED_CAPTURE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "requested browser capture exceeds Horizon's aggregate retention limit",
        ));
    }
    std::fs::create_dir_all(directory)?;
    let stale_before = SystemTime::now()
        .checked_sub(CAPTURE_RETENTION)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    prune_at(
        directory,
        requested_max_file_bytes,
        MAX_RETAINED_CAPTURE_FILES,
        MAX_RETAINED_CAPTURE_BYTES,
        stale_before,
    )
}

fn validate_panel_capture_directory(panel_local_id: &str, directory: &Path) -> std::io::Result<()> {
    let expected_panel = safe_local_id(panel_local_id);
    let is_panel_capture_directory = directory.file_name().is_some_and(|name| name == "captures")
        && directory
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == expected_panel.as_str());
    if is_panel_capture_directory {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "browser capture directory is outside the panel's persistent profile",
        ))
    }
}

fn prune_at(
    directory: &Path,
    reserved_bytes: u64,
    max_files: usize,
    max_total_bytes: u64,
    stale_before: SystemTime,
) -> std::io::Result<()> {
    let max_existing_files = max_files.saturating_sub(1);
    let max_existing_bytes = max_total_bytes.saturating_sub(reserved_bytes);
    let mut retained = Vec::with_capacity(max_existing_files);
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let Some(candidate) = capture_file(&entry)? else {
            continue;
        };
        if candidate.modified < stale_before {
            remove_file(&candidate.path)?;
            continue;
        }
        retained.push(candidate);
        if retained.len() > max_existing_files
            && let Some(oldest) = oldest_index(&retained)
        {
            let removed = retained.swap_remove(oldest);
            remove_file(&removed.path)?;
        }
    }

    retained.sort_by(capture_age_order);
    let mut retained_bytes = retained
        .iter()
        .fold(0u64, |total, capture| total.saturating_add(capture.bytes));
    for capture in retained {
        if retained_bytes <= max_existing_bytes {
            break;
        }
        remove_file(&capture.path)?;
        retained_bytes = retained_bytes.saturating_sub(capture.bytes);
    }
    Ok(())
}

fn capture_file(entry: &DirEntry) -> std::io::Result<Option<CaptureFile>> {
    if !entry.file_type()?.is_file() {
        return Ok(None);
    }
    let path = entry.path();
    if path.extension().and_then(|extension| extension.to_str()) != Some("ndjson") {
        return Ok(None);
    }
    let metadata = entry.metadata()?;
    Ok(Some(CaptureFile {
        path,
        modified: metadata.modified()?,
        bytes: metadata.len(),
    }))
}

fn oldest_index(captures: &[CaptureFile]) -> Option<usize> {
    captures
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| capture_age_order(left, right))
        .map(|(index, _)| index)
}

fn capture_age_order(left: &CaptureFile, right: &CaptureFile) -> std::cmp::Ordering {
    left.modified
        .cmp(&right.modified)
        .then_with(|| left.path.cmp(&right.path))
}

fn remove_file(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_reserves_the_next_capture_by_count_and_total_bytes() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let paths = ["first.ndjson", "second.ndjson", "third.ndjson"].map(|name| root.path().join(name));
        for (path, bytes) in paths.iter().zip([5usize, 6, 7]) {
            std::fs::write(path, vec![b'x'; bytes])
                .unwrap_or_else(|error| panic!("capture fixture write failed: {error}"));
        }

        prune_at(root.path(), 7, 3, 20, SystemTime::UNIX_EPOCH)
            .unwrap_or_else(|error| panic!("capture retention failed: {error}"));

        let retained = std::fs::read_dir(root.path())
            .unwrap_or_else(|error| panic!("capture directory read failed: {error}"))
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        assert!(retained.len() <= 2);
        assert!(
            retained
                .iter()
                .map(|entry| entry.metadata().map_or(0, |metadata| metadata.len()))
                .sum::<u64>()
                <= 13
        );
    }

    #[test]
    fn retention_removes_expired_captures_but_ignores_other_files() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let capture = root.path().join("expired.ndjson");
        let unrelated = root.path().join("keep.txt");
        std::fs::write(&capture, b"capture").unwrap_or_else(|error| panic!("capture fixture write failed: {error}"));
        std::fs::write(&unrelated, b"keep").unwrap_or_else(|error| panic!("unrelated fixture write failed: {error}"));

        prune_at(root.path(), 1, 4, 100, SystemTime::now() + Duration::from_secs(1))
            .unwrap_or_else(|error| panic!("capture retention failed: {error}"));

        assert!(!capture.exists());
        assert!(unrelated.exists());
    }

    #[test]
    fn retention_rejects_a_directory_outside_the_panel_profile() {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir failed: {error}"));
        let valid = root.path().join(safe_local_id("panel")).join("captures");
        assert!(validate_panel_capture_directory("panel", &valid).is_ok());
        assert!(validate_panel_capture_directory("panel", root.path()).is_err());
        assert!(
            validate_panel_capture_directory("other-panel", &valid).is_err(),
            "retention must not prune another panel's storage"
        );
    }
}
