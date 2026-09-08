use super::super::SetupInputError as Error;
use super::{CONFINED, File, Mode, OFlags, SelectedRepositoryReader, SetupIntent, openat2};
use rustix::fs::Dir;
use std::{
    fs::Metadata,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

const MAX_NODES: usize = 262_144;
const MAX_PATH_BYTES: usize = 16 * 1024 * 1024;

pub(super) fn validate(retained: &File, intent: &SetupIntent) -> Result<(), Error> {
    let identity = retained.metadata().map_err(|_| Error::Unverified)?;
    for source in [&intent.objects_directory, &intent.bundle_store] {
        separate(source, &identity, MAX_NODES)?;
    }
    Ok(())
}

fn same(left: &Metadata, right: &Metadata) -> bool {
    (left.dev(), left.ino()) == (right.dev(), right.ino())
}

fn separate(source: &Path, retained: &Metadata, limit: usize) -> Result<(), Error> {
    let reader = SelectedRepositoryReader::open(source).map_err(|_| Error::Unverified)?;
    let identity = reader.root.handle().metadata().map_err(|_| Error::Unverified)?;
    let mut pending = vec![PathBuf::from(".")];
    let (mut nodes, mut bytes) = (1usize, 1usize);
    while let Some(relative) = pending.pop() {
        if nodes > limit || relative.components().count() > 64 {
            return Err(Error::Unverified);
        }
        let node = File::from(
            openat2(
                reader.root.handle(),
                &relative,
                OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
                CONFINED,
            )
            .map_err(|_| Error::Unverified)?,
        );
        let metadata = node.metadata().map_err(|_| Error::Unverified)?;
        if metadata.nlink() == 0 || !(metadata.is_file() || metadata.is_dir()) || same(&metadata, retained) {
            return Err(Error::Unverified);
        }
        if !metadata.is_dir() {
            continue;
        }
        let directory = File::from(
            openat2(
                &node,
                ".",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                CONFINED,
            )
            .map_err(|_| Error::Unverified)?,
        );
        for entry in Dir::read_from(&directory).map_err(|_| Error::Unverified)? {
            let entry = entry.map_err(|_| Error::Unverified)?;
            let name = entry.file_name().to_str().map_err(|_| Error::Unverified)?;
            if matches!(name, "." | "..") {
                continue;
            }
            let child = relative.join(name);
            nodes = nodes
                .checked_add(1)
                .filter(|count| *count <= limit)
                .ok_or(Error::Unverified)?;
            bytes = bytes
                .checked_add(child.as_os_str().len())
                .filter(|count| *count <= MAX_PATH_BYTES)
                .ok_or(Error::Unverified)?;
            pending.push(child);
        }
    }
    let current = SelectedRepositoryReader::open(source).map_err(|_| Error::Unverified)?;
    if !same(
        &identity,
        &current.root.handle().metadata().map_err(|_| Error::Unverified)?,
    ) {
        return Err(Error::Unverified);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::{directory, private};
    use super::*;
    use crate::repository_overlay::retained_setup::tests::intent;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };

    #[test]
    fn exact_nested_alias_and_unsafe_input_roots_are_read_only_rejections() {
        let source = private();
        let other = private();
        fs::write(source.path().join("sentinel"), b"unchanged").unwrap();
        let nested = source.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o700)).unwrap();
        for root in [source.path(), &nested] {
            for bundle in [false, true] {
                let mut expected = intent();
                expected.objects_directory = if bundle { other.path() } else { source.path() }.into();
                expected.bundle_store = if bundle { source.path() } else { other.path() }.into();
                assert_eq!(directory(root).check_inputs(&expected), Err(Error::Unverified));
                assert!(!root.join("setup-claim.json").exists() && !root.join("setup-data").exists());
            }
        }
        let retained = File::open(other.path()).unwrap().metadata().unwrap();
        assert_eq!(separate(source.path(), &retained, MAX_NODES), Ok(()));
        assert_eq!(separate(source.path(), &retained, 0), Err(Error::Unverified));
        let alias = other.path().join("alias");
        symlink(source.path(), &alias).unwrap();
        assert_eq!(separate(&alias, &retained, MAX_NODES), Err(Error::Unverified));
        assert_eq!(
            separate(&source.path().join("missing"), &retained, MAX_NODES),
            Err(Error::Unverified)
        );
        assert_eq!(fs::read(source.path().join("sentinel")).unwrap(), b"unchanged");
        assert_eq!(fs::read_dir(source.path()).unwrap().count(), 2);
        assert_eq!(fs::read_dir(nested).unwrap().count(), 0);
    }
}
