//! Host-side handling for agent-requested browser panel lifecycle changes.

use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::{Duration, Instant};

use horizon_core::browser::manifest::{
    self, AgentIdentity, BrowserCreateAuditStatus, BrowserCreateRequest, BrowserCreateResult,
    BrowserVisibilityAuditStatus, BrowserVisibilityRequest, BrowserVisibilityResult, ManifestWorkspace,
};
use horizon_core::browser::{BackendAvailability, BackendKind, BrowserStatus};
use horizon_core::{Board, PanelId, PanelKind, PanelOptions, WorkspaceId, browser_actor};

use super::HorizonApp;

const CREATE_REQUEST_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Default)]
pub(super) struct BrowserCreateHostState {
    last_request_poll: Option<Instant>,
    pending: Vec<PendingBrowserCreate>,
    /// Board placement the manifests were last stamped for; a change
    /// re-stamps on the same frame instead of waiting for the next tick.
    stamped_placement: Option<u64>,
}

struct PendingBrowserCreate {
    request: BrowserCreateRequest,
    panel_id: PanelId,
    panel_local_id: String,
    backend: BackendKind,
}

#[derive(Clone, Copy)]
struct ActorPanel {
    panel_id: PanelId,
    workspace_id: WorkspaceId,
}

enum BrowserCreateCompletion {
    Waiting,
    Completed,
    Failed,
}

/// One browser panel's host-owned state as the board currently has it.
struct BrowserPlacement {
    local_id: String,
    visible: bool,
    workspace: ManifestWorkspace,
}

/// Outcome of stamping every browser manifest: whether any file changed and
/// whether every manifest this host owns is now current. An incomplete sync
/// keeps the placement fingerprint uncommitted so the next frame retries.
#[derive(Clone, Copy)]
struct HostStateSync {
    changed: bool,
    complete: bool,
}

impl HorizonApp {
    pub(super) fn poll_browser_create_requests(&mut self) -> bool {
        let mut changed = self.finish_pending_browser_creates();
        let now = Instant::now();
        let poll_due = self
            .browser_create_host
            .last_request_poll
            .is_none_or(|last| now.saturating_duration_since(last) >= CREATE_REQUEST_POLL_INTERVAL);
        // Placement changes are handled once per frame, after rendering, by
        // `restamp_browser_manifests_for_placement`; the tick only keeps the
        // cadence-based stamp.
        if poll_due {
            self.browser_create_host.last_request_poll = Some(now);
            changed |= self.poll_host_requests();
            changed |= self.stamp_current_placement();
        }
        changed
    }

    /// Re-stamp the manifests as soon as the board placement they depend on
    /// changes. This runs at the end of every frame, after queued workspace
    /// changes and the moves made while rendering, so moving or hiding a
    /// panel revokes the old workspace's access before the frame ends rather
    /// than on a later tick.
    pub(super) fn restamp_browser_manifests_for_placement(&mut self) -> bool {
        if self.browser_create_host.stamped_placement == Some(placement_fingerprint(&self.board)) {
            return false;
        }
        self.stamp_current_placement()
    }

    /// Stamp every owned manifest for the current placement and remember the
    /// placement only if all of them are current, so a failed write is
    /// retried on the next frame instead of waiting for the next tick.
    fn stamp_current_placement(&mut self) -> bool {
        let placement = placement_fingerprint(&self.board);
        let sync = self.sync_browser_manifest_host_state();
        self.browser_create_host.stamped_placement = sync.complete.then_some(placement);
        sync.changed
    }

    /// Root of the Horizon home whose manifests this host stamps. Production
    /// constructs the session store from `HorizonHome::resolve()`, the same
    /// root the drivers and the MCP server use for their default paths.
    fn host_manifest_root(&self) -> &Path {
        self.session_store.home().root()
    }

