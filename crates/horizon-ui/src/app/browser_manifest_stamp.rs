//! The host stamp of the browser manifests: the placement and workspace each one authorizes.

use std::hash::{Hash, Hasher};
use std::path::Path;

use horizon_core::browser::BackendKind;
use horizon_core::browser::manifest::{self, AgentIdentity, HostStampOutcome, ManifestWorkspace};
use horizon_core::{Board, PanelKind, WorkspaceId, browser_actor};

/// One browser panel's host-owned state as the board has it; no visibility while a request for it runs.
pub(super) struct BrowserPlacement {
    pub(super) local_id: String,
    pub(super) visible: Option<bool>,
    pub(super) workspace: ManifestWorkspace,
}

/// Outcome of stamping every browser manifest: whether any file changed and
/// whether every manifest this host owns is now current. An incomplete sync
/// keeps the placement fingerprint uncommitted so the next tick retries.
#[derive(Clone, Copy, Default)]
pub(super) struct HostStateSync {
    pub(super) changed: bool,
    pub(super) complete: bool,
}

/// The workspace stamp for browser panels in `workspace_id`: this host plus
/// the identities of every agent panel currently sharing that workspace.
pub(super) fn browser_workspace(board: &Board, workspace_id: WorkspaceId) -> Option<ManifestWorkspace> {
    let workspace = board.workspace(workspace_id)?;
    let actors = board
        .panels
        .iter()
        .filter(|panel| panel.kind.is_agent() && panel.workspace_id == workspace_id)
        .map(|panel| browser_actor(&panel.local_id))
        .collect();
    Some(ManifestWorkspace::new(
        manifest::host_instance(),
        &workspace.local_id,
        actors,
    ))
}

/// The host-owned state of every browser panel on the board, in board order.
pub(super) fn browser_placements(board: &Board) -> Vec<BrowserPlacement> {
    board
        .panels
        .iter()
        .filter(|panel| panel.kind == PanelKind::Browser)
        .filter_map(|panel| {
            browser_workspace(board, panel.workspace_id).map(|workspace| BrowserPlacement {
                local_id: panel.local_id.clone(),
                visible: Some(panel.visible),
                workspace,
            })
        })
        .collect()
}

/// Stamp each placement's manifest under `root`. Manifests that are not live
/// yet, or that another host's driver owns, are not this host's to stamp and
/// do not make the sync incomplete; every I/O failure does (including a
/// permission failure on the lock or the write), so the caller retries on
/// the next frame.
pub(super) fn sync_manifest_host_state(
    root: &Path,
    placements: &[BrowserPlacement],
    mut confirm_scope: impl FnMut(&str, bool),
) -> HostStateSync {
    let mut sync = HostStateSync {
        changed: false,
        complete: true,
    };
    for placement in placements {
        // A placement without a visibility keeps the manifest's, read under the write's lock.
        match manifest::sync_host_state_in(root, &placement.local_id, placement.visible, &placement.workspace) {
            Ok(HostStampOutcome::Written) => {
                sync.changed = true;
                confirm_scope(&placement.local_id, true);
            }
            Ok(HostStampOutcome::Unchanged) => confirm_scope(&placement.local_id, true),
            Ok(HostStampOutcome::NotOwned) => {
                confirm_scope(&placement.local_id, false);
                tracing::debug!(panel_id = %placement.local_id, "browser manifest belongs to another Horizon host");
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                sync.complete = false;
                confirm_scope(&placement.local_id, false);
                tracing::warn!(panel_id = %placement.local_id, %error, "could not synchronize browser host state");
            }
        }
    }
    sync
}

/// Everything the workspace stamps depend on: the persisted identity of each
/// browser and agent panel, the persisted identity of the workspace it sits
/// in, and whether each browser panel is shown. Persisted ids rather than
/// board ids, because activating another session replaces the board and
/// restarts its numeric ids. Cheap enough to compute once per frame; it
/// touches no files.
pub(super) fn placement_fingerprint(board: &Board) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for panel in &board.panels {
        let browser = panel.kind == PanelKind::Browser;
        if !browser && !panel.kind.is_agent() {
            continue;
        }
        panel.local_id.hash(&mut hasher);
        board
            .workspace(panel.workspace_id)
            .map(|workspace| workspace.local_id.as_str())
            .hash(&mut hasher);
        browser.hash(&mut hasher);
        (browser && panel.visible).hash(&mut hasher);
    }
    hasher.finish()
}

pub(super) fn host_identity(actor: &str) -> AgentIdentity<'_> {
    AgentIdentity::new(actor, Some(manifest::host_instance()))
}

