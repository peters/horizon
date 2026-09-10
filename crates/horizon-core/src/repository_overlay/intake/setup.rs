//! Explicit task selection and point-in-time checkout inspection; never setup or start.

use super::IntakeRequest;
use crate::{
    cloud_run::{ArtifactDigest, CloudJobId},
    repository_overlay::checkout::publication::validate_sibling_name,
};
use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf};

pub const SETUP_SELECTION_LIMIT: usize = super::REQUEST_LIMIT + 2048;

/// Immutable repository identity, not current working-tree contents or task authority.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupCheckoutSelection {
    version: u8,
    intake: IntakeRequest,
    destination: String,
}

impl SetupCheckoutSelection {
    /// Validate a selection without inspecting storage or granting execution.
    /// # Errors
    /// Rejects invalid intake identities and destination components.
    pub fn new(intake: IntakeRequest, destination: String) -> Result<Self, SetupCheckoutError> {
        let selection = Self {
            version: 1,
            intake,
            destination,
        };
        selection.binding()?;
        Ok(selection)
    }

    /// Decode bounded strict input. Equivalent JSON field order has one binding.
    /// # Errors
    /// Rejects malformed, duplicate, unknown, oversized or invalid fields.
    pub fn decode(bytes: &[u8]) -> Result<Self, SetupCheckoutError> {
        if bytes.len() > SETUP_SELECTION_LIMIT {
            return Err(SetupCheckoutError::Invalid);
        }
        let selection: Self = serde_json::from_slice(bytes).map_err(|_| SetupCheckoutError::Invalid)?;
        selection.binding()?;
        Ok(selection)
    }

    /// Inert canonical identity; no checkout, key, provider or retained-file access.
    /// # Errors
    /// Rejects invalid or unsupported selection metadata.
    pub fn binding(&self) -> Result<ArtifactDigest, SetupCheckoutError> {
        if self.version != 1 || self.intake.encode().is_err() || validate_sibling_name(&self.destination).is_err() {
            return Err(SetupCheckoutError::Invalid);
        }
        let bytes = serde_json::to_vec(&("horizon-prepared-task-v1", self)).map_err(|_| SetupCheckoutError::Invalid)?;
        Ok(ArtifactDigest::sha256(&bytes))
    }

    #[must_use]
    pub fn runtime(&self) -> CloudJobId {
        self.intake.job_id
    }

    /// Inspect the matching intake/setup and current private checkout root without writes.
    /// HEAD, index and working files may evolve; their original contents are not rehashed.
    /// Requires stable exclusively controlled ancestry/root and the existing storage/tool
    /// premises through consumption. This is not historical inode proof or continuous
    /// revocation. Run off the UI thread; blocking storage has no hard deadline.
    /// # Errors
    /// Unknown setup, conflicting identities, unsafe/missing roots and cancellation refuse
    /// new task admission. Nothing is created, repaired, synchronized, started or removed.
    pub fn inspect(&self, cancelled: impl Fn() -> bool) -> Result<SetupCheckoutLocation, SetupCheckoutError> {
        self.binding()?;
        if cancelled() {
            return Err(SetupCheckoutError::Cancelled);
        }
        #[cfg(target_os = "linux")]
        return inspect(self, &cancelled);
        #[cfg(not(target_os = "linux"))]
        Err(SetupCheckoutError::Unsupported)
    }
}

impl fmt::Debug for SetupCheckoutSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetupCheckoutSelection").finish_non_exhaustive()
    }
}