    fn poll_host_requests(&mut self) -> bool {
        let mut changed = false;
        let requests = match manifest::list_create_requests() {
            Ok(requests) => requests,
            Err(error) => {
                tracing::warn!(error = %error, "could not poll browser create requests");
                return changed;
            }
        };
        for request in requests {
            if !launched_by_this_host(request.host_instance.as_deref()) {
                continue;
            }
            let Some(actor_panel) = actor_panel(&self.board, &request.actor) else {
                continue;
            };
            let request = match manifest::claim_create_request(
                &request.request_id,
                &request.actor,
                manifest::host_instance(),
                std::process::id(),
            ) {
                Ok(Some(request)) => request,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(request_id = %request.request_id, error = %error, "could not claim browser create request");
                    continue;
                }
            };
            changed = true;
            self.start_requested_browser(request, actor_panel);
        }
        changed | self.poll_browser_visibility_requests()
    }

    fn start_requested_browser(&mut self, request: BrowserCreateRequest, actor_panel: ActorPanel) {
        if request.deadline_at_millis < manifest::now_millis() {
            complete_failure(
                &request,
                "request_expired",
                "browser create request expired before Horizon could accept it",
            );
            return;
        }
        let backend = request.backend.unwrap_or(self.template_config.browser.backend);
        if let BackendAvailability::UnsupportedPlatform(reason) = backend.availability() {
            complete_failure(&request, "unsupported_platform", reason);
            return;
        }
        if backend_session_limit_reached(&self.board, backend) {
            complete_failure(
                &request,
                "session_limit_reached",
                "the selected browser backend has reached its live-session limit",
            );
            return;
        }

        let mut browser_config = self.template_config.browser.clone();
        browser_config.backend = backend;
        let options = PanelOptions {
            command: request.url.clone(),
            kind: PanelKind::Browser,
            visible: request.visible,
            browser_config: Some(browser_config),
            ..PanelOptions::default()
        };
        let panel_id = match self.board.create_panel(options, actor_panel.workspace_id) {
            Ok(panel_id) => panel_id,
            Err(error) => {
                tracing::error!(request_id = %request.request_id, %error, "failed to create requested browser panel");
                complete_failure(
                    &request,
                    "panel_create_failed",
                    "Horizon could not create the requested browser panel; inspect local logs",
                );
                return;
            }
        };
        let Some(panel_local_id) = self.board.panel(panel_id).map(|panel| panel.local_id.clone()) else {
            tracing::error!(request_id = %request.request_id, "created browser panel disappeared before registration");
            complete_failure(
                &request,
                "panel_create_failed",
                "Horizon could not register the requested browser panel",
            );
            return;
        };
        for status in [BrowserCreateAuditStatus::Queued, BrowserCreateAuditStatus::Dispatched] {
            if let Err(error) = manifest::record_create_status(&panel_local_id, &request, backend, status) {
                tracing::error!(request_id = %request.request_id, %error, "could not audit requested browser creation");
                self.board.close_panel(panel_id);
                complete_failure(
                    &request,
                    "audit_failed",
                    "Horizon refused to create an unaudited browser panel",
                );
                return;
            }
        }
        self.browser_create_host.pending.push(PendingBrowserCreate {
            request,
            panel_id,
            panel_local_id,
            backend,
        });
        self.mark_runtime_dirty();
    }

    fn poll_browser_visibility_requests(&mut self) -> bool {
        let requests = match manifest::list_visibility_requests() {
            Ok(requests) => requests,
            Err(error) => {
                tracing::warn!(error = %error, "could not poll browser visibility requests");
                return false;
            }
        };
        let mut changed = false;
        for request in requests {
            if !launched_by_this_host(request.host_instance.as_deref()) {
                continue;
            }
            let Some(actor_panel) = actor_panel(&self.board, &request.actor) else {
                continue;
            };
            let request = match manifest::claim_visibility_request(
                &request.request_id,
                &request.actor,
                manifest::host_instance(),
                std::process::id(),
            ) {
                Ok(Some(request)) => request,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(request_id = %request.request_id, error = %error, "could not claim browser visibility request");
                    continue;
                }
            };
            changed |= self.apply_browser_visibility_request(&request, actor_panel);
        }
        changed
    }

    fn apply_browser_visibility_request(
        &mut self,
        request: &BrowserVisibilityRequest,
        actor_panel: ActorPanel,
    ) -> bool {
        if request.deadline_at_millis < manifest::now_millis() {
            complete_visibility_failure(request, "request_expired", "browser visibility request expired");
            return false;
        }
        let Some(panel_id) = self.board.panel_id_by_local_id(&request.panel_local_id) else {
            complete_visibility_failure(
                request,
                "panel_not_in_host",
                "browser panel is not hosted by the requesting agent's Horizon instance",
            );
            return false;
        };
        let Some(panel) = self.board.panel(panel_id) else {
            complete_visibility_failure(request, "panel_closed", "browser panel is not live");
            return false;
        };
        if panel.kind != PanelKind::Browser {
            complete_visibility_failure(request, "not_browser_panel", "target panel is not a browser panel");
            return false;
        }
        if panel.workspace_id != actor_panel.workspace_id {
            complete_visibility_failure(
                request,
                "panel_outside_workspace",
                "browser panel is outside the requesting agent's Horizon workspace",
            );
            return false;
        }
        let Some(workspace) = browser_workspace(&self.board, panel.workspace_id) else {
            complete_visibility_failure(request, "workspace_unavailable", "browser panel workspace is not live");
            return false;
        };
        let original_visible = panel.visible;
        let owned_by_actor = manifest::read(&request.panel_local_id)
            .and_then(|manifest| {
                manifest
                    .live_owner(manifest::now_millis())
                    .map(|owner| owner.name.clone())
            })
            .as_deref()
            == Some(request.actor.as_str());
        if !owned_by_actor {
            complete_visibility_failure(request, "ownership_changed", "browser panel ownership changed");
            return false;
        }
        if let Err(error) = manifest::record_visibility_status(request, BrowserVisibilityAuditStatus::Dispatched) {
            tracing::warn!(request_id = %request.request_id, %error, "could not audit browser visibility dispatch");
            complete_visibility_failure(
                request,
                "audit_failed",
                "Horizon refused an unaudited visibility change",
            );
            return false;
        }
        if let Err(error) = publish_manifest_host_state(&request.panel_local_id, request.visible, &workspace) {
            tracing::warn!(request_id = %request.request_id, %error, "could not update browser manifest visibility");
            complete_visibility_failure(
                request,
                "manifest_update_failed",
                "browser panel visibility could not be updated",
            );
            return false;
        }
        let local_changed = self.board.set_panel_visible(panel_id, request.visible);
        if !request.visible && self.fullscreen_panel == Some(panel_id) {
            self.fullscreen_panel = None;
        }
        if !request.visible && self.board.focused.is_none() {
            self.board.focus(actor_panel.panel_id);
        }
        if let Err(error) = manifest::record_visibility_status(request, BrowserVisibilityAuditStatus::Completed) {
            tracing::warn!(request_id = %request.request_id, %error, "could not audit browser visibility completion");
            let _ = publish_manifest_host_state(&request.panel_local_id, original_visible, &workspace);
            let _ = self.board.set_panel_visible(panel_id, original_visible);
            complete_visibility_failure(request, "audit_failed", "visibility change could not be audited");
            return false;
        }
        complete_visibility_result(&BrowserVisibilityResult::ready(request));
        if local_changed {
            self.mark_runtime_dirty();
        }
        local_changed
    }

    /// Keep every live browser manifest's host-owned presentation and
    /// workspace membership current, so MCP authorization follows panel moves
    /// and visibility changes made through the UI.
    fn sync_browser_manifest_host_state(&self) -> HostStateSync {
        sync_manifest_host_state(self.host_manifest_root(), &browser_placements(&self.board))
    }

    fn finish_pending_browser_creates(&mut self) -> bool {
        if self.browser_create_host.pending.is_empty() {
            return false;
        }
        let mut changed = false;
        let mut waiting = Vec::new();
        for pending in std::mem::take(&mut self.browser_create_host.pending) {
            if browser_create_is_terminal(&self.board, &pending) {
                changed = true;
                continue;
            }
            match finish_ready_browser_create(&self.board, &pending) {
                BrowserCreateCompletion::Waiting => waiting.push(pending),
                BrowserCreateCompletion::Completed => changed = true,
                BrowserCreateCompletion::Failed => {
                    self.board.close_panel(pending.panel_id);
                    changed = true;
                }
            }
        }
        self.browser_create_host.pending = waiting;
        changed
    }
}

