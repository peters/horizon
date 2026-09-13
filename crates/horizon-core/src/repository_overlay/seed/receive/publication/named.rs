use super::super::{
    observe::layout::{Layout, reopen},
    observe_git_base_pack, staging,
};
use super::{
    NamedPackPublication as Outcome, NamedPackPublicationFailure as Failure, PackPublicationError as Error,
    PackReceiveLimits, PublishedGitPack, ReceivedGitPack,
    linux::{SyncPoint, directory, synchronize_layout},
};
use crate::repository_overlay::seed::MAX_PACK_PATH_BYTES;
use rustix::fs::{Dir, Mode, OFlags, ResolveFlags, mkdirat, openat2, renameat};
use std::{
    fs::File,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

const PACK: &str = "pack";
const PRIVATE: Mode = Mode::RUSR.union(Mode::WUSR).union(Mode::XUSR);
const CONFINED: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);

pub(super) struct Binding {
    root: File,
    path: PathBuf,
    destination: PathBuf,
    digest: String,
    source: PathBuf,
    layout: Layout,
}

pub(super) struct Slot(File);

pub(super) fn publish(
    root: &Path,
    pack: ReceivedGitPack,
    limits: PackReceiveLimits,
    cancelled: &impl Fn() -> bool,
    sync: &mut impl FnMut(SyncPoint, &File) -> Result<(), Error>,
    claim: &mut impl FnMut(&File, &str) -> rustix::io::Result<()>,
    rename: &mut impl FnMut(&Binding, &Slot) -> rustix::io::Result<()>,
) -> Result<Outcome, Failure> {
    let destination = root
        .as_os_str()
        .len()
        .checked_add(pack.sha256().as_str().len() + PACK.len() + 2)
        .is_some_and(|length| length <= MAX_PACK_PATH_BYTES)
        .then(|| root.join(pack.sha256().as_str()).join(PACK));
    let binding = match Binding::open(root, &pack, destination.as_deref(), limits, cancelled) {
        Ok(binding) => binding,
        Err(reason) => return Err(retained(reason, pack, destination)),
    };
    let existing = match Slot::open(&binding) {
        Ok(slot) => slot,
        Err(reason) => return Err(retained(reason, pack, destination)),
    };
    if let Some(slot) = existing {
        return existing_pack(&binding, &slot, pack, limits, cancelled, sync);
    }
    let prepared = synchronize_layout(&binding.layout, cancelled, &mut |point, file| {
        sync(point, file)?;
        binding.verify_input(cancelled)
    })
    .and_then(|()| binding.verify_input(cancelled))
    .and_then(|()| staging::check_cancel(cancelled).map_err(Error::from));
    if let Err(reason) = prepared {
        return Err(retained(reason, pack, destination));
    }
    match claim(&binding.root, &binding.digest) {
        Ok(()) => {}
        Err(rustix::io::Errno::EXIST) => {
            return match Slot::open(&binding) {
                Ok(Some(slot)) => existing_pack(&binding, &slot, pack, limits, cancelled, sync),
                _ => Err(retained(Error::DestinationExists, pack, destination)),
            };
        }
        Err(_) => {
            return Err(Failure::ClaimUnconfirmed {
                pack: Box::new(pack),
                destination: binding.destination,
            });
        }
    }
    let slot = match prepare_slot(&binding, cancelled, sync) {
        Ok(slot) => slot,
        Err(reason) => return Err(retained(reason, pack, destination)),
    };
    // This invocation alone owns this fresh, still-empty permanent slot.
    if rename(&binding, &slot).is_err() {
        return Err(Failure::RenameUnconfirmed {
            pack: Box::new(pack),
            destination: binding.destination,
        });
    }
    let mut pack = pack;
    pack.path.clone_from(&binding.destination);
    pack.objects_directory = pack.path.join("decoded/objects");
    let pack = PublishedGitPack(pack);
    if let Err(reason) = finish(&binding, &slot, pack.pack(), cancelled, sync) {
        return Err(Failure::PublishedUnsynchronized {
            reason,
            pack: Box::new(pack),
        });
    }
    Ok(Outcome::Published(pack))
}

impl Binding {
    fn open(
        root: &Path,
        pack: &ReceivedGitPack,
        destination: Option<&Path>,
        limits: PackReceiveLimits,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Self, Error> {
        staging::check_cancel(cancelled)?;
        limits.validate(pack.into())?;
        let destination = destination.ok_or(Error::Verification(super::SeedError::Limit))?;
        if pack.path().as_os_str().len() > MAX_PACK_PATH_BYTES
            || (pack.path().parent() != Some(root) && pack.path() != destination)
        {
            return Err(Error::Verification(super::SeedError::UnsafeParent));
        }
        let result = Self {
            root: directory(root)?,
            path: root.to_path_buf(),
            destination: destination.to_path_buf(),
            digest: pack.sha256().as_str().to_owned(),
            source: pack
                .path()
                .strip_prefix(root)
                .map_err(|_| Error::Storage)?
                .to_path_buf(),
            layout: Layout::open(pack.path(), pack.into(), cancelled)?,
        };
        result.verify_input(cancelled)?;
        observe_git_base_pack(pack.path(), pack.into(), limits, cancelled)?;
        result.verify_input(cancelled)?;
        Ok(result)
    }

    fn verify_root(&self) -> Result<(), Error> {
        same_inode(&self.root, &directory(&self.path)?)
    }

    fn verify_input(&self, cancelled: &impl Fn() -> bool) -> Result<(), Error> {
        self.verify_root()?;
        let input = File::from(
            openat2(
                &self.root,
                &self.source,
                OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                CONFINED,
            )
            .map_err(|_| Error::Storage)?,
        );
        same_inode(&input, self.layout.root())?;
        self.layout.recheck(cancelled)?;
        Ok(())
    }
}

impl Slot {
    fn open(binding: &Binding) -> Result<Option<Self>, Error> {
        binding.verify_root()?;
        let file = match openat2(
            &binding.root,
            &binding.digest,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            CONFINED,
        ) {
            Ok(file) => File::from(file),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(_) => return Err(Error::DestinationExists),
        };
        let metadata = file.metadata().map_err(|_| Error::Storage)?;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o7777 != 0o700
            || metadata.nlink() == 0
        {
            return Err(Error::DestinationExists);
        }
        Ok(Some(Self(file)))
    }

