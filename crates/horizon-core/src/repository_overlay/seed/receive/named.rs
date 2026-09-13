use super::{
    ExpectedGitPack, PackReceiveLimits, ReceivedGitPack, SeedError as Error, SeedFailure, observe::layout::reopen,
    observe_git_base_pack, receive, staging,
};
use crate::repository_overlay::{
    checkout::publication::validate_sibling_name,
    reader::{SelectedRepositoryReader, linux::same_metadata},
    seed::MAX_PACK_PATH_BYTES,
};
use rustix::fs::{Dir, Mode, OFlags, ResolveFlags, mkdirat, openat2, renameat};
use std::{
    fs::{File, Metadata},
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

const CONFINED: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);

struct Operations<'a> {
    claim: &'a mut dyn FnMut(&File, &str) -> rustix::io::Result<()>,
    rename: &'a mut dyn FnMut(&File, &str, &str) -> rustix::io::Result<()>,
}

/// Receive an explicit pack in one caller-named, exclusively created private attempt.
/// This opt-in mode uses ordinary renames only inside that owned reservation; the
/// default receiver retains its no-replace operations. Stable ancestry, exclusive
/// same-user control and trusted native tools remain caller preconditions, not
/// protection against concurrent same-user/privileged mutation. No filesystem is
/// qualified by this API and no synchronization or physical durability is claimed.
/// All existing encoded identity, EOF, decode, closure and per-child bounds apply.
/// Blocking input/storage work remains outside native deadlines. Run off the UI thread.
/// # Errors
/// Invalid metadata/name/parent fail before a claim or input read. Once mkdir is
/// attempted, every failure retains its attempted locator, which does NOT prove
/// ownership or existence. An existing name is never adopted, even if complete.
/// A mkdir/rename error can follow a successful operation: no retry, rollback,
/// repair, overwrite of an existing attempt or cleanup is authorized. Only a
/// separately requested read-only observation may inspect a retained complete tree.
pub fn receive_named_git_base_pack(
    parent: &Path,
    attempt_name: &str,
    expected: ExpectedGitPack<'_>,
    input: &mut impl Read,
    limits: PackReceiveLimits,
    cancelled: impl Fn() -> bool,
) -> Result<ReceivedGitPack, SeedFailure> {
    receive_with(
        parent,
        attempt_name,
        expected,
        input,
        limits,
        &cancelled,
        &mut Operations {
            claim: &mut |parent, name| mkdirat(parent, name, Mode::from_raw_mode(0o700)),
            rename: &mut |directory, source, target| renameat(directory, source, directory, target),
        },
    )
}

fn receive_with(
    parent: &Path,
    name: &str,
    expected: ExpectedGitPack<'_>,
    input: &mut impl Read,
    limits: PackReceiveLimits,
    cancelled: &impl Fn() -> bool,
    operations: &mut Operations<'_>,
) -> Result<ReceivedGitPack, SeedFailure> {
    let admission = || {
        limits.validate(expected)?;
        staging::check_cancel(cancelled)?;
        validate_sibling_name(name).map_err(|_| Error::UnsafeParent)?;
        let path = parent.join(name);
        if path.as_os_str().len() > MAX_PACK_PATH_BYTES {
            return Err(Error::Limit);
        }
        let parent = staging::private_parent(parent)?;
        staging::check_cancel(cancelled)?;
        Ok((path, parent))
    };
    let (path, parent_handle) = admission().map_err(|reason| SeedFailure { reason, residue: None })?;
    let run = || {
        // Only a successful exclusive claim grants permission to populate this name.
        (operations.claim)(parent_handle.root.handle(), name).map_err(|_| Error::Storage)?;
        let reservation = Reservation {
            root: directory(parent_handle.root.handle(), name, true)?,
            parent: parent_handle,
            parent_path: parent.to_path_buf(),
            name: name.to_owned(),
        };
        reservation.verify()?;
        children(&reservation.root, &[])?;
        staging::check_cancel(cancelled)?;
        receive(&path, expected, input, limits, cancelled, &mut |_, hash| {
            reservation.relocate(hash, cancelled, operations.rename)
        })?;
        reservation.verify()?;
        let observed = observe_git_base_pack(&path, expected, limits, cancelled)?;
        reservation.verify()?;
        Ok(observed)
    };
    run().map_err(|reason| SeedFailure {
        reason,
        residue: Some(path),
    })
}

struct Reservation {
    parent: SelectedRepositoryReader,
    parent_path: PathBuf,
    name: String,
    root: File,
}

impl Reservation {
    fn verify(&self) -> Result<(), Error> {
        let current = staging::private_parent(&self.parent_path)?;
        same_directory(current.root.handle(), self.parent.root.handle())?;
        same_directory(&directory(self.parent.root.handle(), &self.name, true)?, &self.root)
    }

