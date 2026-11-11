//! The typed reason a panel that stays cannot serve a page.
//!
//! A panel of a browser host keeps its place and its last address when the
//! browser of that machine stops, and the person has Retry. The agent that
//! reads the panel needs the same reason, because the panel still appears in
//! `browser_list`. The driver of that panel is gone, so the host publishes the
//! reason instead: it creates the manifest that the driver removed, or stamps
//! the one that is still there, and it clears the reason when the panel
//! connects again or leaves the board.
//!
//! Only the process that owns the panel writes this, and every write takes the
//! manifest lock, so a driver that starts while a reason is cleared keeps its
//! own manifest.

use std::path::Path;

use horizon_browser::BackendKind;
use serde::{Deserialize, Serialize};

use super::{
    BrowserManifest, HostStampOutcome, ManifestLock, ManifestWorkspace, manifest_path_for_root, mutate_at, now_millis,
    read_at,
};
use crate::paths::BrowserRuntimePaths;

/// The typed reason a panel cannot serve a page while the person keeps it. The
/// code is a fixed identifier that an agent acts on, and the message keeps the
/// words of the tool that failed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestFailure {
    pub code: String,
    pub message: String,
}

impl ManifestFailure {
    /// The reason `code` with the words of `message`.
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// What a kept panel is, for the agents that read it.
#[derive(Clone, Debug)]
pub struct KeptPanel<'a> {
    /// The workspace the panel is in, so the same agents that share it can read
    /// the reason.
    pub workspace: ManifestWorkspace,
    /// Whether the person hides the panel.
    pub hidden: bool,
    /// The last address the panel showed, which Retry opens again.
    pub url: &'a str,
    pub title: &'a str,
    pub backend: BackendKind,
    pub failure: ManifestFailure,
}

/// Publish the reason that the panel `panel_local_id` keeps in this Horizon home.
///
/// # Errors
/// Returns an I/O error when the manifest lock or the write fails.
pub fn publish_kept_panel_failure(
    root: &Path,
    panel_local_id: &str,
    kept: &KeptPanel<'_>,
) -> std::io::Result<HostStampOutcome> {
    let path = manifest_path_for_root(root, panel_local_id);
    let now = now_millis();
    let mut outcome = HostStampOutcome::Unchanged;
    mutate_at(&path, panel_local_id, true, |manifest| {
        // A manifest that names another Horizon host belongs to that host's
        // driver, which is not this host's to describe.
        if manifest
            .host
            .as_deref()
            .is_some_and(|host| host != kept.workspace.host_instance)
        {
            outcome = HostStampOutcome::NotOwned;
            return false;
        }
        let unchanged = manifest.url == kept.url
            && manifest.title == kept.title
            && manifest.backend == kept.backend
            && manifest.hidden == kept.hidden
            && manifest.workspace.as_ref() == Some(&kept.workspace)
            && manifest.failure.as_ref() == Some(&kept.failure);
        if unchanged {
            return false;
        }
        manifest.url = kept.url.to_string();
        manifest.title = kept.title.to_string();
        manifest.backend = kept.backend;
        manifest.hidden = kept.hidden;
        manifest.workspace = Some(kept.workspace.clone());
        manifest.host = Some(kept.workspace.host_instance.clone());
        manifest.failure = Some(kept.failure.clone());
        manifest.updated_at = now;
        outcome = HostStampOutcome::Written;
        true
    })?;
    Ok(outcome)
}

/// Forget the reason of `panel_local_id`, which is still a panel of this board,
/// or remove its manifest when `panel_gone` says the panel left the board.
///
/// # Errors
/// Returns an I/O error when the manifest lock or the write fails.
pub fn release_kept_panel_failure(
    root: &Path,
    panel_local_id: &str,
    panel_gone: bool,
) -> std::io::Result<HostStampOutcome> {
    let path = manifest_path_for_root(root, panel_local_id);
    if panel_gone {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // The read and the unlink hold the manifest lock, so a driver that
        // starts for this panel now keeps its own manifest.
        let _lock = ManifestLock::acquire(&path)?;
        let Some(manifest) = read_at(&path) else {
            return Ok(HostStampOutcome::Unchanged);
        };
        if manifest.failure.is_none() {
            return Ok(HostStampOutcome::NotOwned);
        }
        match std::fs::remove_file(&path) {
            Ok(()) => return Ok(HostStampOutcome::Written),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HostStampOutcome::Unchanged),
            Err(error) => return Err(error),
        }
    }
    let mut outcome = HostStampOutcome::Unchanged;
    mutate_at(&path, panel_local_id, false, |manifest| {
        if manifest.failure.take().is_none() {
            return false;
        }
        manifest.updated_at = now_millis();
        outcome = HostStampOutcome::Written;
        true
    })?;
    Ok(outcome)
}

/// The reason that the panel of `panel_local_id` keeps, when it has one.
#[must_use]
pub fn kept_panel_failure(panel_local_id: &str) -> Option<ManifestFailure> {
    read_at(&manifest_path_for_root(
        BrowserRuntimePaths::resolve().root(),
        panel_local_id,
    ))
    .and_then(|manifest: BrowserManifest| manifest.failure)
}

#[cfg(test)]
mod tests;