    fn verify(&self, binding: &Binding, complete: bool) -> Result<(), Error> {
        let current = Self::open(binding)?.ok_or(Error::DestinationExists)?;
        same_inode(&self.0, &current.0)?;
        let mut count = 0;
        for entry in Dir::read_from(&reopen(&self.0)?).map_err(|_| Error::Storage)? {
            let entry = entry.map_err(|_| Error::Storage)?;
            let name = entry.file_name().to_bytes();
            if matches!(name, b"." | b"..") {
                continue;
            }
            if !complete || name != PACK.as_bytes() || count != 0 {
                return Err(Error::DestinationExists);
            }
            count += 1;
        }
        if count != usize::from(complete) {
            return Err(Error::DestinationExists);
        }
        Ok(())
    }
}

fn prepare_slot(
    binding: &Binding,
    cancelled: &impl Fn() -> bool,
    sync: &mut impl FnMut(SyncPoint, &File) -> Result<(), Error>,
) -> Result<Slot, Error> {
    let slot = Slot::open(binding)?.ok_or(Error::DestinationExists)?;
    for (point, file) in [(SyncPoint::Directory, &slot.0), (SyncPoint::Parent, &binding.root)] {
        binding.verify_input(cancelled)?;
        slot.verify(binding, false)?;
        sync(point, &reopen(file)?)?;
    }
    binding.verify_input(cancelled)?;
    slot.verify(binding, false)?;
    staging::check_cancel(cancelled)?;
    Ok(slot)
}

fn finish(
    binding: &Binding,
    slot: &Slot,
    pack: &ReceivedGitPack,
    cancelled: &impl Fn() -> bool,
    sync: &mut impl FnMut(SyncPoint, &File) -> Result<(), Error>,
) -> Result<(), Error> {
    let layout = Layout::open(pack.path(), pack.into(), cancelled)?;
    for (point, file) in [
        (SyncPoint::PublishedRoot, layout.root()),
        (SyncPoint::Directory, &slot.0),
        (SyncPoint::Parent, &binding.root),
    ] {
        slot.verify(binding, true)?;
        layout.matches_relocated(&binding.layout)?;
        staging::check_cancel(cancelled)?;
        sync(point, &reopen(file)?)?;
        layout.recheck(cancelled)?;
    }
    slot.verify(binding, true)?;
    layout.matches_relocated(&binding.layout)?;
    staging::check_cancel(cancelled)?;
    Ok(())
}

fn existing_pack(
    binding: &Binding,
    slot: &Slot,
    incoming: ReceivedGitPack,
    limits: PackReceiveLimits,
    cancelled: &impl Fn() -> bool,
    sync: &mut impl FnMut(SyncPoint, &File) -> Result<(), Error>,
) -> Result<Outcome, Failure> {
    let result = (|| {
        slot.verify(binding, true)?;
        let layout = Layout::open(&binding.destination, (&incoming).into(), cancelled)?;
        let pack = observe_git_base_pack(&binding.destination, (&incoming).into(), limits, cancelled)?;
        layout.recheck(cancelled)?;
        synchronize_layout(&layout, cancelled, &mut |point, file| {
            slot.verify(binding, true)?;
            binding.verify_input(cancelled)?;
            sync(point, file)
        })?;
        for (point, file) in [(SyncPoint::Directory, &slot.0), (SyncPoint::Parent, &binding.root)] {
            staging::check_cancel(cancelled)?;
            sync(point, &reopen(file)?)?;
            slot.verify(binding, true)?;
            binding.verify_input(cancelled)?;
            layout.recheck(cancelled)?;
        }
        staging::check_cancel(cancelled)?;
        Ok(pack)
    })();
    match result {
        Ok(pack) => Ok(Outcome::Existing {
            unused_incoming: (incoming.path() != pack.path()).then_some(incoming),
            pack,
        }),
        Err(reason) => Err(retained(reason, incoming, Some(binding.destination.clone()))),
    }
}

fn retained(reason: Error, pack: ReceivedGitPack, destination: Option<PathBuf>) -> Failure {
    Failure::Retained {
        reason,
        pack: Box::new(pack),
        destination,
    }
}

fn same_inode(left: &File, right: &File) -> Result<(), Error> {
    let left = left.metadata().map_err(|_| Error::Storage)?;
    let right = right.metadata().map_err(|_| Error::Storage)?;
    if (left.dev(), left.ino()) == (right.dev(), right.ino()) {
        Ok(())
    } else {
        Err(Error::Storage)
    }
}

pub(super) fn claim(root: &File, name: &str) -> rustix::io::Result<()> {
    mkdirat(root, name, PRIVATE)
}

pub(super) fn rename(binding: &Binding, slot: &Slot) -> rustix::io::Result<()> {
    renameat(&binding.root, &binding.source, &slot.0, PACK)
}

#[cfg(test)]
mod tests;
