//! Read only explicitly selected Git paths; not export approval or an atomic snapshot.

#[cfg(target_os = "linux")]
mod content;
#[cfg(target_os = "linux")]
mod linux;

use super::{
    MAX_CHANGES, MAX_METADATA_BYTES, OverlayPlanError,
    bundle::{OverlayBundleError, RepositoryOverlayBundle},
    paths,
    reader::RepositoryReadError,
};
use crate::cloud_run::GitSource;
use std::{collections::BTreeSet, path::Path};

/// Capture literal selected index/worktree changes relative to the supplied current HEAD.
/// No discovery, writes, filters, subprocesses, network or selection expansion occurs.
/// The caller authorizes selection/export separately and supplies trusted Git metadata.
/// The repository identity is not checked against a configured remote URL.
///
/// Requires a pinned Linux worktree, SHA-1 repository and ordinary unconflicted index.
/// Payload bounds do not limit native Git metadata allocations or filesystem latency.
/// Run off the UI thread. Per-read and selected index/HEAD change detection does not
/// make this an atomic multi-file snapshot or protect against same-user ABA edits.
/// # Errors
/// Rejects invalid/excluded selections, unsafe nodes, unsupported Git state, detected
/// changes, mismatched HEAD and existing plan/file/aggregate payload limit violations.
pub fn capture_selected(
    root: &Path,
    source: GitSource,
    selected: &[&str],
) -> Result<RepositoryOverlayBundle, GitCaptureError> {
    source.validate().map_err(|_| OverlayPlanError::InvalidSource)?;
    let selection = validate_selection(selected)?;
    #[cfg(target_os = "linux")]
    {
        linux::capture(root, source, &selection)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (root, source, selection);
        Err(GitCaptureError::Unsupported)
    }
}

fn validate_selection<'a>(selected: &[&'a str]) -> Result<Vec<&'a str>, GitCaptureError> {
    if selected.len() > MAX_CHANGES / 2 {
        return Err(OverlayPlanError::ChangeLimit.into());
    }
    let mut unique = BTreeSet::new();
    let mut bytes = 0usize;
    for path in selected {
        paths::validate(path)?;
        bytes = bytes
            .checked_add(path.len())
            .filter(|total| *total <= MAX_METADATA_BYTES / 2)
            .ok_or(OverlayPlanError::MetadataLimit)?;
        if !unique.insert(*path) {
            return Err(OverlayPlanError::DuplicatePath.into());
        }
    }
    Ok(unique.into_iter().collect())
}

/// Redacted failures contain no Git diagnostics, paths, object identities or file bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum GitCaptureError {
    #[error("selected Git capture is unsupported on this platform")]
    Unsupported,
    #[error("selected root is not the exact supported Git worktree")]
    Repository,
    #[error("selected Git base does not match current HEAD")]
    BaseMismatch,
    #[error("selected Git index semantics are unsupported")]
    UnsupportedIndex,
    #[error("selected Git node or ancestor is unsupported")]
    UnsupportedNode,
    #[error("selected Git object is invalid or inconsistent")]
    InvalidObject,
    #[error("selected Git state changed during capture")]
    Changed,
    #[error("selected Git metadata could not be read")]
    GitRead,
    #[error(transparent)]
    Policy(#[from] OverlayPlanError),
    #[error(transparent)]
    Read(#[from] RepositoryReadError),
    #[error(transparent)]
    Bundle(#[from] OverlayBundleError),
}

#[cfg(test)]
mod tests;