pub(super) fn backend_session_limit_reached(board: &Board, backend: BackendKind) -> bool {
    let Some(limit) = backend.capabilities().max_sessions else {
        return false;
    };
    let live = board
        .panels
        .iter()
        .filter_map(|panel| panel.browser())
        .filter(|browser| browser.backend() == backend && browser.status.is_alive())
        .count();
    live >= usize::try_from(limit).unwrap_or(usize::MAX)
}

pub(super) fn publish_manifest_host_state(
    panel_local_id: &str,
    visible: bool,
    workspace: &ManifestWorkspace,
) -> std::io::Result<()> {
    match manifest::sync_host_state(panel_local_id, visible, workspace)? {
        HostStampOutcome::Written | HostStampOutcome::Unchanged => Ok(()),
        HostStampOutcome::NotOwned => Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "browser panel's driver runs in another Horizon host",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::browser_request_claims::launched_by_this_host;
    use horizon_core::{PanelKind, PanelOptions};

    fn agent_options() -> PanelOptions {
        let (command, args) = ("/bin/sh", vec!["-c".to_string(), "exit 0".to_string()]);
        PanelOptions {
            command: Some(command.to_string()),
            args,
            kind: PanelKind::Codex,
            ..PanelOptions::default()
        }
    }

    #[test]
    fn placement_fingerprint_follows_membership_not_unrelated_panels() {
        let mut board = Board::new();
        let alpha = board.create_workspace("alpha");
        let beta = board.create_workspace("beta");
        let empty = placement_fingerprint(&board);
        let agent_id = board.create_panel(agent_options(), alpha).expect("agent panel");
        let with_agent = placement_fingerprint(&board);
        assert_ne!(
            empty, with_agent,
            "an agent panel joining a workspace changes the stamp inputs"
        );

        let shell_id = board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Shell,
                    ..agent_options()
                },
                alpha,
            )
            .expect("shell panel");
        assert_eq!(
            placement_fingerprint(&board),
            with_agent,
            "shell panels do not take part in browser authorization"
        );
        board.assign_panel_to_workspace(shell_id, beta);
        assert_eq!(placement_fingerprint(&board), with_agent);

        board.assign_panel_to_workspace(agent_id, beta);
        let moved = placement_fingerprint(&board);
        assert_ne!(with_agent, moved, "moving an agent panel changes the stamp inputs");
        board.assign_panel_to_workspace(agent_id, alpha);
        assert_eq!(
            placement_fingerprint(&board),
            with_agent,
            "moving back restores the fingerprint"
        );

        // A replacement board (another session) restarts numeric ids; its
        // persisted ids differ, so its fingerprint must differ too.
        let mut replacement = Board::new();
        let other_alpha = replacement.create_workspace("alpha");
        let _beta = replacement.create_workspace("beta");
        replacement
            .create_panel(agent_options(), other_alpha)
            .expect("agent panel in the replacement board");
        assert_ne!(
            placement_fingerprint(&replacement),
            with_agent,
            "identically shaped boards with different persisted ids never share a fingerprint"
        );
    }

    #[test]
    fn restamping_rewrites_a_live_manifest_for_the_new_membership() {
        let root = tempfile::tempdir().expect("isolated horizon home");
        let mut board = Board::new();
        let alpha = board.create_workspace("alpha");
        let beta = board.create_workspace("beta");
        let agent_id = board.create_panel(agent_options(), alpha).expect("agent panel");
        let actor = browser_actor(&board.panel(agent_id).expect("agent panel").local_id);
        let path = manifest::manifest_path_for_root(root.path(), "browser-1");
        manifest::write_at(
            &path,
            &manifest::BrowserManifest {
                panel_local_id: "browser-1".to_string(),
                host: Some(manifest::host_instance().to_string()),
                ..manifest::BrowserManifest::default()
            },
        )
        .expect("write live manifest");
        let placements = |board: &Board, visible: bool| {
            vec![BrowserPlacement {
                local_id: "browser-1".to_string(),
                visible: Some(visible),
                workspace: browser_workspace(board, alpha).expect("alpha workspace"),
            }]
        };

        let first = sync_manifest_host_state(root.path(), &placements(&board, true), |_, _| {});
        assert!(first.changed && first.complete);
        let stamped = manifest::read_at(&path).expect("stamped manifest");
        assert!(stamped.authorizes(AgentIdentity::new(&actor, Some(manifest::host_instance()))));
        assert!(!stamped.hidden);

        board.assign_panel_to_workspace(agent_id, beta);
        let moved = sync_manifest_host_state(root.path(), &placements(&board, false), |_, _| {});
        assert!(moved.changed && moved.complete);
        let restamped = manifest::read_at(&path).expect("re-stamped manifest");
        assert!(
            !restamped.authorizes(AgentIdentity::new(&actor, Some(manifest::host_instance()))),
            "the agent that left the workspace is no longer authorized"
        );
        assert!(restamped.hidden, "visibility follows the board");

        let steady = sync_manifest_host_state(root.path(), &placements(&board, false), |_, _| {});
        assert!(
            !steady.changed && steady.complete,
            "an unchanged placement writes nothing"
        );

        let missing = vec![BrowserPlacement {
            local_id: "not-live-yet".to_string(),
            visible: Some(true),
            workspace: browser_workspace(&board, alpha).expect("alpha workspace"),
        }];
        let sync = sync_manifest_host_state(root.path(), &missing, |_, _| {});
        assert!(
            !sync.changed && sync.complete,
            "a manifest that is not live yet is not this host's to stamp"
        );
    }

    #[test]
    fn failed_manifest_sync_revokes_recovery_but_missing_retired_manifests_do_not() {
        let root = tempfile::tempdir().expect("root");
        let path = manifest::manifest_path_for_root(root.path(), "browser-1");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("directory");
        std::fs::write(&path, "invalid manifest").expect("unreadable manifest fixture");
        let placements = vec![BrowserPlacement {
            local_id: "browser-1".into(),
            visible: Some(true),
            workspace: ManifestWorkspace::new(manifest::host_instance(), "workspace", Vec::new()),
        }];
        let allocation = horizon_core::browser::RemoteAllocation::default();
        allocation.mark_published();
        allocation.retain_scope(horizon_browser::RemoteAllocationScope {
            admission_fallback: false,
            host: manifest::host_instance().into(),
            workspace: Some("workspace".into()),
            owner: Some("owner".into()),
        });
        let sync = sync_manifest_host_state(root.path(), &placements, |_, confirmed| {
            allocation.confirm_scope(confirmed);
        });
        assert!(!sync.complete);
        assert_eq!(
            allocation.status_for(manifest::host_instance(), "owner", "workspace", true),
            None
        );
        std::fs::remove_file(&path).expect("simulate completed teardown");
        let sync = sync_manifest_host_state(root.path(), &placements, |_, _| {
            panic!("absence must preserve the existing retirement decision");
        });
        assert!(sync.complete);
    }

    #[test]
    fn workspace_stamp_follows_agent_panel_membership() {
        let mut board = Board::new();
        let alpha = board.create_workspace("alpha");
        let beta = board.create_workspace("beta");
        assert!(browser_workspace(&board, WorkspaceId(u64::MAX)).is_none());
        let (command, args) = ("/bin/sh", vec!["-c".to_string(), "exit 0".to_string()]);
        let agent_id = board
            .create_panel(
                PanelOptions {
                    command: Some(command.to_string()),
                    args,
                    kind: PanelKind::Codex,
                    ..PanelOptions::default()
                },
                alpha,
            )
            .expect("agent panel");
        let actor = browser_actor(&board.panel(agent_id).expect("agent panel").local_id);
        let identity = host_identity(&actor);

        let alpha_stamp = browser_workspace(&board, alpha).expect("alpha workspace");
        assert_eq!(alpha_stamp.local_id, board.workspace(alpha).expect("alpha").local_id);
        assert_eq!(alpha_stamp.host_instance, manifest::host_instance());
        assert!(alpha_stamp.authorizes(identity));
        assert!(
            !alpha_stamp.authorizes(AgentIdentity::new(&actor, Some("another-live-host"))),
            "a copied session in another process never matches this host's stamp"
        );
        assert!(
            !browser_workspace(&board, beta)
                .expect("beta workspace")
                .authorizes(identity)
        );
        assert!(launched_by_this_host(Some(manifest::host_instance())));
        assert!(!launched_by_this_host(Some("another-live-host")));
        assert!(!launched_by_this_host(None));

        board.assign_panel_to_workspace(agent_id, beta);

        assert!(
            !browser_workspace(&board, alpha)
                .expect("alpha workspace")
                .authorizes(identity)
        );
        assert!(
            browser_workspace(&board, beta)
                .expect("beta workspace")
                .authorizes(identity)
        );
    }

    #[test]
    fn unlimited_backends_do_not_report_a_host_limit() {
        let board = Board::new();
        assert!(!backend_session_limit_reached(&board, BackendKind::ChromiumCdp));
        assert!(!backend_session_limit_reached(&board, BackendKind::FirefoxBidi));
    }
}
