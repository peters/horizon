//! Standard GitHub LFS hydration, only inside a newly claimed private checkout.
use super::{GitPreparation, GitPreparationError as Error, git::Commands};
use crate::{
    cloud_run::ArtifactDigest,
    repository_overlay::reader::{SelectedRepositoryNode, SelectedRepositoryReader},
};
use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::Path,
};

pub(super) const MAX_OBJECT: usize = 64 * 1024 * 1024;
const MAX_TOTAL: usize = 512 * 1024 * 1024;
const MAX_PATHS: usize = 1024;
const PREFIX: &str = "version https://git-lfs.github.com/spec/v1\n";

pub(super) struct Pointer {
    pub path: String,
    pub encoded: Vec<u8>,
    pub size: usize,
    digest: ArtifactDigest,
}

impl Pointer {
    fn parse(path: String, encoded: Vec<u8>) -> Result<Self, Error> {
        let raw = std::str::from_utf8(&encoded).map_err(|_| Error::UnsupportedRepository)?;
        let (oid, size) = raw
            .strip_prefix(PREFIX)
            .and_then(|value| value.strip_prefix("oid sha256:"))
            .and_then(|value| value.split_once("\nsize "))
            .ok_or(Error::UnsupportedRepository)?;
        let size = size.strip_suffix('\n').ok_or(Error::UnsupportedRepository)?;
        let parsed: usize = size.parse().map_err(|_| Error::UnsupportedRepository)?;
        if parsed > MAX_OBJECT
            || size != parsed.to_string()
            || oid.len() != 64
            || !oid.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::UnsupportedRepository);
        }
        let digest = ArtifactDigest::parse_sha256(oid).map_err(|_| Error::UnsupportedRepository)?;
        Ok(Self {
            path,
            encoded,
            size: parsed,
            digest,
        })
    }

    fn verify_bytes(&self, bytes: &[u8]) -> Result<(), Error> {
        if bytes.len() != self.size || ArtifactDigest::sha256(bytes) != self.digest {
            return Err(Error::Git);
        }
        Ok(())
    }
}

fn plan(directory: &Path, attributes: &[u8], matches: &[u8], commit: &str) -> Result<Vec<Pointer>, Error> {
    if !attributes.is_empty() && !attributes.ends_with(&[0]) {
        return Err(Error::UnsupportedRepository);
    }
    let fields: Vec<_> = attributes.split(|b| *b == 0).collect();
    let fields = &fields[..fields.len() - 1];
    if fields.len() % 3 != 0 {
        return Err(Error::UnsupportedRepository);
    }
    let reader = SelectedRepositoryReader::open(directory).map_err(|_| Error::UnsafeRoot)?;
    let mut pointers = Vec::new();
    let mut total = 0usize;
    let mut names = std::collections::BTreeSet::new();
    for fields in fields.as_chunks::<3>().0 {
        if fields[1] != b"filter" || !names.insert(fields[0]) {
            return Err(Error::UnsupportedRepository);
        }
        match fields[2] {
            b"unspecified" | b"unset" => continue,
            b"lfs" => {}
            _ => return Err(Error::UnsupportedRepository),
        }
        if pointers.len() == MAX_PATHS {
            return Err(Error::UnsupportedRepository);
        }
        let path = std::str::from_utf8(fields[0])
            .map_err(|_| Error::UnsupportedRepository)?
            .to_owned();
        let SelectedRepositoryNode::File { bytes, .. } =
            reader.read(&path, 1024).map_err(|_| Error::UnsupportedRepository)?
        else {
            return Err(Error::UnsupportedRepository);
        };
        let pointer = Pointer::parse(path, bytes)?;
        total = total
            .checked_add(pointer.size)
            .filter(|total| *total <= MAX_TOTAL)
            .ok_or(Error::UnsupportedRepository)?;
        pointers.push(pointer);
    }
    // Pointer-looking tracked data without the LFS attribute is not silently left unhydrated.
    for matched in matches.split(|b| *b == 0).filter(|value| !value.is_empty()) {
        let path = matched
            .strip_prefix(format!("{commit}:").as_bytes())
            .ok_or(Error::UnsupportedRepository)?;
        if !pointers.iter().any(|p| p.path.as_bytes() == path) {
            return Err(Error::UnsupportedRepository);
        }
    }
    Ok(pointers)
}

fn block_logs(directory: &Path) -> Result<(), Error> {
    let root = directory.join(".git/lfs");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .map_err(|_| Error::Storage)?;
    // git-lfs v3 writes panic/download logs under LocalLogDir. A regular file
    // makes mkdir/log creation fail; stderr is discarded by the command runner.
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join("logs"))
        .map_err(|_| Error::Storage)?;
    Ok(())
}

