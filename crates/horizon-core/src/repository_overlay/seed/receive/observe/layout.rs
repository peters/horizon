use super::{ExpectedGitPack, MAX_OBJECTS, SeedError as Error, staging, view};
use crate::repository_overlay::reader::{SelectedRepositoryReader, linux::same_metadata};
use git2::Oid;
use rustix::fs::{Dir, Mode, OFlags, ResolveFlags, openat2};
use std::{
    fs::{File, Metadata, OpenOptions},
    io::{Read, Seek, SeekFrom},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

const CONFINED: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);

struct Node {
    name: String,
    handle: File,
    metadata: Metadata,
}

pub(super) struct Layout {
    path: PathBuf,
    reader: SelectedRepositoryReader,
    nodes: Vec<Node>,
    pack_hash: Oid,
    index_bytes: u64,
    pub(super) pack_path: PathBuf,
    pub(super) index_path: PathBuf,
    pub(super) pack: File,
}

impl Layout {
    pub(super) fn open(
        path: &Path,
        expected: ExpectedGitPack<'_>,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Self, Error> {
        let reader = SelectedRepositoryReader::open(path).map_err(|_| Error::UnsafeParent)?;
        let mut nodes = vec![bind(&reader, "", true)?];
        for prefix in ["decoded", "selection"] {
            staging::check_cancel(cancelled)?;
            nodes.push(bind(&reader, prefix, true)?);
            for directory in view::INITIAL_DIRECTORIES {
                nodes.push(bind(&reader, &format!("{prefix}/{directory}"), true)?);
            }
            for (name, contents) in [
                ("HEAD", view::INITIAL_HEAD.to_owned()),
                ("config", view::INITIAL_CONFIG.to_owned()),
                ("shallow", format!("{}\n", expected.base_commit)),
            ] {
                let name = format!("{prefix}/{name}");
                nodes.push(bind(&reader, &name, false)?);
                let record = reader.root.read_private_file(&name, 128).map_err(|_| Error::Object)?;
                if record.bytes != contents.as_bytes() {
                    return Err(Error::Object);
                }
            }
        }
        let pack_directory = nodes
            .iter()
            .find(|node| node.name == "decoded/objects/pack")
            .ok_or(Error::Object)?;
        let names = children(pack_directory, 2)?;
        let (pack_name, hash) = names
            .iter()
            .find_map(|name| {
                name.strip_prefix("pack-")
                    .and_then(|tail| tail.strip_suffix(".pack"))
                    .map(|hash| (name, hash))
            })
            .ok_or(Error::Object)?;
        if hash.len() != 40
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(Error::Object);
        }
        let index_name = format!("pack-{hash}.idx");
        let pack_hash = Oid::from_str(hash).map_err(|_| Error::Object)?;
        if names.len() != 2 || !names.contains(&index_name) {
            return Err(Error::Object);
        }
        let pack_name = format!("decoded/objects/pack/{pack_name}");
        let index_name = format!("decoded/objects/pack/{index_name}");
        let pack_node = bind(&reader, &pack_name, false)?;
        let index_node = bind(&reader, &index_name, false)?;
        if pack_node.metadata.len() != expected.encoded_bytes
            || index_node.metadata.len() > MAX_OBJECTS as u64 * 40 + 2048
        {
            return Err(Error::Limit);
        }
        let pack = reopen(&pack_node.handle)?;
        let index_bytes = index_node.metadata.len();
        let result = Self {
            path: path.to_path_buf(),
            reader,
            pack_hash,
            index_bytes,
            nodes: {
                nodes.extend([pack_node, index_node]);
                nodes
            },
            pack_path: path.join(pack_name),
            index_path: path.join(index_name),
            pack,
        };
        result.recheck(cancelled)?;
        Ok(result)
    }

    pub(super) fn recheck(&self, cancelled: &impl Fn() -> bool) -> Result<(), Error> {
        let current = SelectedRepositoryReader::open(&self.path).map_err(|_| Error::UnsafeParent)?;
        let first = self.nodes.first().ok_or(Error::Object)?;
        if !same_metadata(
            &first.metadata,
            &current.root.handle().metadata().map_err(|_| Error::Storage)?,
        ) {
            return Err(Error::UnsafeParent);
        }
        for node in &self.nodes {
            staging::check_cancel(cancelled)?;
            let named = bind(&self.reader, &node.name, node.metadata.is_dir())?;
            if !same_metadata(&node.metadata, &named.metadata)
                || !same_metadata(&node.metadata, &node.handle.metadata().map_err(|_| Error::Storage)?)
            {
                return Err(Error::Object);
            }
            if node.metadata.is_dir() {
                let prefix = if node.name.is_empty() {
                    String::new()
                } else {
                    format!("{}/", node.name)
                };
                let expected = self
                    .nodes
                    .iter()
                    .filter_map(|child| {
                        child
                            .name
                            .strip_prefix(&prefix)
                            .filter(|name| !name.is_empty() && !name.contains('/'))
                    })
                    .collect::<Vec<_>>();
                let actual = children(node, expected.len())?;
                if actual.len() != expected.len() || actual.iter().any(|name| !expected.contains(&name.as_str())) {
                    return Err(Error::Object);
                }
            }
        }
        Ok(())
    }

    pub(super) fn check_pack_identity(&mut self, objects: u32) -> Result<(), Error> {
        if self.index_bytes > u64::from(objects) * 40 + 2048 {
            Err(Error::Limit)
        } else {
            let mut trailer = [0; 20];
            self.pack.seek(SeekFrom::End(-20)).map_err(|_| Error::Source)?;
            self.pack.read_exact(&mut trailer).map_err(|_| Error::Source)?;
            if trailer == self.pack_hash.as_bytes() {
                Ok(())
            } else {
                Err(Error::Object)
            }
        }
    }
}