fn actor_panel(board: &Board, actor: &str) -> Option<ActorPanel> {
    board
        .panels
        .iter()
        .find(|panel| panel.kind.is_agent() && browser_actor(&panel.local_id) == actor)
        .map(|panel| ActorPanel {
            panel_id: panel.id,
            workspace_id: panel.workspace_id,
        })
}

/// The workspace stamp for browser panels in `workspace_id`: this host plus
/// the identities of every agent panel currently sharing that workspace.
fn browser_workspace(board: &Board, workspace_id: WorkspaceId) -> Option<ManifestWorkspace> {
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
fn browser_placements(board: &Board) -> Vec<BrowserPlacement> {
    board
        .panels
        .iter()
        .filter(|panel| panel.kind == PanelKind::Browser)
        .filter_map(|panel| {
            browser_workspace(board, panel.workspace_id).map(|workspace| BrowserPlacement {
                local_id: panel.local_id.clone(),
                visible: panel.visible,
                workspace,
            })
        })
        .collect()
}

/// Stamp each placement's manifest under `root`. Manifests that are not live
/// yet, or that another host's driver owns, are not this host's to stamp and
/// do not make the sync incomplete; any other failure does, so the caller
/// retries on the next frame.
fn sync_manifest_host_state(root: &Path, placements: &[BrowserPlacement]) -> HostStateSync {
    let mut sync = HostStateSync {
        changed: false,
        complete: true,
    };
    for placement in placements {
        match manifest::sync_host_state_in(root, &placement.local_id, placement.visible, &placement.workspace) {
            Ok(true) => sync.changed = true,
            Ok(false) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                tracing::debug!(panel_id = %placement.local_id, %error, "browser manifest belongs to another Horizon host");
            }
            Err(error) => {
                sync.complete = false;
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
fn placement_fingerprint(board: &Board) -> u64 {
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

/// Requests name the host that launched the agent; a second Horizon process
/// hosting a copy of the same session must leave them alone.
fn launched_by_this_host(host_instance: Option<&str>) -> bool {
    host_instance == Some(manifest::host_instance())
}

fn host_identity(actor: &str) -> AgentIdentity<'_> {
    AgentIdentity::new(actor, Some(manifest::host_instance()))
}

fn backend_session_limit_reached(board: &Board, backend: BackendKind) -> bool {
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

fn finish_ready_browser_create(board: &Board, pending: &PendingBrowserCreate) -> BrowserCreateCompletion {
    if manifest::read(&pending.panel_local_id).is_none() {
        return BrowserCreateCompletion::Waiting;
    }
    let Some(workspace) = board
        .panel(pending.panel_id)
        .and_then(|panel| browser_workspace(board, panel.workspace_id))
    else {
        record_and_complete_failure(
            pending,
            "workspace_unavailable",
            "Horizon could not determine the new browser panel's workspace",
        );
        return BrowserCreateCompletion::Failed;
    };
    if !workspace.authorizes(host_identity(&pending.request.actor)) {
        // The user moved a panel while the browser started. Keep the panel
        // where they put it and report the lost workspace instead of closing
        // it or handing an uncontrollable panel back as ready.
        if let Err(error) = publish_manifest_host_state(&pending.panel_local_id, pending.request.visible, &workspace) {
            tracing::warn!(request_id = %pending.request.request_id, %error, "could not stamp a moved browser panel");
        }
        record_and_complete_failure(
            pending,
            "workspace_changed",
            "the browser panel left the requesting agent's workspace during creation and was left in place",
        );
        return BrowserCreateCompletion::Completed;
    }
    // Stamp and assign ownership in one locked transaction so no other
    // same-workspace agent can claim the new panel in between.
    if let Err(error) = manifest::publish_requested_panel(
        &pending.panel_local_id,
        pending.request.visible,
        &workspace,
        host_identity(&pending.request.actor),
    ) {
        tracing::error!(request_id = %pending.request.request_id, %error, "could not publish requested browser panel");
        let (code, message) = if error.kind() == std::io::ErrorKind::PermissionDenied {
            (
                "ownership_failed",
                "Horizon could not assign the new browser panel to the requesting agent",
            )
        } else {
            (
                "manifest_update_failed",
                "Horizon could not publish the new browser panel's visibility and workspace",
            )
        };
        record_and_complete_failure(pending, code, message);
        return BrowserCreateCompletion::Failed;
    }
    if let Err(error) = manifest::record_create_status(
        &pending.panel_local_id,
        &pending.request,
        pending.backend,
        BrowserCreateAuditStatus::Completed,
    ) {
        tracing::error!(request_id = %pending.request.request_id, %error, "could not complete browser creation audit");
        record_and_complete_failure(
            pending,
            "audit_failed",
            "Horizon could not complete the browser creation audit",
        );
        return BrowserCreateCompletion::Failed;
    }
    complete_result(&BrowserCreateResult::ready(
        &pending.request,
        pending.panel_local_id.clone(),
    ));
    BrowserCreateCompletion::Completed
}

fn browser_create_is_terminal(board: &Board, pending: &PendingBrowserCreate) -> bool {
    let Some(browser) = board.panel(pending.panel_id).and_then(|panel| panel.browser()) else {
        record_and_complete_failure(
            pending,
            "panel_closed",
            "the requested browser panel closed before it became controllable",
        );
        return true;
    };
    let failure = match &browser.status {
        BrowserStatus::Error { .. } => Some((
            "backend_start_failed",
            "the selected browser backend did not start; inspect the visible panel or local logs",
        )),
        BrowserStatus::Stopped { .. } => Some((
            "backend_stopped",
            "the selected browser backend stopped before it became controllable",
        )),
        BrowserStatus::Starting | BrowserStatus::Ready => None,
    };
    if let Some((code, message)) = failure {
        record_and_complete_failure(pending, code, message);
        return true;
    }
    if pending.request.deadline_at_millis < manifest::now_millis() {
        record_and_complete_failure(
            pending,
            "create_timeout",
            "the browser panel did not become controllable before the create deadline",
        );
        return true;
    }
    false
}

fn record_and_complete_failure(pending: &PendingBrowserCreate, code: &str, message: &str) {
    if let Err(error) = manifest::record_create_status(
        &pending.panel_local_id,
        &pending.request,
        pending.backend,
        BrowserCreateAuditStatus::Failed,
    ) {
        tracing::warn!(request_id = %pending.request.request_id, %error, "could not append failed browser creation audit");
    }
    complete_failure(&pending.request, code, message);
}

fn complete_failure(request: &BrowserCreateRequest, code: &str, message: &str) {
    complete_result(&BrowserCreateResult::failed(request, code, message));
}

fn complete_result(result: &BrowserCreateResult) {
    if let Err(error) = manifest::complete_create_request(result) {
        tracing::error!(request_id = %result.request_id, %error, "could not publish browser create result");
    }
}

fn publish_manifest_host_state(
    panel_local_id: &str,
    visible: bool,
    workspace: &ManifestWorkspace,
) -> std::io::Result<()> {
    manifest::sync_host_state(panel_local_id, visible, workspace).map(|_| ())
}

fn complete_visibility_failure(request: &BrowserVisibilityRequest, code: &str, message: &str) {
    if let Err(error) = manifest::record_visibility_status(request, BrowserVisibilityAuditStatus::Failed) {
        tracing::warn!(request_id = %request.request_id, %error, "could not append failed browser visibility audit");
    }
    complete_visibility_result(&BrowserVisibilityResult::failed(request, code, message));
}

fn complete_visibility_result(result: &BrowserVisibilityResult) {
    if let Err(error) = manifest::complete_visibility_request(result) {
        tracing::error!(request_id = %result.request_id, %error, "could not publish browser visibility result");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::test_app;

    fn agent_options() -> PanelOptions {
        let (command, args) = if cfg!(windows) {
            ("cmd.exe", vec!["/C".to_string(), "exit 0".to_string()])
        } else {
            ("/bin/sh", vec!["-c".to_string(), "exit 0".to_string()])
        };
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
        replacement.create_workspace("beta");
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
                visible,
                workspace: browser_workspace(board, alpha).expect("alpha workspace"),
            }]
        };

        let first = sync_manifest_host_state(root.path(), &placements(&board, true));
        assert!(first.changed && first.complete);
        let stamped = manifest::read_at(&path).expect("stamped manifest");
        assert!(stamped.authorizes(AgentIdentity::new(&actor, Some(manifest::host_instance()))));
        assert!(!stamped.hidden);

        board.assign_panel_to_workspace(agent_id, beta);
        let moved = sync_manifest_host_state(root.path(), &placements(&board, false));
        assert!(moved.changed && moved.complete);
        let restamped = manifest::read_at(&path).expect("re-stamped manifest");
        assert!(
            !restamped.authorizes(AgentIdentity::new(&actor, Some(manifest::host_instance()))),
            "the agent that left the workspace is no longer authorized"
        );
        assert!(restamped.hidden, "visibility follows the board");

        let steady = sync_manifest_host_state(root.path(), &placements(&board, false));
        assert!(
            !steady.changed && steady.complete,
            "an unchanged placement writes nothing"
        );

        let missing = vec![BrowserPlacement {
            local_id: "not-live-yet".to_string(),
            visible: true,
            workspace: browser_workspace(&board, alpha).expect("alpha workspace"),
        }];
        let sync = sync_manifest_host_state(root.path(), &missing);
        assert!(
            !sync.changed && sync.complete,
            "a manifest that is not live yet is not this host's to stamp"
        );
    }

    #[test]
    fn a_placement_change_restamps_before_the_next_poll_tick() {
        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        let beta = app.board.create_workspace("beta");
        let agent_id = app.board.create_panel(agent_options(), alpha).expect("agent panel");

        app.poll_browser_create_requests();
        let first_poll = app.browser_create_host.last_request_poll.expect("first tick polls");
        let stamped = app.browser_create_host.stamped_placement.expect("first tick stamps");

        app.poll_browser_create_requests();
        assert_eq!(
            app.browser_create_host.last_request_poll,
            Some(first_poll),
            "an unchanged board waits for the poll interval"
        );
        assert_eq!(app.browser_create_host.stamped_placement, Some(stamped));

        app.board.assign_panel_to_workspace(agent_id, beta);
        app.restamp_browser_manifests_for_placement();
        assert_eq!(
            app.browser_create_host.last_request_poll,
            Some(first_poll),
            "the end-of-frame re-stamp does not advance the request poll cadence"
        );
        let after_move = app.browser_create_host.stamped_placement;
        assert_ne!(
            after_move,
            Some(stamped),
            "a placement change re-stamps on the same frame"
        );

        app.poll_browser_create_requests();
        assert_eq!(
            app.browser_create_host.stamped_placement, after_move,
            "the next frame's poll leaves the placement to the end-of-frame check"
        );
        assert_eq!(app.browser_create_host.last_request_poll, Some(first_poll));
        assert!(
            !app.restamp_browser_manifests_for_placement(),
            "an unchanged placement is not re-stamped again"
        );
    }

    #[test]
    fn only_the_exact_horizon_actor_matches_a_panel() {
        assert!(actor_panel(&Board::new(), "horizon:missing").is_none());
        assert!(actor_panel(&Board::new(), "external").is_none());
    }

    #[test]
    fn unlimited_backends_do_not_report_a_host_limit() {
        let board = Board::new();
        assert!(!backend_session_limit_reached(&board, BackendKind::ChromiumCdp));
        assert!(!backend_session_limit_reached(&board, BackendKind::FirefoxBidi));
    }

    #[test]
    fn workspace_stamp_follows_agent_panel_membership() {
        let mut board = Board::new();
        let alpha = board.create_workspace("alpha");
        let beta = board.create_workspace("beta");
        assert!(browser_workspace(&board, WorkspaceId(u64::MAX)).is_none());
        let (command, args) = if cfg!(windows) {
            ("cmd.exe", vec!["/C".to_string(), "exit 0".to_string()])
        } else {
            ("/bin/sh", vec!["-c".to_string(), "exit 0".to_string()])
        };
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
}