fn replace_pointer(directory: &Path, pointer: &Pointer, bytes: &[u8]) -> Result<(), Error> {
    pointer.verify_bytes(bytes)?;
    let parent = File::open(directory).map_err(|_| Error::UnsafeRoot)?;
    let mut file = File::from(
        openat2(
            &parent,
            pointer.path.as_str(),
            OFlags::RDWR | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(|_| Error::UnsafeRoot)?,
    );
    let metadata = file.metadata().map_err(|_| Error::Storage)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7022 != 0
    {
        return Err(Error::UnsafeRoot);
    }
    let mut current = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(&mut file, 1025), &mut current).map_err(|_| Error::Storage)?;
    if current != pointer.encoded {
        return Err(Error::Conflict);
    }
    std::io::Seek::rewind(&mut file).map_err(|_| Error::Storage)?;
    file.set_len(0)
        .and_then(|()| file.write_all(bytes))
        .and_then(|()| file.sync_all())
        .map_err(|_| Error::Storage)?;
    Ok(())
}

pub(super) fn hydrate(
    git: &mut impl Commands,
    directory: &Path,
    request: &GitPreparation,
    attributes: &[u8],
    matches: &[u8],
    cancelled: &dyn Fn() -> bool,
    verify: &dyn Fn() -> Result<(), Error>,
) -> Result<(), Error> {
    verify()?;
    let pointers = plan(directory, attributes, matches, request.source.commit.as_str())?;
    verify()?;
    if pointers.is_empty() {
        return Ok(());
    }
    let version = git.run(directory, &["lfs", "version"], &[], false, cancelled)?;
    verify()?;
    let version = std::str::from_utf8(&version).map_err(|_| Error::Git)?;
    let minor = version
        .strip_prefix("git-lfs/3.")
        .and_then(|v| v.split('.').next())
        .and_then(|v| v.parse::<u32>().ok());
    if minor.is_none_or(|minor| minor < 3) {
        return Err(Error::UnsupportedRepository);
    }
    block_logs(directory)?;
    for pointer in &pointers {
        verify()?;
        let bytes = git.smudge(directory, request, pointer, cancelled)?;
        verify()?;
        replace_pointer(directory, pointer, &bytes)?;
    }
    finish_index(git, directory, request, &pointers, cancelled, verify)
}

fn finish_index(
    git: &mut impl Commands,
    directory: &Path,
    request: &GitPreparation,
    pointers: &[Pointer],
    cancelled: &dyn Fn() -> bool,
    verify: &dyn Fn() -> Result<(), Error>,
) -> Result<(), Error> {
    // Ordinary task-side Git must clean hydrated bytes back to their index pointers.
    // Only fixed packaged commands are installed, never a repository-supplied filter.
    for (key, value) in [
        ("filter.lfs.clean", "/usr/bin/git-lfs clean -- %f"),
        ("filter.lfs.smudge", "/usr/bin/git-lfs smudge -- %f"),
        ("filter.lfs.process", "/usr/bin/git-lfs filter-process"),
        ("filter.lfs.required", "true"),
    ] {
        verify()?;
        git.run(directory, &["config", "--local", key, value], &[], false, cancelled)?;
        verify()?;
    }
    let mut run = |args: &[&str], input: &[u8]| {
        let mut configured = vec![
            "--literal-pathspecs",
            "-c",
            "filter.lfs.process=/usr/bin/git-lfs filter-process",
            "-c",
            "filter.lfs.smudge=/usr/bin/git-lfs smudge -- %f",
            "-c",
            "filter.lfs.required=true",
        ];
        configured.extend_from_slice(args);
        verify()?;
        let output = git.run(directory, &configured, input, false, cancelled)?;
        verify()?;
        Ok::<_, Error>(output)
    };
    let mut paths = Vec::new();
    for pointer in pointers {
        paths.extend_from_slice(pointer.path.as_bytes());
        paths.push(0);
    }
    // Refresh the fresh index's filtered-file stat entries, then prove the entire
    // index still represents the exact fetched tree. This never runs on old claims.
    run(&["add", "--pathspec-from-file=-", "--pathspec-file-nul"], &paths)?;
    run(
        &[
            "diff-index",
            "--cached",
            "--quiet",
            request.source.commit.as_str(),
            "--",
        ],
        &[],
    )?;
    if !run(&["status", "--porcelain=v1", "-z", "--untracked-files=all"], &[])?.is_empty() {
        return Err(Error::Git);
    }
    verify()
}

#[cfg(test)]
mod tests;