fn bind(reader: &SelectedRepositoryReader, name: &str, directory: bool) -> Result<Node, Error> {
    let handle = File::from(
        openat2(
            reader.root.handle(),
            if name.is_empty() { "." } else { name },
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
            CONFINED,
        )
        .map_err(|_| Error::UnsafeParent)?,
    );
    let metadata = handle.metadata().map_err(|_| Error::Storage)?;
    let root = reader.root.handle().metadata().map_err(|_| Error::Storage)?;
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.dev() != root.dev()
        || metadata.nlink() == 0
        || metadata.mode() & 0o7000 != 0
    {
        return Err(Error::UnsafeParent);
    }
    // Inner modes inherit umask; the verified 0700 root and views provide privacy.
    if directory {
        if !metadata.is_dir() || (matches!(name, "" | "decoded" | "selection") && metadata.mode() & 0o7777 != 0o700) {
            return Err(Error::UnsafeParent);
        }
    } else if !metadata.is_file() || metadata.nlink() != 1 || metadata.mode() & 0o7777 != 0o600 {
        return Err(Error::UnsafeParent);
    }
    Ok(Node {
        name: name.to_owned(),
        handle,
        metadata,
    })
}

fn reopen(handle: &File) -> Result<File, Error> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(format!("/proc/self/fd/{}", handle.as_raw_fd()))
        .map_err(|_| Error::Storage)
}

fn children(node: &Node, limit: usize) -> Result<Vec<String>, Error> {
    let mut names = Vec::new();
    for entry in Dir::read_from(&reopen(&node.handle)?).map_err(|_| Error::Storage)? {
        let entry = entry.map_err(|_| Error::Storage)?;
        let name = entry.file_name().to_str().map_err(|_| Error::Object)?;
        if matches!(name, "." | "..") {
            continue;
        }
        if names.len() >= limit {
            return Err(Error::Object);
        }
        names.push(name.to_owned());
    }
    Ok(names)
}
