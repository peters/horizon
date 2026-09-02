//! Private one-shot result files for externally queued browser actions.

use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

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
    // The production caller holds this panel's stable manifest lock through
    // `with_owned_manifest`, before the per-action lock is opened below.
    write_at(&default_action_result_path(panel_local_id, &result.action_id), result)
}

pub(super) fn remove_stale(panel_local_id: &str) -> std::io::Result<()> {
    let root = HorizonHome::resolve();
    let manifest_path = super::manifest_path_for_root(root.root(), panel_local_id);
    let _manifest_lock = ManifestLock::acquire(&manifest_path)?;
    remove_stale_at(root.root(), panel_local_id)
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

pub(super) fn remove_stale_except_at(
    root: &Path,
    panel_local_id: &str,
    retained_action_id: Option<&str>,
    timeout: Duration,
) -> std::io::Result<()> {
    // `remove_owned_at_with_timeout` holds the stable panel manifest lock.
    // Every production result writer and consumer takes that lock before a
    // per-action lock, so deleting action lock paths cannot split contenders
    // across different inodes.
    let Some(retained_action_id) = retained_action_id else {
        return remove_stale_at(root, panel_local_id);
    };
    let retained_path = action_result_path_for_root(root, panel_local_id, retained_action_id);
    let panel_dir = retained_path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "browser action result path has no parent",
        )
    })?;
    let entries = match std::fs::read_dir(panel_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let retain_result = retained_path.try_exists()?;
    let mut result_paths = Vec::new();
    for entry in entries {
        let path = entry?.path();
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("json") => result_paths.push(path),
            Some("lock") => {
                let result_path = path.with_extension("");
                if result_path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
                {
                    result_paths.push(result_path);
                }
            }
            Some(_) | None => {}
        }
    }
    result_paths.sort();
    result_paths.dedup();
    let deadline = Instant::now() + timeout;
    for path in result_paths {
        if retain_result && path == retained_path {
            continue;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let _lock = ManifestLock::acquire_with_timeout(&path, remaining)?;
        remove_file_if_present(&path)?;
        remove_file_if_present(&result_lock_path(&path))?;
    }
    remove_empty_directory(panel_dir)
}

fn result_lock_path(result_path: &Path) -> PathBuf {
    result_path.with_extension("json.lock")
}

