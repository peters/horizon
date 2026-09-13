use super::{ADMISSION_BYTES, CheckpointError as Error, CheckpointRequest};
use crate::repository_overlay::reader::SelectedRepositoryReader;
use rustix::fs::{AtFlags, Mode, OFlags, ResolveFlags, StatxFlags, mkdirat, openat2, statx};
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
    if mount(left)? != mount(right)? {
        return Err(Error::Identity);
    }
    let left = left.metadata().map_err(|_| Error::Storage)?;
    let right = right.metadata().map_err(|_| Error::Storage)?;
    if (left.dev(), left.ino(), left.uid(), left.mode()) == (right.dev(), right.ino(), right.uid(), right.mode()) {
        Ok(())
    } else {
        Err(Error::Identity)
    }
}

fn mount(file: &File) -> Result<u64, Error> {
    let stat = statx(file, "", AtFlags::EMPTY_PATH, StatxFlags::MNT_ID).map_err(|_| Error::Identity)?;
    if stat.stx_mask & StatxFlags::MNT_ID.bits() == 0 || stat.stx_mnt_id == 0 {
        return Err(Error::Identity);
    }
    Ok(stat.stx_mnt_id)
}

fn unrelated(ancestor: &File, descendant: &File) -> Result<(), Error> {
    let ancestor = ancestor.metadata().map_err(|_| Error::Storage)?;
    let identity = (ancestor.dev(), ancestor.ino());
    let expected_mount = mount(descendant)?;
    let mut current = descendant.try_clone().map_err(|_| Error::Storage)?;
    for _ in 0..256 {
        let meta = current.metadata().map_err(|_| Error::Storage)?;
        if (meta.dev(), meta.ino()) == identity {
            return Err(Error::Identity);
        }
        let parent = File::from(
            openat2(
                &current,
                "..",
                OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
            )
            .map_err(|_| Error::Identity)?,
        );
        let next = parent.metadata().map_err(|_| Error::Storage)?;
        if mount(&parent)? != expected_mount || (meta.dev(), meta.ino()) == (next.dev(), next.ino()) {
            return Ok(());
        }
        current = parent;
    }
    Err(Error::Identity)
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
        // The held 0700 root confines inherited descendant modes, as in the pack receiver.
        if meta.dev() != device
            || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.mode() & 0o7000 != 0
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
            || mount(&parent)? != mount(source.root.handle())?
        {
            return Err(Error::Identity);
        }
        // Device equality alone admits bind-mounted aliases of the checkout.
        unrelated(&parent, source.root.handle())?;
        unrelated(source.root.handle(), &parent)?;
        if usage(&request.parent, request.max_retained_bytes)?
            .checked_add(ADMISSION_BYTES)
            .is_none_or(|bytes| bytes > request.max_retained_bytes)
        {
            return Err(Error::Capacity);
        }
        same(&private(&request.parent)?, &parent)?;
        same(
            SelectedRepositoryReader::open(checkout)
                .map_err(|_| Error::Identity)?
                .root
                .handle(),
            source.root.handle(),
        )?;
        Ok(parent)
    }

    pub fn claim(request: &CheckpointRequest, parent: File) -> Result<Self, Error> {
        same(&private(&request.parent)?, &parent)?;
        mkdirat(&parent, request.attempt_name.as_str(), Mode::from_raw_mode(0o700)).map_err(|_| Error::Storage)?;
        let path = request.parent.join(&request.attempt_name);
        let root = private(&path)?;
        if root.metadata().map_err(|_| Error::Storage)?.dev() != parent.metadata().map_err(|_| Error::Storage)?.dev()
            || mount(&root)? != mount(&parent)?
        {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_ancestry_rejects_self_and_ancestors_but_accepts_siblings() {
        let fixture = tempfile::tempdir().unwrap();
        let root = File::open(fixture.path()).unwrap();
        let child = fixture.path().join("child");
        let sibling = fixture.path().join("sibling");
        fs::create_dir(&child).unwrap();
        fs::create_dir(&sibling).unwrap();
        let child = File::open(child).unwrap();
        let sibling = File::open(sibling).unwrap();
        assert_eq!(mount(&root).unwrap(), mount(&child).unwrap());
        assert_eq!(mount(&child).unwrap(), mount(&sibling).unwrap());
        assert_eq!(unrelated(&child, &child), Err(Error::Identity));
        assert_eq!(unrelated(&root, &child), Err(Error::Identity));
        assert_eq!(unrelated(&root, &sibling), Err(Error::Identity));
        assert!(unrelated(&child, &sibling).is_ok());
        assert!(unrelated(&sibling, &child).is_ok());
    }
}
