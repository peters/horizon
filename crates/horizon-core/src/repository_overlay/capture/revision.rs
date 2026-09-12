use super::{
    GitCaptureError as Error,
    content::{Contents, GitNode},
    linux::{State, tree_node},
};
use crate::{
    cloud_run::{GitCommitSha, GitSource},
    repository_overlay::{
        RepositoryOverlayPlan,
        bundle::RepositoryOverlayBundle,
        reader::{MAX_READ_BYTES, RepositoryReadError},
    },
};
use git2::{Oid, Repository};
use std::path::Path;

pub(super) fn capture(
    root: &Path,
    mut source: GitSource,
    branch: &str,
    selected: &[&str],
) -> Result<RepositoryOverlayBundle, Error> {
    let state = State::open_with(root, |repository| branch_head(repository, branch))?;
    source.commit = GitCommitSha::parse(state.base.to_string()).map_err(|_| Error::InvalidObject)?;
    let baseline = state.selected_index(selected)?;
    let tree = state
        .repository
        .find_commit(state.base)
        .and_then(|commit| commit.tree())
        .map_err(|_| Error::GitRead)?;
    let mut contents = Contents::default();
    let mut index = Vec::new();
    let mut working = Vec::new();
    for (path, indexed) in selected.iter().zip(&baseline) {
        // Preserve the existing base-tree ancestor refusal even for removed gitlinks.
        tree_node(tree.clone(), path, |id| state.repository.find_tree(id))?;
        index.push(contents.change(path, indexed.map(|node| node.read(&state.repository)).transpose()?)?);
        let node = match state.reader.read(path, MAX_READ_BYTES) {
            Ok(node) => Some(node),
            Err(RepositoryReadError::Missing) => None,
            Err(error) => return Err(error.into()),
        };
        working.push(contents.change(path, node)?);
    }
    verify(&state, branch, selected, &baseline)?;
    contents.finish(RepositoryOverlayPlan::new(source, index, working)?)
}

pub(super) fn verify(
    state: &State,
    branch: &str,
    selected: &[&str],
    baseline: &[Option<GitNode>],
) -> Result<(), Error> {
    state.verify(selected, baseline).map_err(|error| match error {
        Error::BaseMismatch => Error::Changed,
        error => error,
    })?;
    if branch_head(&state.repository, branch).map_err(|_| Error::Changed)? != state.base {
        return Err(Error::Changed);
    }
    Ok(())
}

pub(super) fn branch_head(repository: &Repository, branch: &str) -> Result<Oid, Error> {
    let head = repository.head().map_err(|_| Error::BaseMismatch)?;
    if !head.is_branch() || head.name().map_err(|_| Error::BaseMismatch)? != format!("refs/heads/{branch}") {
        return Err(Error::BaseMismatch);
    }
    head.peel_to_commit()
        .map(|commit| commit.id())
        .map_err(|_| Error::BaseMismatch)
}
