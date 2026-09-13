use super::super::{
    observe::layout::{Layout, reopen},
    observe_git_base_pack, staging,
};
use super::{
    PackPublicationError as Error, PackPublicationFailure as Failure, PackReceiveLimits, PublishedGitPack,
    ReceivedGitPack,
};
use crate::repository_overlay::{
    checkout::publication::validate_sibling_name, reader::SelectedRepositoryReader, storage,
};
use rustix::fs::{RenameFlags, renameat_with};
use std::{
    ffi::OsString,
    fs::File,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SyncPoint {
    File,
    Directory,
    PublishedRoot,
    Parent,
}

pub(super) struct Binding {
    parent: File,
    parent_path: PathBuf,
    source: OsString,
    layout: Layout,
}

pub(super) fn publish(
    mut pack: ReceivedGitPack,
    sibling: &str,
    limits: PackReceiveLimits,
    cancelled: &impl Fn() -> bool,
    sync: &mut impl FnMut(SyncPoint, &File) -> Result<(), Error>,
    rename: &mut impl FnMut(&Binding, &str) -> rustix::io::Result<()>,
    storage: &impl Fn(&File) -> Result<(), Error>,
) -> Result<PublishedGitPack, Failure> {
    let binding = match before(&pack, sibling, limits, cancelled, sync, storage) {
        Ok(binding) => binding,
        Err(reason) => return Err(Failure::Unpublished { reason, pack }),
    };
    if let Err(reason) = staging::check_cancel(cancelled) {
        return Err(Failure::Unpublished {
            reason: reason.into(),
            pack,
        });
    }
    if let Err(error) = rename(&binding, sibling) {
        let reason = match error {
            rustix::io::Errno::EXIST | rustix::io::Errno::NOTEMPTY => Error::DestinationExists,
            rustix::io::Errno::NOSYS => Error::Unsupported,
            _ => {
                return Err(Failure::RenameUnconfirmed {
                    destination: pack.path.with_file_name(sibling),
                    pack: Box::new(pack),
                });
            }
        };
        return Err(Failure::Unpublished { reason, pack });
    }
    // No later failure may describe a successful rename as an unpublished source.
    pack.path.set_file_name(sibling);
    pack.objects_directory = pack.path.join("decoded/objects");
    let pack = PublishedGitPack(pack);
    if let Err(reason) = after(pack.pack(), &binding, cancelled, sync) {
        return Err(Failure::PublishedUnsynchronized { reason, pack });
    }
    Ok(pack)
}

fn before(
    pack: &ReceivedGitPack,
    sibling: &str,
    limits: PackReceiveLimits,
    cancelled: &impl Fn() -> bool,
    sync: &mut impl FnMut(SyncPoint, &File) -> Result<(), Error>,
    storage: &impl Fn(&File) -> Result<(), Error>,
) -> Result<Binding, Error> {
    staging::check_cancel(cancelled)?;
    limits.validate(pack.into())?;
    validate_sibling_name(sibling).map_err(|_| Error::InvalidName)?;
    if pack.path.file_name() == Some(std::ffi::OsStr::new(sibling)) {
        return Err(Error::InvalidName);
    }
    let parent_path = pack.path.parent().ok_or(Error::Storage)?.to_path_buf();
    let parent = directory(&parent_path)?;
    let binding = Binding {
        parent,
        parent_path,
        source: pack.path.file_name().ok_or(Error::Storage)?.to_os_string(),
        layout: Layout::open(&pack.path, pack.into(), cancelled)?,
    };
    binding.verify_parent()?;
    storage(&binding.parent)?;
    observe_git_base_pack(&pack.path, pack.into(), limits, cancelled)?;
    binding.layout.recheck(cancelled)?;
    synchronize_layout(&binding.layout, cancelled, sync)?;
    binding.verify_parent()?;
    Ok(binding)
}

pub(super) fn synchronize_layout(
    layout: &Layout,
    cancelled: &impl Fn() -> bool,
    sync: &mut impl FnMut(SyncPoint, &File) -> Result<(), Error>,
) -> Result<(), Error> {
    for (point, files) in [
        (SyncPoint::File, layout.files().collect::<Vec<_>>()),
        (SyncPoint::Directory, layout.directories().rev().collect()),
    ] {
        for file in files {
            staging::check_cancel(cancelled)?;
            sync(point, &reopen(file)?)?;
            layout.recheck(cancelled)?;
        }
    }
    Ok(())
}

fn after(
    pack: &ReceivedGitPack,
    binding: &Binding,
    cancelled: &impl Fn() -> bool,
    sync: &mut impl FnMut(SyncPoint, &File) -> Result<(), Error>,
) -> Result<(), Error> {
    staging::check_cancel(cancelled)?;
    binding.verify_parent()?;
    let layout = Layout::open(&pack.path, pack.into(), cancelled)?;
    layout.matches_relocated(&binding.layout)?;
    for (point, file) in [
        (SyncPoint::PublishedRoot, layout.root()),
        (SyncPoint::Parent, &binding.parent),
    ] {
        staging::check_cancel(cancelled)?;
        sync(point, &reopen(file)?)?;
        layout.recheck(cancelled)?;
        layout.matches_relocated(&binding.layout)?;
        binding.verify_parent()?;
    }
    Ok(())
}

impl Binding {
    fn verify_parent(&self) -> Result<(), Error> {
        let named = directory(&self.parent_path)?.metadata().map_err(|_| Error::Storage)?;
        let held = self.parent.metadata().map_err(|_| Error::Storage)?;
        let root = self.layout.root().metadata().map_err(|_| Error::Storage)?;
        if (named.dev(), named.ino()) != (held.dev(), held.ino()) || root.dev() != held.dev() {
            Err(Error::Storage)
        } else {
            Ok(())
        }
    }
}

pub(super) fn directory(path: &Path) -> Result<File, Error> {
    let reader = SelectedRepositoryReader::open(path).map_err(|_| Error::Storage)?;
    let file = reader.root.handle().try_clone().map_err(|_| Error::Storage)?;
    let metadata = file.metadata().map_err(|_| Error::Storage)?;
    if !metadata.is_dir()
        || metadata.mode() & 0o7777 != 0o700
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.nlink() == 0
    {
        Err(Error::Storage)
    } else {
        Ok(file)
    }
}

pub(super) fn rename(binding: &Binding, sibling: &str) -> rustix::io::Result<()> {
    renameat_with(
        &binding.parent,
        &binding.source,
        &binding.parent,
        sibling,
        RenameFlags::NOREPLACE,
    )
}

pub(super) fn supported_storage(parent: &File) -> Result<(), Error> {
    storage::qualify(parent).map_err(|error| match error {
        storage::StorageQualificationError::Unsupported => Error::Unsupported,
        storage::StorageQualificationError::Storage => Error::Storage,
    })
}
