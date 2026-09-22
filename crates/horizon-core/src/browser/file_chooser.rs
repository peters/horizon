//! Bounded, asynchronous host directory browsing for manual browser uploads.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

pub use horizon_browser::{FileChooserAnswer, FileChooserHandle, FileChooserRequest};

#[derive(Debug)]
pub struct DirectoryEntry {
    pub path: PathBuf,
    pub directory: bool,
}

#[derive(Debug)]
pub struct DirectoryListing {
    pub entries: Vec<DirectoryEntry>,
    pub truncated: bool,
}

/// Reads at most 2,000 directory entries away from the render thread.
#[must_use]
pub fn read_directory(
    path: PathBuf,
    wake: impl FnOnce() + Send + 'static,
) -> Receiver<Result<DirectoryListing, String>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| {
            let directory = std::fs::read_dir(path).map_err(|error| error.to_string())?;
            let mut entries = Vec::new();
            let mut truncated = false;
            for (index, entry) in directory.take(2_001).enumerate() {
                if index == 2_000 {
                    truncated = true;
                    break;
                }
                let entry = entry.map_err(|error| error.to_string())?;
                let Ok(metadata) = std::fs::metadata(entry.path()) else {
                    continue;
                };
                if metadata.is_dir() || metadata.is_file() {
                    entries.push(DirectoryEntry {
                        path: entry.path(),
                        directory: metadata.is_dir(),
                    });
                }
            }
            entries.sort_by(|a, b| b.directory.cmp(&a.directory).then_with(|| a.path.cmp(&b.path)));
            Ok(DirectoryListing { entries, truncated })
        })();
        let _ = sender.send(result);
        wake();
    });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[cfg(unix)]
    #[test]
    fn skipped_entries_still_count_toward_the_directory_limit() {
        let root = tempfile::tempdir().expect("temporary directory");
        for index in 0..2_001 {
            std::os::unix::fs::symlink("missing-target", root.path().join(index.to_string())).expect("symlink");
        }
        for truncated in [true, false] {
            let listing = read_directory(root.path().to_path_buf(), || {})
                .recv_timeout(Duration::from_secs(5))
                .expect("worker")
                .expect("listing");
            assert!(listing.entries.is_empty());
            assert_eq!(listing.truncated, truncated);
            if truncated {
                std::fs::remove_file(root.path().join("2000")).expect("remove extra entry");
            }
        }
    }

    #[test]
    fn listing_orders_directories_first_and_reports_missing_directories() {
        let root = tempfile::tempdir().expect("temporary directory");
        std::fs::write(root.path().join("alpha.txt"), "synthetic").expect("file");
        std::fs::create_dir(root.path().join("z-folder")).expect("directory");
        let listing = read_directory(root.path().to_path_buf(), || {})
            .recv_timeout(Duration::from_secs(5))
            .expect("worker")
            .expect("listing");
        assert_eq!(listing.entries.len(), 2);
        assert!(listing.entries[0].directory);
        assert!(!listing.entries[1].directory);
        assert!(!listing.truncated);
        assert!(
            read_directory(root.path().join("missing"), || {})
                .recv_timeout(Duration::from_secs(5))
                .expect("worker")
                .is_err()
        );
    }
}
