//! Private one-shot result files for externally queued browser actions.

use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use atomicwrites::{AllowOverwrite, AtomicFile};
use horizon_browser::AgentActionResult;

use super::ManifestLock;
use crate::horizon_home::{HorizonHome, safe_local_id};

const MAX_RETAINED_RESULTS: usize = 256;
const RESULT_RETENTION: Duration = Duration::from_mins(5);

#[must_use]
pub fn action_result_path_for_root(root: &Path, panel_local_id: &str, action_id: &str) -> PathBuf {
    root.join("runtime")
        .join("browser-results")
        .join(safe_local_id(panel_local_id))
        .join(format!("{}.json", safe_local_id(action_id)))
}

#[must_use]
pub fn default_action_result_path(panel_local_id: &str, action_id: &str) -> PathBuf {
    action_result_path_for_root(HorizonHome::resolve().root(), panel_local_id, action_id)
}

pub(super) fn write(panel_local_id: &str, result: &AgentActionResult) -> std::io::Result<()> {
    write_at(&default_action_result_path(panel_local_id, &result.action_id), result)
}

pub(super) fn remove_stale(panel_local_id: &str) -> std::io::Result<()> {
    remove_stale_at(HorizonHome::resolve().root(), panel_local_id)
}

fn remove_stale_at(root: &Path, panel_local_id: &str) -> std::io::Result<()> {
    let panel_dir = root
        .join("runtime")
        .join("browser-results")
        .join(safe_local_id(panel_local_id));
    match std::fs::remove_dir_all(panel_dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn write_at(path: &Path, result: &AgentActionResult) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "browser action result path has no parent",
        ));
    };
    std::fs::create_dir_all(parent)?;
    let _lock = ManifestLock::acquire(path)?;
    let encoded = serde_json::to_vec(result).map_err(std::io::Error::other)?;
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    AtomicFile::new(path, AllowOverwrite)
        .write_with_options(|file| std::io::Write::write_all(file, &encoded), options)
        .map_err(std::io::Error::from)?;
    prune_results_at(parent, path, MAX_RETAINED_RESULTS, SystemTime::now() - RESULT_RETENTION)
}

fn prune_results_at(
    directory: &Path,
    current_path: &Path,
    max_results: usize,
    stale_before: SystemTime,
) -> std::io::Result<()> {
    let mut results = std::fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|extension| extension.to_str()) == Some("json")).then(|| {
                let modified = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                (modified, path)
            })
        })
        .collect::<Vec<_>>();
    results.sort();
    let fresh_count = results.iter().filter(|(modified, _)| *modified >= stale_before).count();
    let count_excess = fresh_count.saturating_sub(max_results);
    let mut fresh_removed = 0;
    for (modified, path) in results {
        if path == current_path || (modified >= stale_before && fresh_removed >= count_excess) {
            continue;
        }
        let _lock = match ManifestLock::acquire_with_timeout(&path, Duration::ZERO) {
            Ok(lock) => lock,
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(error) => return Err(error),
        };
        match std::fs::remove_file(&path) {
            Ok(()) => {
                if modified >= stale_before {
                    fresh_removed += 1;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Atomically consume an action result if the driver has published it.
///
/// # Errors
/// Returns an I/O or invalid-data error. An invalid result is retained for
/// diagnosis instead of being silently discarded.
pub fn take_action_result(panel_local_id: &str, action_id: &str) -> std::io::Result<Option<AgentActionResult>> {
    take_at(&default_action_result_path(panel_local_id, action_id), action_id)
}

fn take_at(path: &Path, action_id: &str) -> std::io::Result<Option<AgentActionResult>> {
    let _lock = match ManifestLock::acquire(path) {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let encoded = match std::fs::read(path) {
        Ok(encoded) => encoded,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let result: AgentActionResult = serde_json::from_slice(&encoded)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if result.action_id != action_id {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "browser action result identity did not match its path",
        ));
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(Some(result)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_browser::{BrowserControlValue, new_action_id};

    #[test]
    fn results_are_private_one_shot_files_with_confined_paths() {
        let root = std::env::temp_dir().join(format!(
            "horizon-browser-results-{}-{}",
            std::process::id(),
            new_action_id()
        ));
        let action_id = new_action_id();
        let path = action_result_path_for_root(&root, "../panel", &action_id);
        let result = AgentActionResult::completed(action_id.clone(), BrowserControlValue::Accepted);

        assert_eq!(take_at(&path, &action_id).unwrap(), None);
        assert!(!root.exists());
        write_at(&path, &result).unwrap();
        assert!(path.starts_with(&root));
        assert_eq!(take_at(&path, &action_id).unwrap(), Some(result));
        assert_eq!(take_at(&path, &action_id).unwrap(), None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let other_id = new_action_id();
            let other_path = action_result_path_for_root(&root, "panel", &other_id);
            write_at(
                &other_path,
                &AgentActionResult::completed(other_id, BrowserControlValue::Accepted),
            )
            .unwrap();
            assert_eq!(
                std::fs::metadata(other_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let retained_id = new_action_id();
        let retained_path = action_result_path_for_root(&root, "../panel", &retained_id);
        write_at(
            &retained_path,
            &AgentActionResult::completed(retained_id, BrowserControlValue::Accepted),
        )
        .unwrap();
        remove_stale_at(&root, "../panel").unwrap();
        assert!(!retained_path.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn mismatched_result_identity_is_not_consumed() {
        let root = std::env::temp_dir().join(format!("horizon-browser-results-mismatch-{}", std::process::id()));
        let path = action_result_path_for_root(&root, "panel", "expected");
        write_at(
            &path,
            &AgentActionResult::completed("different", BrowserControlValue::Accepted),
        )
        .unwrap();

        assert_eq!(
            take_at(&path, "expected").unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn result_retention_prunes_oldest_unconsumed_files() {
        let root = std::env::temp_dir().join(format!("horizon-browser-results-retention-{}", std::process::id()));
        let directory = root.join("runtime/browser-results/panel");
        std::fs::create_dir_all(&directory).unwrap();
        let paths = ["first.json", "second.json", "current.json"].map(|name| directory.join(name));
        for path in &paths {
            std::fs::write(path, b"{}").unwrap();
        }

        prune_results_at(&directory, &paths[2], 2, SystemTime::UNIX_EPOCH).unwrap();

        assert!(paths[2].exists());
        assert_eq!(
            std::fs::read_dir(&directory)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().and_then(|extension| extension.to_str()) == Some("json"))
                .count(),
            2
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
