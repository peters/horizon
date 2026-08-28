//! Shared private-file helpers for bounded host request queues.

use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use atomicwrites::{AllowOverwrite, AtomicFile};
use serde::{Serialize, de::DeserializeOwned};

pub(super) const MAX_PENDING_REQUESTS: usize = 32;
const REQUEST_RETENTION: Duration = Duration::from_mins(5);

pub(super) fn queue_lock_path(directory: &Path) -> PathBuf {
    directory.join("queue.json")
}

pub(super) fn request_count(directory: &Path) -> std::io::Result<usize> {
    Ok(std::fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".request.json"))
        .count())
}

pub(super) fn prune_at(directory: &Path) -> std::io::Result<()> {
    let stale_before = SystemTime::now()
        .checked_sub(REQUEST_RETENTION)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let is_request_file = entry.file_name().to_string_lossy().ends_with(".request.json")
            || entry.file_name().to_string_lossy().ends_with(".result.json");
        let is_stale = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| modified < stale_before);
        if !is_request_file || !is_stale {
            continue;
        }
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(super) fn write_private_json(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let encoded = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    AtomicFile::new(path, AllowOverwrite)
        .write_with_options(|file| std::io::Write::write_all(file, &encoded), options)
        .map_err(std::io::Error::from)
}

pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> std::io::Result<Option<T>> {
    let encoded = match std::fs::read(path) {
        Ok(encoded) => encoded,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    serde_json::from_slice(&encoded)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}
