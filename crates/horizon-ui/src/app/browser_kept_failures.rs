//! The reasons that the kept panels of a browser host tell the agents.
//!
//! A panel of a browser host that loses its browser stays with its last address,
//! and the person has Retry. Its driver is gone, so no manifest describes that
//! panel, and an agent that reads `browser_panel` would find nothing. This host
//! publishes the typed reason instead, on the coordination worker, and it
//! clears the reason when the panel connects again or leaves the board.

use std::collections::BTreeMap;

use horizon_core::browser::BackendKind;
use horizon_core::browser::manifest::{self, KeptPanel, ManifestFailure, ManifestWorkspace};
use horizon_core::{Board, PanelKind};

use super::HorizonApp;
use super::browser_manifest_stamp::browser_workspace;

/// One kept panel as the agents read it, owned so the worker holds no board.
#[derive(Clone, Debug)]
struct KeptReason {
    local_id: String,
    workspace: ManifestWorkspace,
    hidden: bool,
    url: String,
    title: String,
    backend: BackendKind,
    failure: ManifestFailure,
}

impl KeptReason {
    fn kept_panel(&self) -> KeptPanel<'_> {
        KeptPanel {
            workspace: self.workspace.clone(),
            hidden: self.hidden,
            url: &self.url,
            title: &self.title,
            backend: self.backend,
            failure: self.failure.clone(),
        }
    }
}

/// The panels whose kept reason this host published, with the code it wrote, so
/// one reason is written once and a gone panel's manifest leaves the list.
#[derive(Default)]
pub(super) struct KeptBrowserFailures {
    published: BTreeMap<String, String>,
}

impl HorizonApp {
    /// Publishes the reason of every panel that keeps one, and clears the
    /// reason of a panel that connected again or left the board. The files are
    /// written on the coordination worker; this frame only queues them.
    pub(super) fn sync_kept_browser_failures(&mut self) -> bool {
        let kept = kept_reasons(&self.board);
        let root = self.host_manifest_root().to_path_buf();
        let published = self.browser_create_host.kept_failures.published.clone();
        let mut queued = false;
        let kept_now: Vec<String> = kept.iter().map(|reason| reason.local_id.clone()).collect();
        for reason in kept {
            let code = reason.failure.code.clone();
            if published.get(&reason.local_id) == Some(&code) {
                continue;
            }
            let published_id = reason.local_id.clone();
            let local_id = published_id.clone();
            let work_root = root.clone();
            self.browser_create_host.io.write_for_board(move || {
                match manifest::publish_kept_panel_failure(&work_root, &local_id, &reason.kept_panel()) {
                    Ok(_) => {}
                    Err(error) => tracing::warn!(
                        panel_id = %local_id,
                        %error,
                        "could not publish why a kept browser panel cannot serve a page"
                    ),
                }
            });
            self.browser_create_host
                .kept_failures
                .published
                .insert(published_id, code);
            queued = true;
        }
        let live: Vec<String> = self.board.panels.iter().map(|panel| panel.local_id.clone()).collect();
        let released: Vec<String> = self
            .browser_create_host
            .kept_failures
            .published
            .keys()
            .filter(|local_id| !kept_now.contains(local_id))
            .cloned()
            .collect();
        for local_id in released {
            self.browser_create_host.kept_failures.published.remove(&local_id);
            // A panel that still stands connected again, so its reason goes and
            // its place stays; a panel that left the board leaves the list.
            let panel_gone = !live.contains(&local_id);
            let work_root = root.clone();
            self.browser_create_host.io.write(move || {
                if let Err(error) = manifest::release_kept_panel_failure(&work_root, &local_id, panel_gone) {
                    tracing::warn!(
                        panel_id = %local_id,
                        %error,
                        "could not clear the reason of a browser panel that needs it no more"
                    );
                }
            });
            queued = true;
        }
        queued
    }

    /// Clears the reason of the panels of this board, for a board that a session
    /// switch replaces: the panels of the new board describe themselves.
    pub(super) fn forget_kept_browser_failures(&mut self) -> bool {
        let published = std::mem::take(&mut self.browser_create_host.kept_failures.published);
        if published.is_empty() {
            return false;
        }
        let root = self.host_manifest_root().to_path_buf();
        for local_id in published.into_keys() {
            let work_root = root.clone();
            self.browser_create_host.io.write(move || {
                if let Err(error) = manifest::release_kept_panel_failure(&work_root, &local_id, true) {
                    tracing::warn!(
                        panel_id = %local_id,
                        %error,
                        "could not remove the reason of a browser panel of the previous session"
                    );
                }
            });
        }
        true
    }
}

/// Every panel of `board` that keeps the reason its browser host gave, in board
/// order. A panel that is still connecting, or one that never showed a page,
/// has no reason to publish: the first has none, and the second ended.
fn kept_reasons(board: &Board) -> Vec<KeptReason> {
    board
        .panels
        .iter()
        .filter(|panel| panel.kind == PanelKind::Browser)
        .filter_map(|panel| {
            let browser = panel.browser()?;
            let failure = browser.attached_host_failure()?;
            if browser.has_ended() {
                return None;
            }
            Some(KeptReason {
                local_id: panel.local_id.clone(),
                workspace: browser_workspace(board, panel.workspace_id)?,
                hidden: !panel.visible,
                url: browser.display_url().to_string(),
                title: browser.title.clone(),
                backend: browser.backend(),
                failure: ManifestFailure::new(failure.code(), failure.report()),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