    fn relocate(
        &self,
        hash: &str,
        cancelled: &impl Fn() -> bool,
        rename: &mut dyn FnMut(&File, &str, &str) -> rustix::io::Result<()>,
    ) -> Result<(), Error> {
        self.verify()?;
        let directory = directory(&self.root, "decoded/objects/pack", false)?;
        let nodes = [
            Node::open(&directory, "received.pack")?,
            Node::open(&directory, "received.idx")?,
        ];
        let mut names = ["received.pack".to_owned(), "received.idx".to_owned()];
        for (index, extension) in ["pack", "idx"].into_iter().enumerate() {
            staging::check_cancel(cancelled)?;
            self.verify()?;
            same_directory(&directory_at(&self.root)?, &directory)?;
            children(&directory, &[&names[0], &names[1]])?;
            for (node, name) in nodes.iter().zip(&names) {
                node.verify(&directory, name, name.starts_with("pack-"))?;
            }
            let target = format!("pack-{hash}.{extension}");
            // No target is present in this exclusively held private directory. Any
            // error is unconfirmed, including an error after the rename took effect.
            rename(&directory, &names[index], &target).map_err(|_| Error::Storage)?;
            names[index] = target;
        }
        self.verify()?;
        same_directory(&directory_at(&self.root)?, &directory)?;
        children(&directory, &[&names[0], &names[1]])?;
        for (node, name) in nodes.iter().zip(&names) {
            node.verify(&directory, name, true)?;
        }
        staging::check_cancel(cancelled)
    }
}

fn directory_at(root: &File) -> Result<File, Error> {
    directory(root, "decoded/objects/pack", false)
}

fn pin(parent: &File, name: &str) -> Result<File, Error> {
    openat2(
        parent,
        name,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        CONFINED,
    )
    .map(File::from)
    .map_err(|_| Error::UnsafeParent)
}

fn directory(parent: &File, name: &str, private: bool) -> Result<File, Error> {
    let handle = pin(parent, name)?;
    let metadata = handle.metadata().map_err(|_| Error::Storage)?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.nlink() == 0
        || metadata.mode() & 0o7000 != 0
        || (private && metadata.mode() & 0o7777 != 0o700)
    {
        return Err(Error::UnsafeParent);
    }
    Ok(handle)
}

fn same_directory(left: &File, right: &File) -> Result<(), Error> {
    let left = left.metadata().map_err(|_| Error::Storage)?;
    let right = right.metadata().map_err(|_| Error::Storage)?;
    if (left.dev(), left.ino(), left.uid(), left.gid(), left.mode())
        == (right.dev(), right.ino(), right.uid(), right.gid(), right.mode())
        && right.nlink() != 0
    {
        Ok(())
    } else {
        Err(Error::UnsafeParent)
    }
}

fn children(directory: &File, expected: &[&str]) -> Result<(), Error> {
    let mut names = Vec::new();
    for entry in Dir::read_from(&reopen(directory)?).map_err(|_| Error::Storage)? {
        let entry = entry.map_err(|_| Error::Storage)?;
        let name = entry.file_name().to_str().map_err(|_| Error::Object)?;
        if matches!(name, "." | "..") {
            continue;
        }
        if names.len() >= expected.len() || !expected.contains(&name) {
            return Err(Error::Object);
        }
        names.push(name.to_owned());
    }
    if names.len() == expected.len() {
        Ok(())
    } else {
        Err(Error::Object)
    }
}

struct Node {
    handle: File,
    metadata: Metadata,
}

impl Node {
    fn open(directory: &File, name: &str) -> Result<Self, Error> {
        let handle = pin(directory, name)?;
        let metadata = handle.metadata().map_err(|_| Error::Storage)?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.mode() & 0o7777 != 0o600
            || metadata.uid() != rustix::process::geteuid().as_raw()
        {
            return Err(Error::Object);
        }
        Ok(Self { handle, metadata })
    }

    fn verify(&self, directory: &File, name: &str, renamed: bool) -> Result<(), Error> {
        let held = self.handle.metadata().map_err(|_| Error::Storage)?;
        let named = Self::open(directory, name)?.metadata;
        let stable = if renamed {
            // A successful rename changes ctime, but must preserve every other
            // content/identity field (atime is deliberately not an integrity field).
            let before = &self.metadata;
            (
                before.dev(),
                before.ino(),
                before.uid(),
                before.gid(),
                before.mode(),
                before.nlink(),
                before.len(),
                before.mtime(),
                before.mtime_nsec(),
            ) == (
                held.dev(),
                held.ino(),
                held.uid(),
                held.gid(),
                held.mode(),
                held.nlink(),
                held.len(),
                held.mtime(),
                held.mtime_nsec(),
            )
        } else {
            same_metadata(&self.metadata, &held) && self.metadata.gid() == held.gid()
        };
        if stable && same_metadata(&held, &named) && held.gid() == named.gid() {
            Ok(())
        } else {
            Err(Error::Object)
        }
    }
}

#[cfg(test)]
mod tests;
