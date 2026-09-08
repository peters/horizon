use super::{
    GitObjectSource, PreparedPrivateCheckout, PrivateCheckoutError as Error, PrivateCheckoutFailure,
    ResolvedRepositoryOverlay, files::Root, loose,
};
use crate::repository_overlay::{
    namespace::{NamespaceEntry, NamespaceFile},
    seed::prepare_git_seed,
};
use git2::{ObjectType, Oid};
use std::{collections::BTreeSet, io::Write, path::Path};

pub(super) fn prepare(
    parent: &Path,
    resolved: &ResolvedRepositoryOverlay,
    source: &mut impl GitObjectSource,
    cancelled: &impl Fn() -> bool,
) -> Result<PreparedPrivateCheckout, PrivateCheckoutFailure> {
    let seed = prepare_git_seed(parent, resolved, source, cancelled).map_err(|failure| PrivateCheckoutFailure {
        reason: failure.reason.into(),
        residue: failure.residue().map(Path::to_path_buf),
    })?;
    let path = seed.path();
    write(path, resolved, cancelled).map_err(|reason| PrivateCheckoutFailure {
        reason,
        residue: Some(path.to_owned()),
    })?;
    Ok(PreparedPrivateCheckout {
        path: path.to_owned(),
        base_commit: seed.base_commit(),
        manifest_sha256: resolved.bundle().manifest_sha256().clone(),
    })
}

pub(super) fn write(
    path: &Path,
    resolved: &ResolvedRepositoryOverlay,
    cancelled: &impl Fn() -> bool,
) -> Result<(), Error> {
    check_cancel(cancelled)?;
    let root = Root::open(path)?;
    for directory in directories(resolved.working_tree().entries().map(|(path, _)| path), cancelled)? {
        check_cancel(cancelled)?;
        root.directory(directory)?;
    }
    for (path, entry) in resolved.working_tree().entries() {
        if let NamespaceEntry::File { source, executable } = entry {
            check_cancel(cancelled)?;
            let mut output = root.create(path)?;
            let id = match source {
                NamespaceFile::Base { object, bytes } => {
                    let hex = object.to_string();
                    let input = root.read_regular(&format!(".git/objects/{}/{}", &hex[..2], &hex[2..]))?;
                    loose::copy(&input, *bytes, &mut output, cancelled)?;
                    *object
                }
                NamespaceFile::Overlay { sha256, bytes } => {
                    let payload = resolved
                        .bundle()
                        .blob(sha256)
                        .filter(|data| data.len() as u64 == *bytes)
                        .ok_or(Error::Object)?;
                    for chunk in payload.chunks(16 * 1024) {
                        check_cancel(cancelled)?;
                        output.write_all(chunk).map_err(|_| Error::Storage)?;
                    }
                    Oid::hash_object(ObjectType::Blob, payload).map_err(|_| Error::Object)?
                }
            };
            check_cancel(cancelled)?;
            root.verify_file(path, &output, source.bytes(), *executable, id)?;
        }
    }
    for (path, entry) in resolved.working_tree().entries() {
        if let NamespaceEntry::Symlink { target } = entry {
            check_cancel(cancelled)?;
            root.symlink(path, target)?;
        }
    }
    check_cancel(cancelled)?;
    root.verify(path)
}

pub(super) fn directories<'a>(
    paths: impl Iterator<Item = &'a str>,
    cancelled: &impl Fn() -> bool,
) -> Result<BTreeSet<&'a str>, Error> {
    let mut directories = BTreeSet::new();
    for mut path in paths {
        check_cancel(cancelled)?;
        while let Some((parent, _)) = path.rsplit_once('/') {
            check_cancel(cancelled)?;
            if !directories.insert(parent) {
                break;
            }
            path = parent;
        }
    }
    Ok(directories)
}

pub(super) fn check_cancel(cancelled: &impl Fn() -> bool) -> Result<(), Error> {
    if cancelled() { Err(Error::Cancelled) } else { Ok(()) }
}
