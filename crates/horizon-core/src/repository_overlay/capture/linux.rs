use super::{
    GitCaptureError as Error,
    content::{Contents, GitNode},
};
use crate::{
    cloud_run::GitSource,
    repository_overlay::{
        RepositoryOverlayPlan,
        bundle::RepositoryOverlayBundle,
        paths::MAX_PATH_BYTES,
        reader::{MAX_READ_BYTES, RepositoryReadError, SelectedRepositoryReader},
    },
};
use git2::{IndexEntryExtendedFlag, ObjectFormat, ObjectType, Oid, Repository, RepositoryOpenFlags, Tree};
use std::{
    collections::BTreeMap,
    ops::Bound,
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
        let original = tree_node(tree.clone(), path, |id| state.repository.find_tree(id))?;
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
    pub(super) reader: SelectedRepositoryReader,
    pub(super) repository: Repository,
    pub(super) base: Oid,
}

impl State {
    pub(super) fn open(root: &Path, source: &GitSource) -> Result<Self, Error> {
        let state = Self::open_with(root, |_| {
            Oid::from_str(source.commit.as_str()).map_err(|_| Error::BaseMismatch)
        })?;
        state.verify_head()?;
        Ok(state)
    }

    pub(super) fn open_with(
        root: &Path,
        select_base: impl FnOnce(&Repository) -> Result<Oid, Error>,
    ) -> Result<Self, Error> {
        let reader = SelectedRepositoryReader::open(root)?;
        let pinned = format!("/proc/self/fd/{}", reader.root.handle().as_raw_fd());
        let repository = Repository::open_ext(&pinned, RepositoryOpenFlags::NO_SEARCH, &[] as &[&str])
            .map_err(|_| Error::Repository)?;
        if repository.is_bare() || repository.object_format() != ObjectFormat::Sha1 {
            return Err(Error::Repository);
        }
        let base = select_base(&repository)?;
        let state = Self {
            reader,
            repository,
            base,
        };
        state.verify_root()?;
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
        let mut prefix = Vec::with_capacity(MAX_PATH_BYTES);
        let mut found = vec![None; selected.len()];
        // Git's by-path index lookup can fold case. Only literal selected bytes grant access.
        for entry in index.iter() {
            if entry.mode == 0o040_000
                || IndexEntryExtendedFlag::from_bits_retain(entry.flags_extended)
                    .intersects(IndexEntryExtendedFlag::INTENT_TO_ADD | IndexEntryExtendedFlag::SKIP_WORKTREE)
            {
                return Err(Error::UnsupportedIndex);
            }
            if has_selected_descendant(&wanted, &entry.path, &mut prefix) {
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

pub(super) fn has_selected_descendant(wanted: &BTreeMap<&[u8], usize>, path: &[u8], prefix: &mut Vec<u8>) -> bool {
    if path.len() >= MAX_PATH_BYTES {
        return false;
    }
    prefix.clear();
    prefix.extend_from_slice(path);
    prefix.push(b'/');
    wanted
        .range::<[u8], _>((Bound::Included(prefix.as_slice()), Bound::Unbounded))
        .next()
        .is_some_and(|(selected, _)| selected.starts_with(prefix))
}

pub(super) fn tree_node<'repo>(
    mut tree: Tree<'repo>,
    path: &str,
    mut load: impl FnMut(Oid) -> Result<Tree<'repo>, git2::Error>,
) -> Result<Option<GitNode>, Error> {
    let mut components = path.split('/').peekable();
    while let Some(component) = components.next() {
        let Some(entry) = tree.get_name(component) else {
            return Ok(None);
        };
        let (id, kind, mode) = (entry.id(), entry.kind(), entry.filemode_raw());
        if components.peek().is_none() {
            let mode = u32::try_from(mode).map_err(|_| Error::UnsupportedNode)?;
            return Ok(Some(GitNode::new(mode, id)?));
        }
        if kind != Some(ObjectType::Tree) {
            return Err(Error::UnsupportedNode);
        }
        drop(entry);
        tree = load(id).map_err(|_| Error::GitRead)?;
    }
    Err(Error::UnsupportedNode)
}
