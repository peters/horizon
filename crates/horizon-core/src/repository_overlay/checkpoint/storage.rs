use super::{ADMISSION_BYTES, CheckpointError as Error, CheckpointRequest};
use crate::repository_overlay::reader::SelectedRepositoryReader;
use rustix::fs::{Mode, OFlags, ResolveFlags, mkdirat, openat2};
use std::{
    fs::{self, File},
    io::Write,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

const NODES: usize = 8192;
const RECORD_LIMIT: usize = 128 * 1024;

pub(super) struct Generation {
    pub path: PathBuf,
    root: File,
    parent: File,
    parent_path: PathBuf,
    budget: u64,
}

pub(super) fn private(path: &Path) -> Result<File, Error> {
    let reader = SelectedRepositoryReader::open(path).map_err(|_| Error::Identity)?;
    let file = reader.root.handle().try_clone().map_err(|_| Error::Storage)?;
    let meta = file.metadata().map_err(|_| Error::Storage)?;
    if meta.uid() != rustix::process::geteuid().as_raw() || meta.mode() & 0o7777 != 0o700 || meta.nlink() == 0 {
        return Err(Error::Identity);
    }
    Ok(file)
}

fn same(left: &File, right: &File) -> Result<(), Error> {
    let left = left.metadata().map_err(|_| Error::Storage)?;
    let right = right.metadata().map_err(|_| Error::Storage)?;
    if (left.dev(), left.ino(), left.uid(), left.mode()) == (right.dev(), right.ino(), right.uid(), right.mode()) {
        Ok(())
    } else {
        Err(Error::Identity)
    }
}

pub(super) fn usage(path: &Path, budget: u64) -> Result<u64, Error> {
    let root = private(path)?;
    let device = root.metadata().map_err(|_| Error::Storage)?.dev();
    let mut pending = vec![path.to_path_buf()];
    let (mut count, mut bytes) = (0, 0u64);
    while let Some(path) = pending.pop() {
        count += 1;
        if count > NODES {
            return Err(Error::Capacity);
        }
        let meta = fs::symlink_metadata(&path).map_err(|_| Error::Storage)?;
        if meta.dev() != device
            || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.mode() & 0o7022 != 0
            || !(meta.is_dir() || (meta.is_file() && meta.nlink() == 1))
        {
            return Err(Error::Identity);
        }
        bytes = bytes
            .checked_add(meta.len().max(meta.blocks().saturating_mul(512)).max(4096))
            .ok_or(Error::Capacity)?;
        if bytes > budget {
            return Err(Error::Capacity);
        }
        if meta.is_dir() {
            for child in fs::read_dir(path).map_err(|_| Error::Storage)? {
                if pending.len() + count >= NODES {
                    return Err(Error::Capacity);
                }
                pending.push(child.map_err(|_| Error::Storage)?.path());
            }
        }
    }
    same(&private(path)?, &root)?;
    Ok(bytes)
}

impl Generation {
    pub fn admit(request: &CheckpointRequest, checkout: &Path) -> Result<File, Error> {
        if request.parent.starts_with(checkout) || checkout.starts_with(&request.parent) {
            return Err(Error::Identity);
        }
        let parent = private(&request.parent)?;
        let source = SelectedRepositoryReader::open(checkout).map_err(|_| Error::Identity)?;
        if parent.metadata().map_err(|_| Error::Storage)?.dev()
            != source.root.handle().metadata().map_err(|_| Error::Storage)?.dev()
        {
            return Err(Error::Identity);
        }
        if usage(&request.parent, request.max_retained_bytes)?
            .checked_add(ADMISSION_BYTES)
            .is_none_or(|bytes| bytes > request.max_retained_bytes)
        {
            return Err(Error::Capacity);
        }
        Ok(parent)
    }

    pub fn claim(request: &CheckpointRequest, parent: File) -> Result<Self, Error> {
        mkdirat(&parent, request.attempt_name.as_str(), Mode::from_raw_mode(0o700)).map_err(|_| Error::Storage)?;
        let path = request.parent.join(&request.attempt_name);
        let root = private(&path)?;
        if root.metadata().map_err(|_| Error::Storage)?.dev() != parent.metadata().map_err(|_| Error::Storage)?.dev() {
            return Err(Error::Identity);
        }
        let result = Self {
            path,
            root,
            parent,
            parent_path: request.parent.clone(),
            budget: request.max_retained_bytes,
        };
        result.check()?;
        for name in ["scratch", "packs", "bundles"] {
            mkdirat(&result.root, name, Mode::from_raw_mode(0o700)).map_err(|_| Error::Storage)?;
        }
        Ok(result)
    }

    pub fn check(&self) -> Result<(), Error> {
        same(&private(&self.parent_path)?, &self.parent)?;
        same(&private(&self.path)?, &self.root)?;
        usage(&self.parent_path, self.budget)?;
        Ok(())
    }

    pub fn finish(&self, bytes: &[u8]) -> Result<(), Error> {
        self.check()?;
        if bytes.len() > RECORD_LIMIT {
            return Err(Error::Capacity);
        }
        let mut file = File::from(
            openat2(
                &self.root,
                "generation.json",
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
            )
            .map_err(|_| Error::Storage)?,
        );
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| Error::Storage)?;
        for directory in [&self.root, &self.parent] {
            File::from(
                openat2(
                    directory,
                    ".",
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                    ResolveFlags::BENEATH,
                )
                .map_err(|_| Error::Storage)?,
            )
            .sync_all()
            .map_err(|_| Error::Storage)?;
        }
        let reader = SelectedRepositoryReader::open(&self.path).map_err(|_| Error::Identity)?;
        let record = reader
            .root
            .read_private_file("generation.json", RECORD_LIMIT)
            .map_err(|_| Error::Storage)?;
        same(&file, &record.file)?;
        if record.bytes != bytes {
            return Err(Error::Storage);
        }
        self.check()
    }
}
