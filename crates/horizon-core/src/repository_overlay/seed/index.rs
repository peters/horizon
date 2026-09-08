use super::{GitObjectSource, ResolvedRepositoryOverlay, SeedError as Error, import::Importer};
use crate::repository_overlay::namespace::{NamespaceEntry, NamespaceFile};
use git2::{IndexEntry, IndexTime, ObjectType, Repository};

pub(super) fn write<S: GitObjectSource, C: Fn() -> bool>(
    repository: &Repository,
    resolved: &ResolvedRepositoryOverlay,
    importer: &mut Importer<'_, '_, S, C>,
) -> Result<(), Error> {
    let mut index = repository.index().map_err(|_| Error::Storage)?;
    if !index.is_empty() {
        return Err(Error::Storage);
    }
    for (path, entry) in resolved.index().entries() {
        let (id, mode) = match entry {
            NamespaceEntry::File { source, executable } => {
                let id = match source {
                    NamespaceFile::Base { object, bytes } => {
                        importer.object(*object, ObjectType::Blob, Some(*bytes))?
                    }
                    NamespaceFile::Overlay { sha256, .. } => {
                        importer.blob(resolved.bundle().blob(sha256).ok_or(Error::Object)?)?
                    }
                };
                (id, if *executable { 0o100_755 } else { 0o100_644 })
            }
            NamespaceEntry::Symlink { target } => (importer.blob(target.as_bytes())?, 0o120_000),
        };
        index
            .add(&IndexEntry {
                ctime: IndexTime::new(0, 0),
                mtime: IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode,
                uid: 0,
                gid: 0,
                file_size: 0,
                id,
                flags: 0,
                flags_extended: 0,
                path: path.as_bytes().to_vec(),
            })
            .map_err(|_| Error::Storage)?;
    }
    index.write().map_err(|_| Error::Storage)?;
    Ok(())
}
