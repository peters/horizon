use super::{
    PreparedPrivateCheckout, PublicationError as Error, PublicationFailure, PublishedCheckout, validate_sibling_name,
    walk,
};
use crate::repository_overlay::{reader::SelectedRepositoryReader, storage};
use rustix::fs::{Mode, OFlags, RenameFlags, ResolveFlags, openat2, renameat_with};
use std::{
    fs::{File, Metadata, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
};

const CONFINED: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SyncPoint {
    File,
    Directory,
    PublishedRoot,
    Parent,
}

pub(super) fn publish(
    mut checkout: PreparedPrivateCheckout,
    sibling: &str,
    cancelled: &impl Fn() -> bool,
    sync: &mut impl FnMut(SyncPoint, &File) -> Result<(), Error>,
    rename: &mut impl FnMut(&PreparedPrivateCheckout, &str) -> rustix::io::Result<()>,
    storage: &impl Fn(&File) -> Result<(), Error>,
) -> Result<PublishedCheckout, PublicationFailure> {
    if let Err(reason) = before_rename(&checkout, sibling, cancelled, sync, storage) {
        return Err(PublicationFailure::Unpublished { reason, checkout });
    }
    if let Err(error) = rename(&checkout, sibling) {
        let reason = match error {
            rustix::io::Errno::EXIST | rustix::io::Errno::NOTEMPTY => Error::DestinationExists,
            rustix::io::Errno::NOSYS => Error::Unsupported,
            _ => {
                return Err(PublicationFailure::RenameUnconfirmed {
                    destination: checkout.path.with_file_name(sibling),
                    checkout,
                });
            }
        };
        return Err(PublicationFailure::Unpublished { reason, checkout });
    }
    // No fallible operation may misreport a successful rename as an unpublished stage.
    checkout.path.set_file_name(sibling);
    let published = PublishedCheckout(checkout);
    if let Err(reason) = after_rename(&published.0, cancelled, sync) {
        return Err(PublicationFailure::PublishedUnsynchronized {
            reason,
            checkout: published,
        });
    }
    Ok(published)
}

fn before_rename(
    checkout: &PreparedPrivateCheckout,
    sibling: &str,
    cancelled: &impl Fn() -> bool,
    sync: &mut impl FnMut(SyncPoint, &File) -> Result<(), Error>,
    storage: &impl Fn(&File) -> Result<(), Error>,
) -> Result<(), Error> {
    check_cancel(cancelled)?;
    validate_sibling_name(sibling)?;
    if checkout.path.file_name() == Some(std::ffi::OsStr::new(sibling)) {
        return Err(Error::InvalidName);
    }
    verify_binding(checkout)?;
    storage(&checkout.parent)?;
    walk::synchronize(checkout.root.handle(), cancelled, sync)?;
    check_cancel(cancelled)?;
    verify_binding(checkout)
}

pub(super) fn rename(checkout: &PreparedPrivateCheckout, sibling: &str) -> rustix::io::Result<()> {
    renameat_with(
        &checkout.parent,
        checkout.path.file_name().ok_or(rustix::io::Errno::INVAL)?,
        &checkout.parent,
        sibling,
        RenameFlags::NOREPLACE,
    )
}

fn after_rename(
    checkout: &PreparedPrivateCheckout,
    cancelled: &impl Fn() -> bool,
    sync: &mut impl FnMut(SyncPoint, &File) -> Result<(), Error>,
) -> Result<(), Error> {
    for (point, handle) in [
        (SyncPoint::PublishedRoot, checkout.root.handle()),
        (SyncPoint::Parent, &checkout.parent),
    ] {
        check_cancel(cancelled)?;
        sync(point, &reopen(handle)?)?;
    }
    check_cancel(cancelled)?;
    verify_binding(checkout)
}

fn verify_binding(checkout: &PreparedPrivateCheckout) -> Result<(), Error> {
    let parent_path = checkout.path.parent().ok_or(Error::UnsafeNode)?;
    let current = SelectedRepositoryReader::open(parent_path).map_err(|_| Error::UnsafeNode)?;
    let parent = checkout.parent.metadata().map_err(|_| Error::Storage)?;
    let named_parent = current.root.handle().metadata().map_err(|_| Error::Storage)?;
    let held = checkout.root.handle().metadata().map_err(|_| Error::Storage)?;
    let named = pin(&checkout.parent, checkout.path.file_name().ok_or(Error::UnsafeNode)?)?
        .metadata()
        .map_err(|_| Error::Storage)?;
    for metadata in [&parent, &named_parent, &held, &named] {
        if !metadata.is_dir()
            || metadata.mode() & 0o7777 != 0o700
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.nlink() == 0
        {
            return Err(Error::UnsafeNode);
        }
    }
    if identity(&parent) != identity(&named_parent) || identity(&held) != identity(&named) || parent.dev() != held.dev()
    {
        return Err(Error::UnsafeNode);
    }
    Ok(())
}

pub(super) fn pin(root: &File, path: &std::ffi::OsStr) -> Result<File, Error> {
    openat2(
        root,
        path,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        CONFINED,
    )
    .map(File::from)
    .map_err(|_| Error::UnsafeNode)
}

pub(super) fn reopen(file: &File) -> Result<File, Error> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(format!("/proc/self/fd/{}", file.as_raw_fd()))
        .map_err(|_| Error::Storage)
}

pub(super) fn check_cancel(cancelled: &impl Fn() -> bool) -> Result<(), Error> {
    if cancelled() { Err(Error::Cancelled) } else { Ok(()) }
}

fn identity(metadata: &Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}

pub(super) fn supported_storage(parent: &File) -> Result<(), Error> {
    storage::qualify(parent).map_err(|error| match error {
        storage::StorageQualificationError::Unsupported => Error::Unsupported,
        storage::StorageQualificationError::Storage => Error::Storage,
    })
}
