//! Host-side handling for agent-requested browser panel lifecycle changes.

use std::time::{Duration, Instant};

use horizon_core::browser::manifest::{
    self, BrowserCreateAuditStatus, BrowserCreateRequest, BrowserCreateResult, BrowserVisibilityAuditStatus,
    BrowserVisibilityRequest, BrowserVisibilityResult,
};
use horizon_core::browser::{BackendAvailability, BackendKind, BrowserStatus};
use horizon_core::{Board, PanelId, PanelKind, PanelOptions, WorkspaceId, browser_actor};

use super::HorizonApp;

const CREATE_REQUEST_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Default)]
pub(super) struct BrowserCreateHostState {
    last_request_poll: Option<Instant>,
    pending: Vec<PendingBrowserCreate>,
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

impl HorizonApp {
    pub(super) fn poll_browser_create_requests(&mut self) -> bool {
        let mut changed = self.finish_pending_browser_creates();
        let now = Instant::now();
        if self
            .browser_create_host
            .last_request_poll
            .is_some_and(|last| now.saturating_duration_since(last) < CREATE_REQUEST_POLL_INTERVAL)
        {
            return changed;
        }
        self.browser_create_host.last_request_poll = Some(now);

        let requests = match manifest::list_create_requests() {
            Ok(requests) => requests,
            Err(error) => {
                tracing::warn!(error = %error, "could not poll browser create requests");
                return changed;
            }
        };
        for request in requests {
            let Some(actor_panel) = actor_panel(&self.board, &request.actor) else {
                continue;
            };
            let request = match manifest::claim_create_request(&request.request_id, &request.actor, std::process::id())
            {
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
        changed |= self.poll_browser_visibility_requests();
        changed |= self.sync_browser_manifest_visibility();
        changed
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
            let Some(actor_panel) = actor_panel(&self.board, &request.actor) else {
                continue;
            };
            let request = match manifest::claim_visibility_request(
                &request.request_id,
                &request.actor,
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
        if let Err(error) = set_manifest_visibility(&request.panel_local_id, request.visible) {
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
            let _ = set_manifest_visibility(&request.panel_local_id, original_visible);
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

    fn sync_browser_manifest_visibility(&self) -> bool {
        let mut changed = false;
        for panel in self
            .board
            .panels
            .iter()
            .filter(|panel| panel.kind == PanelKind::Browser)
        {
            let Some(current) = manifest::read(&panel.local_id) else {
                continue;
            };
            let expected_hidden = !panel.visible;
            if current.hidden == expected_hidden {
                continue;
            }
            match set_manifest_visibility(&panel.local_id, panel.visible) {
                Ok(()) => changed = true,
                Err(error) => {
                    tracing::warn!(panel_id = %panel.local_id, %error, "could not synchronize browser visibility");
                }
            }
        }
        changed
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
            match finish_ready_browser_create(&pending) {
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

fn finish_ready_browser_create(pending: &PendingBrowserCreate) -> BrowserCreateCompletion {
    if manifest::read(&pending.panel_local_id).is_none() {
        return BrowserCreateCompletion::Waiting;
    }
    if let Err(error) = manifest::claim(&pending.panel_local_id, &pending.request.actor, None) {
        tracing::error!(request_id = %pending.request.request_id, %error, "could not assign requested browser ownership");
        record_and_complete_failure(
            pending,
            "ownership_failed",
            "Horizon could not assign the new browser panel to the requesting agent",
        );
        return BrowserCreateCompletion::Failed;
    }
    if let Err(error) = set_manifest_visibility(&pending.panel_local_id, pending.request.visible) {
        tracing::error!(request_id = %pending.request.request_id, %error, "could not set requested browser visibility");
        record_and_complete_failure(
            pending,
            "manifest_update_failed",
            "Horizon could not publish the new browser panel's visibility",
        );
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

fn set_manifest_visibility(panel_local_id: &str, visible: bool) -> std::io::Result<()> {
    manifest::update(panel_local_id, |manifest| {
        manifest.hidden = !visible;
        manifest.updated_at = manifest::now_millis();
    })
    .map(|_| ())
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
}
