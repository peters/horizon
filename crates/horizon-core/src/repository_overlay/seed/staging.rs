use super::{
    GitObjectSource, PreparedGitSeed, ResolvedRepositoryOverlay, SeedError as Error, SeedFailure, import, index,
};
use crate::repository_overlay::reader::SelectedRepositoryReader;
use git2::{Config, Repository, RepositoryInitOptions};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

pub(super) fn prepare(
    parent: &Path,
    resolved: &ResolvedRepositoryOverlay,
    source: &mut impl GitObjectSource,
    cancelled: &impl Fn() -> bool,
) -> Result<PreparedGitSeed, SeedFailure> {
    let reservation = reserve(parent, cancelled).map_err(|reason| SeedFailure { reason, residue: None })?;
    let result = populate(&reservation, resolved, source, cancelled);
    match result {
        Ok(objects) => Ok(PreparedGitSeed {
            path: reservation,
            base_commit: resolved.base_commit(),
            objects,
        }),
        Err(reason) => Err(SeedFailure {
            reason,
            residue: Some(reservation),
        }),
    }
}

pub(super) fn reserve(parent: &Path, cancelled: &impl Fn() -> bool) -> Result<std::path::PathBuf, Error> {
    check_cancel(cancelled)?;
    let _parent = private_parent(parent)?;
    // Stable ancestry and exclusive same-user ownership are caller preconditions.
    // Keep immediately: a failed seed is evidence, never an automatic recursive delete.
    tempfile::Builder::new()
        .prefix("repository-seed-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(parent)
        .map(tempfile::TempDir::keep)
        .map_err(|_| Error::Storage)
}

pub(super) fn private_parent(parent: &Path) -> Result<SelectedRepositoryReader, Error> {
    if !parent.is_absolute() {
        return Err(Error::UnsafeParent);
    }
    let reader = SelectedRepositoryReader::open(parent).map_err(|_| Error::UnsafeParent)?;
    let metadata = reader.root.handle().metadata().map_err(|_| Error::UnsafeParent)?;
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o700
        || !metadata.is_dir()
        || metadata.nlink() == 0
    {
        return Err(Error::UnsafeParent);
    }
    Ok(reader)
}

fn populate(
    path: &Path,
    resolved: &ResolvedRepositoryOverlay,
    source: &mut impl GitObjectSource,
    cancelled: &impl Fn() -> bool,
) -> Result<usize, Error> {
    check_cancel(cancelled)?;
    let mut options = RepositoryInitOptions::new();
    options.no_reinit(true).external_template(false).initial_head("seed");
    let repository = Repository::init_opts(path, &options).map_err(|_| Error::Storage)?;
    let mut config = Config::open(&repository.path().join("config")).map_err(|_| Error::Storage)?;
    config.set_bool("core.ignorecase", false).map_err(|_| Error::Storage)?;
    config.set_bool("core.filemode", true).map_err(|_| Error::Storage)?;
    repository.set_config(&config).map_err(|_| Error::Storage)?;
    let database = repository.odb().map_err(|_| Error::Storage)?;
    let mut importer = import::Importer::new(&database, source, cancelled);
    importer.base(&repository, resolved)?;
    index::write(&repository, resolved, &mut importer)?;
    check_cancel(cancelled)?;
    let commit = format!("{}\n", resolved.base_commit());
    fs::write(repository.path().join("shallow"), &commit).map_err(|_| Error::Storage)?;
    fs::write(repository.path().join("HEAD"), &commit).map_err(|_| Error::Storage)?;
    let reopened = Repository::open(path).map_err(|_| Error::Storage)?;
    if !reopened.head_detached().map_err(|_| Error::Storage)?
        || !reopened.is_shallow()
        || reopened.head().map_err(|_| Error::Storage)?.target() != Some(resolved.base_commit())
    {
        return Err(Error::Object);
    }
    Ok(importer.objects())
}

pub(super) fn check_cancel(cancelled: &impl Fn() -> bool) -> Result<(), Error> {
    if cancelled() { Err(Error::Cancelled) } else { Ok(()) }
}