fn remove_file_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_empty_directory(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn remove_consumed_lock_at(result_path: &Path) -> std::io::Result<()> {
    // The caller still holds the stable panel lock, and a successfully
    // consumed one-shot result cannot be written again. It is therefore safe
    // to remove this per-action coordination inode even while the panel lives.
    let _result_lock = match ManifestLock::acquire(result_path) {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if result_path.exists() {
        return Ok(());
    }
    remove_file_if_present(&result_lock_path(result_path))?;
    let Some(panel_dir) = result_path.parent() else {
        return Ok(());
    };
    remove_empty_directory(panel_dir)
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
    take_action_result_at(HorizonHome::resolve().root(), panel_local_id, action_id)
}

fn take_action_result_at(
    root: &Path,
    panel_local_id: &str,
    action_id: &str,
) -> std::io::Result<Option<AgentActionResult>> {
    let manifest_path = super::manifest_path_for_root(root, panel_local_id);
    let _manifest_lock = ManifestLock::acquire(&manifest_path)?;
    let path = action_result_path_for_root(root, panel_local_id, action_id);
    let result = take_at(&path, action_id)?;
    if result.is_some()
        && let Err(error) = remove_consumed_lock_at(&path)
    {
        tracing::warn!(target: "browser", path = %path.display(), "failed to remove consumed browser result lock: {error}");
    }
    Ok(result)
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
    use horizon_browser::{BrowserControlFailure, BrowserControlValue, new_action_id};

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
    fn driver_teardown_preserves_a_terminal_result_for_its_consumer() {
        let root = std::env::temp_dir().join(format!(
            "horizon-browser-results-teardown-{}-{}",
            std::process::id(),
            new_action_id()
        ));
        let panel_id = "panel";
        let manifest_path = super::super::manifest_path_for_root(&root, panel_id);
        let manifest = super::super::BrowserManifest {
            panel_local_id: panel_id.to_string(),
            host: Some("host-a".to_string()),
            ..super::super::BrowserManifest::default()
        };
        super::super::write_at(&manifest_path, &manifest).unwrap();
        let action_id = new_action_id();
        let result_path = action_result_path_for_root(&root, panel_id, &action_id);
        let result = AgentActionResult::failed(
            action_id.clone(),
            BrowserControlFailure::new("browser_unavailable", "the browser stopped while waiting"),
        );
        write_at(&result_path, &result).unwrap();
        let unrelated_action_id = new_action_id();
        let unrelated_result_path = action_result_path_for_root(&root, panel_id, &unrelated_action_id);
        write_at(
            &unrelated_result_path,
            &AgentActionResult::completed(
                unrelated_action_id,
                BrowserControlValue::Json {
                    value: serde_json::json!({ "private": "page data" }),
                },
            ),
        )
        .unwrap();
        let consumed_action_id = new_action_id();
        let consumed_result_path = action_result_path_for_root(&root, panel_id, &consumed_action_id);
        write_at(
            &consumed_result_path,
            &AgentActionResult::completed(consumed_action_id.clone(), BrowserControlValue::Accepted),
        )
        .unwrap();
        assert!(take_at(&consumed_result_path, &consumed_action_id).unwrap().is_some());
        let consumed_lock_path = result_lock_path(&consumed_result_path);
        assert!(consumed_lock_path.exists());

        let coordination = super::super::ManifestCoordination::default();
        horizon_browser::BrowserCoordination::retain_action_result_on_remove(&coordination, panel_id, &action_id);
        let removed = coordination.remove_at(&root, panel_id, "host-a", Duration::from_secs(1));
        assert!(removed.unwrap());
        assert!(!manifest_path.exists());
        assert!(!unrelated_result_path.exists());
        assert!(!result_lock_path(&unrelated_result_path).exists());
        assert!(!consumed_lock_path.exists());
        assert_eq!(
            take_action_result_at(&root, panel_id, &action_id).unwrap(),
            Some(result)
        );
        assert!(!result_lock_path(&result_path).exists());
        assert_eq!(take_action_result_at(&root, panel_id, &action_id).unwrap(), None);
        assert!(!result_path.parent().unwrap().exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn result_consumption_waits_for_the_stable_panel_lock() {
        let root = std::env::temp_dir().join(format!(
            "horizon-browser-results-panel-lock-{}-{}",
            std::process::id(),
            new_action_id()
        ));
        let panel_id = "panel";
        let action_id = new_action_id();
        let result_path = action_result_path_for_root(&root, panel_id, &action_id);
        let result = AgentActionResult::completed(action_id.clone(), BrowserControlValue::Accepted);
        write_at(&result_path, &result).unwrap();
        let manifest_path = super::super::manifest_path_for_root(&root, panel_id);
        super::super::write_at(
            &manifest_path,
            &super::super::BrowserManifest {
                panel_local_id: panel_id.to_string(),
                ..super::super::BrowserManifest::default()
            },
        )
        .unwrap();
        let manifest_lock = ManifestLock::acquire(&manifest_path).unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let thread_root = root.clone();
        let thread_action_id = action_id.clone();
        let consumer = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let taken = take_action_result_at(&thread_root, panel_id, &thread_action_id);
            result_tx.send(taken).unwrap();
        });

        started_rx.recv().unwrap();
        assert!(result_rx.recv_timeout(Duration::from_millis(25)).is_err());
        drop(manifest_lock);
        assert_eq!(
            result_rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap(),
            Some(result)
        );
        consumer.join().unwrap();
        assert!(!result_lock_path(&result_path).exists());
        assert!(!result_path.parent().unwrap().exists());
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
