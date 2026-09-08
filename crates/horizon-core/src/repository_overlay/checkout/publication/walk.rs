use super::{
    PublicationError as Error,
    linux::{SyncPoint, check_cancel, pin, reopen},
};
use crate::repository_overlay::{
    MAX_CHANGES, MAX_CONTENT_BYTES, MAX_METADATA_BYTES, checkout::files::same, paths, seed,
};
use rustix::fs::{Dir, readlinkat_raw};
use std::{
    ffi::OsStr,
    fs::{File, Metadata},
    os::unix::fs::MetadataExt,
};

// Working leaves/implicit directories, seed objects, 256 fan-out directories and
// conservative fixed Git layout overhead, including the checkout root itself.
pub(super) const MAX_NODES: usize = MAX_CHANGES + seed::MAX_OBJECTS + 256 + 64;
const MAX_PATHS: usize = MAX_METADATA_BYTES + seed::MAX_OBJECTS * 53 + 64 * paths::MAX_PATH_BYTES;
// Working bytes and compressed seed bytes are separate. Loose zlib overhead has
// per-object slack as well as proportional slack; index/fixed metadata is additional.
pub(super) const MAX_BYTES: u64 = MAX_CONTENT_BYTES
    + seed::MAX_BYTES
    + seed::MAX_BYTES / 100
    + seed::MAX_OBJECTS as u64 * 65_536
    + 2 * MAX_METADATA_BYTES as u64;

#[derive(Default)]
pub(super) struct Budget {
    pub nodes: usize,
    pub paths: usize,
    pub bytes: u64,
}

impl Budget {
    pub(super) fn charge(&mut self, path: usize, link: usize, bytes: u64) -> Result<(), Error> {
        self.nodes = self
            .nodes
            .checked_add(1)
            .filter(|n| *n <= MAX_NODES)
            .ok_or(Error::Limit)?;
        self.paths = self
            .paths
            .checked_add(path)
            .and_then(|n| n.checked_add(link))
            .filter(|n| *n <= MAX_PATHS)
            .ok_or(Error::Limit)?;
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|n| *n <= MAX_BYTES)
            .ok_or(Error::Limit)?;
        Ok(())
    }
}

struct Node {
    path: String,
    metadata: Metadata,
}

pub(super) fn synchronize(
    root: &File,
    cancelled: &impl Fn() -> bool,
    sync: &mut impl FnMut(SyncPoint, &File) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut budget = Budget::default();
    let first = node(root, ".", &mut budget)?;
    let mut nodes = vec![first];
    let mut cursor = 0;
    while cursor < nodes.len() {
        check_cancel(cancelled)?;
        if nodes[cursor].metadata.is_dir() {
            children(root, cursor, &mut nodes, &mut budget, cancelled)?;
        }
        cursor += 1;
    }
    for node in nodes.iter().filter(|node| node.metadata.is_file()) {
        synchronize_node(root, node, SyncPoint::File, cancelled, sync)?;
    }
    // Breadth-first collection gives reverse parent order without sorting or recursion.
    for node in nodes.iter().rev().filter(|node| node.metadata.is_dir()) {
        synchronize_node(root, node, SyncPoint::Directory, cancelled, sync)?;
    }
    for node in &nodes {
        check_cancel(cancelled)?;
        verify(root, node)?;
    }
    Ok(())
}

fn children(
    root: &File,
    cursor: usize,
    nodes: &mut Vec<Node>,
    budget: &mut Budget,
    cancelled: &impl Fn() -> bool,
) -> Result<(), Error> {
    let parent = &nodes[cursor];
    let directory = reopen(&verify(root, parent)?)?;
    let prefix = if cursor == 0 {
        String::new()
    } else {
        format!("{}/", parent.path)
    };
    for entry in Dir::read_from(&directory).map_err(|_| Error::Storage)? {
        check_cancel(cancelled)?;
        let entry = entry.map_err(|_| Error::Storage)?;
        let name = entry.file_name().to_str().map_err(|_| Error::UnsafeNode)?;
        if matches!(name, "." | "..") {
            continue;
        }
        let length = prefix.len().checked_add(name.len()).ok_or(Error::Limit)?;
        if length > paths::MAX_PATH_BYTES
            || budget.nodes >= MAX_NODES
            || budget.paths.checked_add(length).is_none_or(|n| n > MAX_PATHS)
        {
            return Err(Error::Limit);
        }
        nodes.push(node(root, &format!("{prefix}{name}"), budget)?);
    }
    verify(root, &nodes[cursor])?;
    Ok(())
}

fn node(root: &File, path: &str, budget: &mut Budget) -> Result<Node, Error> {
    let file = pin(root, OsStr::new(path))?;
    let metadata = file.metadata().map_err(|_| Error::Storage)?;
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.nlink() == 0
        || !(metadata.is_file() || metadata.is_dir() || metadata.is_symlink())
        || (!metadata.is_dir() && metadata.nlink() != 1)
    {
        return Err(Error::UnsafeNode);
    }
    let git = path == ".git" || path.starts_with(".git/");
    if path != "." && !git {
        paths::validate(path).map_err(|_| Error::UnsafeNode)?;
    }
    let link = if metadata.is_symlink() {
        if git {
            return Err(Error::UnsafeNode);
        }
        let mut buffer = [0; paths::MAX_PATH_BYTES + 1];
        let bytes = readlinkat_raw(&file, "", &mut buffer[..]).map_err(|_| Error::Storage)?;
        let target = std::str::from_utf8(&buffer[..bytes]).map_err(|_| Error::UnsafeNode)?;
        paths::validate_link(path, target).map_err(|_| Error::UnsafeNode)?;
        bytes
    } else {
        0
    };
    budget.charge(path.len(), link, if metadata.is_file() { metadata.len() } else { 0 })?;
    Ok(Node {
        path: path.to_owned(),
        metadata,
    })
}

fn verify(root: &File, node: &Node) -> Result<File, Error> {
    let current = pin(root, OsStr::new(&node.path))?;
    if !same(&node.metadata, &current.metadata().map_err(|_| Error::Storage)?) {
        return Err(Error::UnsafeNode);
    }
    Ok(current)
}

fn synchronize_node(
    root: &File,
    node: &Node,
    point: SyncPoint,
    cancelled: &impl Fn() -> bool,
    sync: &mut impl FnMut(SyncPoint, &File) -> Result<(), Error>,
) -> Result<(), Error> {
    check_cancel(cancelled)?;
    let file = reopen(&verify(root, node)?)?;
    if !same(&node.metadata, &file.metadata().map_err(|_| Error::Storage)?) {
        return Err(Error::UnsafeNode);
    }
    sync(point, &file)?;
    verify(root, node)?;
    Ok(())
}
