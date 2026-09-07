use super::{
    GitCaptureError as Error,
    content::{Contents, GitNode},
};
use crate::{
    cloud_run::GitSource,
    repository_overlay::{
        RepositoryOverlayPlan,
        bundle::RepositoryOverlayBundle,
        reader::{MAX_READ_BYTES, RepositoryReadError, SelectedRepositoryReader},
    },
};
use git2::{ErrorCode, IndexEntryExtendedFlag, ObjectFormat, ObjectType, Oid, Repository, RepositoryOpenFlags, Tree};
use std::{
    collections::{BTreeMap, BTreeSet},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::Path,
};

pub(super) fn capture(root: &Path, source: GitSource, selected: &[&str]) -> Result<RepositoryOverlayBundle, Error> {
    let state = State::open(root, &source)?;
    let baseline = state.selected_index(selected)?;
    let commit = state.repository.find_commit(state.base).map_err(|_| Error::GitRead)?;
    let tree = commit.tree().map_err(|_| Error::GitRead)?;
    let mut contents = Contents::default();
    let mut index_changes = Vec::new();
    let mut working_changes = Vec::new();
    for (path, indexed) in selected.iter().zip(&baseline) {
        let original = tree_node(&tree, path)?;
        if original != *indexed {
            let node = indexed.map(|node| node.read(&state.repository)).transpose()?;
            index_changes.push(contents.change(path, node)?);
        }
        let working = match state.reader.read(path, MAX_READ_BYTES) {
            Ok(node) => Some(node),
            Err(RepositoryReadError::Missing) => None,
            Err(error) => return Err(error.into()),
        };
        let same = match (*indexed, &working) {
            (None, None) => true,
            (Some(node), Some(working)) => node.matches(working)?,
            _ => false,
        };
        if !same {
            working_changes.push(contents.change(path, working)?);
        }
    }
    state.verify(selected, &baseline)?;
    contents.finish(RepositoryOverlayPlan::new(source, index_changes, working_changes)?)
}

pub(super) struct State {
    reader: SelectedRepositoryReader,
    repository: Repository,
    base: Oid,
}

impl State {
    pub(super) fn open(root: &Path, source: &GitSource) -> Result<Self, Error> {
        let reader = SelectedRepositoryReader::open(root)?;
        let pinned = format!("/proc/self/fd/{}", reader.root.handle().as_raw_fd());
        let repository = Repository::open_ext(&pinned, RepositoryOpenFlags::NO_SEARCH, &[] as &[&str])
            .map_err(|_| Error::Repository)?;
        if repository.is_bare() || repository.object_format() != ObjectFormat::Sha1 {
            return Err(Error::Repository);
        }
        let base = Oid::from_str(source.commit.as_str()).map_err(|_| Error::BaseMismatch)?;
        let state = Self {
            reader,
            repository,
            base,
        };
        state.verify_root()?;
        state.verify_head()?;
        Ok(state)
    }

    fn verify_root(&self) -> Result<(), Error> {
        let root = self.reader.root.handle().metadata().map_err(|_| Error::Repository)?;
        let path = self.repository.workdir().ok_or(Error::Repository)?;
        let working = path.metadata().map_err(|_| Error::Repository)?;
        if root.dev() != working.dev() || root.ino() != working.ino() {
            return Err(Error::Repository);
        }
        Ok(())
    }

    fn verify_head(&self) -> Result<(), Error> {
        let head = self
            .repository
            .head()
            .and_then(|head| head.peel_to_commit())
            .map_err(|_| Error::BaseMismatch)?;
        if head.id() != self.base {
            return Err(Error::BaseMismatch);
        }
        Ok(())
    }

    pub(super) fn selected_index(&self, selected: &[&str]) -> Result<Vec<Option<GitNode>>, Error> {
        let mut index = self.repository.index().map_err(|_| Error::GitRead)?;
        index.read(true).map_err(|_| Error::GitRead)?;
        if index.has_conflicts() {
            return Err(Error::UnsupportedIndex);
        }
        let wanted: BTreeMap<_, _> = selected
            .iter()
            .enumerate()
            .map(|(position, path)| (path.as_bytes(), position))
            .collect();
        let ancestors: BTreeSet<_> = selected
            .iter()
            .flat_map(|path| parents(path).map(str::as_bytes))
            .collect();
        let mut found = vec![None; selected.len()];
        // Git's by-path index lookup can fold case. Only literal selected bytes grant access.
        for entry in index.iter() {
            if entry.mode == 0o040_000
                || IndexEntryExtendedFlag::from_bits_retain(entry.flags_extended)
                    .intersects(IndexEntryExtendedFlag::INTENT_TO_ADD | IndexEntryExtendedFlag::SKIP_WORKTREE)
            {
                return Err(Error::UnsupportedIndex);
            }
            if ancestors.contains(entry.path.as_slice()) {
                return Err(Error::UnsupportedNode);
            }
            if let Some(position) = wanted.get(entry.path.as_slice()) {
                let node = GitNode::new(entry.mode, entry.id)?;
                if found[*position].replace(node).is_some() {
                    return Err(Error::UnsupportedIndex);
                }
            }
        }
        Ok(found)
    }

    pub(super) fn verify(&self, selected: &[&str], baseline: &[Option<GitNode>]) -> Result<(), Error> {
        self.verify_root()?;
        self.verify_head()?;
        if self.selected_index(selected)? != baseline {
            return Err(Error::Changed);
        }
        Ok(())
    }
}

fn tree_node(tree: &Tree<'_>, path: &str) -> Result<Option<GitNode>, Error> {
    for parent in parents(path) {
        match tree.get_path(Path::new(parent)) {
            Ok(entry) if entry.kind() == Some(ObjectType::Tree) => {}
            Ok(_) => return Err(Error::UnsupportedNode),
            Err(error) if error.code() == ErrorCode::NotFound => {}
            Err(_) => return Err(Error::GitRead),
        }
    }
    match tree.get_path(Path::new(path)) {
        Ok(entry) => {
            let mode = u32::try_from(entry.filemode_raw()).map_err(|_| Error::UnsupportedNode)?;
            Ok(Some(GitNode::new(mode, entry.id())?))
        }
        Err(error) if error.code() == ErrorCode::NotFound => Ok(None),
        Err(_) => Err(Error::GitRead),
    }
}

fn parents(path: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(path.rsplit_once('/').map(|(parent, _)| parent), |path| {
        path.rsplit_once('/').map(|(parent, _)| parent)
    })
}
