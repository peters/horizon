use super::{PreparedPrivateCheckout, PublicationError as Error, PublicationFailure, PublishedCheckout, walk};
use crate::repository_overlay::{paths, reader::SelectedRepositoryReader};
use rustix::fs::{
    CWD, Mode, OFlags, RenameFlags, ResolveFlags, fstatfs, major, minor, openat2, readlinkat_raw, renameat_with,
};
use std::{
    fs::{File, Metadata, OpenOptions},
    io::Read,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
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
    if sibling.len() > 255
        || sibling.contains('/')
        || paths::validate(sibling).is_err()
        || checkout.path.file_name() == Some(std::ffi::OsStr::new(sibling))
    {
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
    if fstatfs(parent).map_err(|_| Error::Storage)?.f_type != libc::EXT4_SUPER_MAGIC {
        return Err(Error::Unsupported);
    }
    let device = parent.metadata().map_err(|_| Error::Storage)?.dev();
    let mut buffer = [0; 4097];
    let length = readlinkat_raw(
        CWD,
        format!("/sys/dev/block/{}:{}", major(device), minor(device)),
        &mut buffer[..],
    )
    .map_err(|_| Error::Unsupported)?;
    if length == buffer.len() {
        return Err(Error::Unsupported);
    }
    let target = std::str::from_utf8(&buffer[..length]).map_err(|_| Error::Unsupported)?;
    let name = Path::new(target)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| name.len() <= 255 && paths::validate(name).is_ok())
        .ok_or(Error::Unsupported)?;
    let mut options = String::new();
    File::open(format!("/proc/fs/ext4/{name}/options"))
        .map_err(|_| Error::Unsupported)?
        .take(4097)
        .read_to_string(&mut options)
        .map_err(|_| Error::Unsupported)?;
    journaled_options(&options)
}

pub(super) fn journaled_options(options: &str) -> Result<(), Error> {
    if options.len() > 4096 || !options.ends_with('\n') {
        return Err(Error::Unsupported);
    }
    let lines: std::collections::BTreeSet<_> = options.split_terminator('\n').collect();
    if lines.len() != options.split_terminator('\n').count()
        || lines
            .iter()
            .any(|line| line.is_empty() || line.bytes().any(|b| b.is_ascii_control() || b == b' '))
        || !lines.contains("rw")
        || !lines.contains("barrier")
        || lines.contains("ro")
        || lines.contains("nobarrier")
        || lines.iter().filter(|line| line.starts_with("data=")).count() != 1
        || !(lines.contains("data=ordered") || lines.contains("data=journal"))
    {
        return Err(Error::Unsupported);
    }
    Ok(())
}
