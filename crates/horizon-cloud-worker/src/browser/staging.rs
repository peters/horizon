//! Local files that a presenting host sends for a worker browser.
//!
//! The control service owns the files in its own folder. It writes each file in
//! order, checks its size and SHA-256, and gives a browser only complete files
//! of that browser. Files that an answer or a drop used stay until the browser
//! closes, because the page reads them after the attachment. The oldest used
//! batches leave first when the service reaches its limits.
use base64::Engine;
use horizon_browser_protocol::cloud_view::{
    CloudStageChunk, CloudStageReceipt, CloudStagedFiles, CloudUploadLimits, valid_upload_batch, valid_upload_name,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Most bytes of all staged files of this service together.
const MAX_STAGED_BYTES: u64 = 1024 * 1024 * 1024;
/// Most batches of all browsers together.
const MAX_BATCHES: usize = 64;
/// The service removes a batch that no answer or drop used in this time.
const UNUSED_LIFETIME: Duration = Duration::from_mins(10);

pub(super) struct Staging {
    root: PathBuf,
    limits: CloudUploadLimits,
    max_staged_bytes: u64,
    max_batches: usize,
    /// Keyed by browser ID and batch ID.
    batches: BTreeMap<(String, String), Batch>,
    next_order: u64,
}

struct Batch {
    order: u64,
    files: BTreeMap<String, StagedFile>,
    touched: Instant,
    used: bool,
}

struct StagedFile {
    size: u64,
    received: u64,
    /// `None` after the service verified the whole file.
    digest: Option<Sha256>,
    /// Open while parts arrive, so each part goes to the file that the service created.
    output: Option<std::fs::File>,
}

impl Staging {
    /// Removes what an earlier service left: its browsers stopped with it. A
    /// folder that stays makes later uploads fail, never the cloud browsers.
    pub(super) fn new(root: PathBuf) -> Self {
        let _ = std::fs::remove_dir_all(&root);
        Self {
            root,
            limits: CloudUploadLimits::default(),
            max_staged_bytes: MAX_STAGED_BYTES,
            max_batches: MAX_BATCHES,
            batches: BTreeMap::new(),
            next_order: 0,
        }
    }

    pub(super) fn limits(&self) -> CloudUploadLimits {
        self.limits
    }

    /// Writes one chunk. The caller has checked that `chunk.id` is a live
    /// browser that accepts uploads.
    pub(super) fn stage(&mut self, chunk: &CloudStageChunk, now: Instant) -> Result<CloudStageReceipt, String> {
        if !valid_upload_batch(&chunk.batch) || !valid_upload_name(&chunk.name) {
            return Err("The upload has an invalid batch or file name".into());
        }
        if chunk.size > self.limits.max_batch_bytes {
            return Err(format!(
                "{} is larger than the upload limit of {} MB",
                chunk.name,
                self.limits.max_batch_bytes / (1024 * 1024)
            ));
        }
        let data = self.decode(&chunk.data)?;
        let end = chunk.offset.saturating_add(data.len() as u64);
        if end > chunk.size {
            return Err(format!(
                "The worker received more bytes of {} than Horizon announced",
                chunk.name
            ));
        }
        let key = (chunk.id.clone(), chunk.batch.clone());
        let new_batch = !self.batches.contains_key(&key);
        if new_batch {
            self.admit_batch(&key, now)?;
        }
        match self.batches.get(&key).and_then(|batch| batch.files.get(&chunk.name)) {
            None => {
                if let Err(error) = self.admit_file(&key, chunk) {
                    // A batch without a file would hold a place until the sweep.
                    if new_batch {
                        self.remove_batch(&key);
                    }
                    return Err(error);
                }
            }
            // A repeated chunk must not discard a verified file.
            Some(file) if file.digest.is_none() => {
                return Err(format!("The worker already has all of {}", chunk.name));
            }
            Some(_) => {}
        }
        let path = self.path(&key.0, &key.1, &chunk.name);
        let result = self.write(&key, chunk, &data, now);
        if result.is_err() {
            self.discard_file(&key, &chunk.name, &path);
        }
        result
    }

    /// The paths of complete files of one batch of `browser`.
    pub(super) fn resolve(&self, browser: &str, files: &CloudStagedFiles) -> Result<Vec<PathBuf>, String> {
        if files.names.is_empty() || files.names.len() > self.limits.max_files {
            return Err(format!("Choose 1 to {} files", self.limits.max_files));
        }
        if files.names.iter().collect::<BTreeSet<_>>().len() != files.names.len() {
            return Err("The upload names one file more than once".into());
        }
        let key = (browser.to_owned(), files.batch.clone());
        let batch = self
            .batches
            .get(&key)
            .ok_or("The worker no longer has these files. Upload them again.")?;
        for name in &files.names {
            if batch.files.get(name).is_none_or(|file| file.digest.is_some()) {
                return Err(format!("The worker did not receive all of {name}. Upload it again."));
            }
        }
        Ok(files
            .names
            .iter()
            .map(|name| self.path(browser, &files.batch, name))
            .collect())
    }

    /// A page got the files of this batch, so they stay until the browser closes.
    pub(super) fn mark_used(&mut self, browser: &str, batch: &str, now: Instant) {
        if let Some(batch) = self.batches.get_mut(&(browser.to_owned(), batch.to_owned())) {
            batch.used = true;
            batch.touched = now;
        }
    }

    /// Removes the batches of browsers that are gone and batches that nothing used in time.
    pub(super) fn sweep(&mut self, live: impl Fn(&str) -> bool, now: Instant) {
        let expired: Vec<_> = self
            .batches
            .iter()
            .filter(|((browser, _), batch)| {
                !live(browser) || (!batch.used && now.saturating_duration_since(batch.touched) >= UNUSED_LIFETIME)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            self.remove_batch(&key);
        }
    }

    fn decode(&self, data: &str) -> Result<Vec<u8>, String> {
        let bound = self.limits.max_chunk_bytes.div_ceil(3) * 4;
        if data.len() as u64 > bound {
            return Err("An upload chunk is larger than the worker accepts".into());
        }
        let data = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| "An upload chunk is not valid base64".to_owned())?;
        // Base64 rounds to groups of four, so the encoded bound alone admits two more bytes.
        if data.len() as u64 > self.limits.max_chunk_bytes {
            return Err("An upload chunk is larger than the worker accepts".into());
        }
        Ok(data)
    }

    fn admit_batch(&mut self, key: &(String, String), now: Instant) -> Result<(), String> {
        while self.batches.len() >= self.max_batches {
            if !self.evict_oldest_used(key) {
                return Err("The worker holds too many uploads that wait for a page. Try again later.".into());
            }
        }
        self.next_order += 1;
        self.batches.insert(
            key.clone(),
            Batch {
                order: self.next_order,
                files: BTreeMap::new(),
                touched: now,
                used: false,
            },
        );
        Ok(())
    }

    fn admit_file(&mut self, key: &(String, String), chunk: &CloudStageChunk) -> Result<(), String> {
        if chunk.offset != 0 {
            return Err(format!("The worker has no start of {}. Upload it again.", chunk.name));
        }
        let batch = &self.batches[key];
        if batch.used {
            return Err("These files are already attached. Upload new files in a new batch.".into());
        }
        if batch.files.len() >= self.limits.max_files {
            return Err(format!("Choose at most {} files", self.limits.max_files));
        }
        let batch_bytes: u64 = batch.files.values().map(|file| file.size).sum();
        if batch_bytes.saturating_add(chunk.size) > self.limits.max_batch_bytes {
            return Err(format!(
                "The files are larger than the upload limit of {} MB",
                self.limits.max_batch_bytes / (1024 * 1024)
            ));
        }
        while self.staged_bytes().saturating_add(chunk.size) > self.max_staged_bytes {
            if !self.evict_oldest_used(key) {
                return Err("The worker has no room for more uploads. Close a cloud browser, then try again.".into());
            }
        }
        let output =
            create_staged_file(&self.root, &key.0, &key.1, &chunk.name).map_err(|error| match error.kind() {
                io::ErrorKind::AlreadyExists => format!("The upload names {} more than once", chunk.name),
                _ => format!("The worker could not save {}: {error}", chunk.name),
            })?;
        if let Some(batch) = self.batches.get_mut(key) {
            batch.files.insert(
                chunk.name.clone(),
                StagedFile {
                    size: chunk.size,
                    received: 0,
                    digest: Some(Sha256::new()),
                    output: Some(output),
                },
            );
        }
        Ok(())
    }

    fn write(
        &mut self,
        key: &(String, String),
        chunk: &CloudStageChunk,
        data: &[u8],
        now: Instant,
    ) -> Result<CloudStageReceipt, String> {
        let batch = self.batches.get_mut(key).ok_or("The upload batch is gone")?;
        batch.touched = now;
        let file = batch.files.get_mut(&chunk.name).ok_or("The upload file is gone")?;
        let digest = file.digest.as_mut().ok_or("The upload file is complete")?;
        if file.size != chunk.size || file.received != chunk.offset {
            return Err(format!(
                "The parts of {} arrived out of order. Upload it again.",
                chunk.name
            ));
        }
        file.output
            .as_mut()
            .ok_or("The upload file is closed")?
            .write_all(data)
            .map_err(|error| format!("The worker could not save {}: {error}", chunk.name))?;
        digest.update(data);
        file.received += data.len() as u64;
        if file.received < file.size {
            return match chunk.sha256 {
                Some(_) => Err(format!("The checksum of {} arrived before its last part", chunk.name)),
                None => Ok(CloudStageReceipt {
                    received: file.received,
                    complete: false,
                }),
            };
        }
        file.output = None;
        let actual = hex(&file.digest.take().unwrap_or_default().finalize());
        if chunk.sha256.as_deref() != Some(actual.as_str()) {
            return Err(format!(
                "The worker received different bytes of {} than Horizon sent. Upload it again.",
                chunk.name
            ));
        }
        Ok(CloudStageReceipt {
            received: file.received,
            complete: true,
        })
    }

    fn staged_bytes(&self) -> u64 {
        self.batches
            .values()
            .flat_map(|batch| batch.files.values())
            .map(|file| file.size)
            .sum()
    }

    /// Removes the oldest used batch other than `keep`. Returns false when there is none.
    fn evict_oldest_used(&mut self, keep: &(String, String)) -> bool {
        let oldest = self
            .batches
            .iter()
            .filter(|(key, batch)| batch.used && *key != keep)
            .min_by_key(|(_, batch)| batch.order)
            .map(|(key, _)| key.clone());
        oldest.is_some_and(|key| {
            self.remove_batch(&key);
            true
        })
    }

    /// A batch without files would hold a place until the sweep, so it goes too.
    fn discard_file(&mut self, key: &(String, String), name: &str, path: &Path) {
        let _ = std::fs::remove_file(path);
        if let Some(batch) = self.batches.get_mut(key) {
            batch.files.remove(name);
            if batch.files.is_empty() {
                self.remove_batch(key);
            }
        }
    }

    fn remove_batch(&mut self, key: &(String, String)) {
        self.batches.remove(key);
        let browser = self.root.join(&key.0);
        let _ = std::fs::remove_dir_all(browser.join(&key.1));
        // Succeeds only for the browser's last batch.
        let _ = std::fs::remove_dir(browser);
    }

    fn path(&self, browser: &str, batch: &str, name: &str) -> PathBuf {
        self.root.join(browser).join(batch).join(name)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(64), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// Creates `root/browser/batch/name` as a new private file. Below the parent of
/// `root`, no step follows a symbolic link, so a link that an agent planted
/// cannot move the file out of its batch folder.
#[cfg(unix)]
fn create_staged_file(root: &Path, browser: &str, batch: &str, name: &str) -> io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags, openat};
    let (Some(parent), Some(folder)) = (root.parent(), root.file_name()) else {
        return Err(io::Error::other("The upload folder has no parent"));
    };
    std::fs::create_dir_all(parent)?;
    let parent = openat(
        rustix::fs::CWD,
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let root = private_directory(&parent, folder)?;
    let browser = private_directory(&root, browser)?;
    let batch = private_directory(&browser, batch)?;
    let file = openat(
        &batch,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )?;
    Ok(std::fs::File::from(file))
}

/// Makes the folder `name` in `parent` if it is missing and opens it, never through a link.
#[cfg(unix)]
fn private_directory(
    parent: impl std::os::fd::AsFd,
    name: impl rustix::path::Arg + Copy,
) -> io::Result<std::os::fd::OwnedFd> {
    use rustix::fs::{Mode, OFlags};
    match rustix::fs::mkdirat(&parent, name, Mode::RWXU) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(error) => return Err(error.into()),
    }
    Ok(rustix::fs::openat(
        &parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?)
}

/// The worker runs on Linux; other hosts only build and test this module.
#[cfg(not(unix))]
fn create_staged_file(root: &Path, browser: &str, batch: &str, name: &str) -> io::Result<std::fs::File> {
    let directory = root.join(browser).join(batch);
    std::fs::create_dir_all(&directory)?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join(name))
}

#[cfg(test)]
mod tests;