/// Current root identity for an immediate same-selection recheck, not an execution grant.
#[derive(Serialize)]
pub struct SetupCheckoutLocation {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum SetupCheckoutError {
    #[error("invalid prepared task repository selection")]
    Invalid,
    #[error("prepared task repository is not confirmed published")]
    Unconfirmed,
    #[error("prepared task repository storage is unavailable or changed")]
    Storage,
    #[error("prepared task repository inspection is unsupported")]
    Unsupported,
    #[error("prepared task repository inspection was cancelled")]
    Cancelled,
}

#[cfg(target_os = "linux")]
fn inspect(
    selection: &SetupCheckoutSelection,
    cancelled: &impl Fn() -> bool,
) -> Result<SetupCheckoutLocation, SetupCheckoutError> {
    inspect_observed(selection, super::observe(&selection.intake, cancelled), cancelled)
}

#[cfg(target_os = "linux")]
fn inspect_observed(
    selection: &SetupCheckoutSelection,
    observed: super::IntakeResponse,
    cancelled: &impl Fn() -> bool,
) -> Result<SetupCheckoutLocation, SetupCheckoutError> {
    use super::IntakeError;
    use crate::repository_overlay::retained_setup::{RetainedSetup, SetupIntent};
    if observed.state != super::IntakeState::Observed
        || observed.reason.is_some()
        || observed.bundle != Some(super::BundleState::Observed)
        || !observed
            .pack
            .as_ref()
            .is_some_and(|pack| pack.state == super::PackState::Observed)
    {
        return Err(match observed.reason {
            Some(IntakeError::Cancelled) => SetupCheckoutError::Cancelled,
            Some(IntakeError::Unsupported) => SetupCheckoutError::Unsupported,
            Some(IntakeError::Storage) => SetupCheckoutError::Storage,
            _ => SetupCheckoutError::Unconfirmed,
        });
    }
    let roots = observed.roots.ok_or(SetupCheckoutError::Unconfirmed)?;
    let intent = SetupIntent::new(
        selection.intake.workspace_local_id.clone(),
        roots.packs.join("base/decoded/objects"),
        roots.bundles,
        selection.intake.overlay.sha256.clone(),
        selection.destination.clone(),
    )
    .map_err(|_| SetupCheckoutError::Invalid)?;
    let store = RetainedSetup::open(&roots.setup).map_err(|error| record_error(error.into()))?;
    let completion = store
        .completion(&intent)
        .map_err(record_error)?
        .ok_or(SetupCheckoutError::Unconfirmed)?;
    let path = roots.setup.join("setup-data").join(&selection.destination);
    if !published(
        selection,
        completion.state(),
        completion.base_commit(),
        completion.bundle_manifest(),
    ) || completion.checkout() != Some(path.as_path())
    {
        return Err(SetupCheckoutError::Unconfirmed);
    }
    let pinned = pin_location(&roots.setup, &path)?;
    if cancelled() {
        return Err(SetupCheckoutError::Cancelled);
    }
    if store.completion(&intent).map_err(record_error)?.as_ref() != Some(&completion) {
        return Err(SetupCheckoutError::Storage);
    }
    let current = pin_location(&roots.setup, &path)?;
    if (pinned.0.device, pinned.0.inode) != (current.0.device, current.0.inode) {
        return Err(SetupCheckoutError::Storage);
    }
    if cancelled() {
        return Err(SetupCheckoutError::Cancelled);
    }
    Ok(pinned.0)
}

#[cfg(target_os = "linux")]
fn record_error(error: crate::repository_overlay::retained_setup::SetupRecordError) -> SetupCheckoutError {
    use crate::repository_overlay::retained_setup::{SetupBoundaryError as Boundary, SetupRecordError as Record};
    match error {
        Record::Boundary(Boundary::Unsupported) => SetupCheckoutError::Unsupported,
        Record::Boundary(Boundary::Cancelled) => SetupCheckoutError::Cancelled,
        Record::Boundary(Boundary::InvalidClaim) | Record::InvalidRecord => SetupCheckoutError::Unconfirmed,
        _ => SetupCheckoutError::Storage,
    }
}

#[cfg(any(target_os = "linux", test))]
fn published(
    selection: &SetupCheckoutSelection,
    state: crate::repository_overlay::retained_setup::SetupCompletionState,
    base: Option<&str>,
    manifest: Option<&ArtifactDigest>,
) -> bool {
    state == crate::repository_overlay::retained_setup::SetupCompletionState::Published
        && base == Some(selection.intake.source.commit.as_str())
        && manifest == Some(&selection.intake.overlay.sha256)
}

#[cfg(target_os = "linux")]
fn pin_location(
    parent: &std::path::Path,
    path: &std::path::Path,
) -> Result<(SetupCheckoutLocation, std::fs::File), SetupCheckoutError> {
    use crate::repository_overlay::reader::{RepositoryReadError, SelectedRepositoryReader};
    use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
    use std::{fs::File, os::unix::fs::MetadataExt};
    let root = SelectedRepositoryReader::open(parent).map_err(|error| match error {
        RepositoryReadError::Unsupported => SetupCheckoutError::Unsupported,
        _ => SetupCheckoutError::Storage,
    })?;
    let relative = path.strip_prefix(parent).map_err(|_| SetupCheckoutError::Storage)?;
    let file = File::from(
        openat2(
            root.root.handle(),
            relative,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(|error| match error {
            rustix::io::Errno::NOSYS | rustix::io::Errno::INVAL | rustix::io::Errno::OPNOTSUPP => {
                SetupCheckoutError::Unsupported
            }
            _ => SetupCheckoutError::Storage,
        })?,
    );
    let metadata = file.metadata().map_err(|_| SetupCheckoutError::Storage)?;
    let parent_metadata = root.root.handle().metadata().map_err(|_| SetupCheckoutError::Storage)?;
    if [&metadata, &parent_metadata].iter().any(|node| {
        !node.is_dir()
            || node.nlink() == 0
            || node.uid() != rustix::process::geteuid().as_raw()
            || node.mode() & 0o7777 != 0o700
    }) || metadata.dev() != parent_metadata.dev()
    {
        return Err(SetupCheckoutError::Storage);
    }
    Ok((
        SetupCheckoutLocation {
            path: path.to_owned(),
            device: metadata.dev(),
            inode: metadata.ino(),
        },
        file,
    ))
}

#[cfg(test)]
mod tests;
