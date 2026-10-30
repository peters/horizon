//! The dispatch, audit and publication of the browser panels an agent asks for.

use std::time::Instant;

use horizon_core::browser::manifest::{
    self, BrowserCreateAuditStatus, BrowserCreateRequest, BrowserCreateResult, ManifestWorkspace,
};
use horizon_core::browser::{BackendAvailability, BackendKind};
use horizon_core::{PanelKind, PanelOptions};

use super::HorizonApp;
use super::browser_create_failure::{
    CREATE_TIMEOUT, complete_result, record_and_complete_failure, record_create_failure,
};
use super::browser_create_lifecycle::{CreateStage, PendingBrowserCreate};
use super::browser_host_io::BoardFence;
use super::browser_manifest_stamp::{backend_session_limit_reached, host_identity};
use super::browser_request_claims::{HOST_RETIRING, fail_create, refuse_expired_create};
use super::browser_requests::ActorPanel;

impl HorizonApp {
    pub(super) fn start_requested_browser(&mut self, mut request: BrowserCreateRequest, actor_panel: ActorPanel) {
        // Startup latency counts everything the host does from accepting the
        // request, including panel creation and the audit writes.
        let started_at = Instant::now();
        if refuse_expired_create(&mut self.browser_create_host.io, &request) {
            return;
        }
        let duplicate = match self.prepare_browser_duplicate(&mut request, actor_panel) {
            Ok(options) => options,
            Err((code, message)) => {
                fail_create(&mut self.browser_create_host.io, &request, code, message);
                return;
            }
        };
        // A remote target is resolved before any panel exists, so a missing
        // or locked credential, an unknown target or a full provider is
        // reported to the agent as a typed refusal.
        let remote = match self.plan_and_admit_remote(&request, actor_panel.workspace_id) {
            Ok(plan) => plan,
            Err(refused) => {
                fail_create(
                    &mut self.browser_create_host.io,
                    &request,
                    refused.code,
                    &refused.message,
                );
                return;
            }
        };
        let backend = remote.as_ref().map_or_else(
            || request.backend.unwrap_or(self.template_config.browser.backend),
            |plan| plan.backend,
        );
        let startup_orientation = remote.as_ref().and_then(|plan| plan.request.orientation());
        if remote.is_none() {
            if let BackendAvailability::UnsupportedPlatform(reason) = backend.availability() {
                fail_create(
                    &mut self.browser_create_host.io,
                    &request,
                    "unsupported_platform",
                    reason,
                );
                return;
            }
            if backend_session_limit_reached(&self.board, backend) {
                fail_create(
                    &mut self.browser_create_host.io,
                    &request,
                    "session_limit_reached",
                    "the selected browser backend has reached its live-session limit",
                );
                return;
            }
        }

        let mut browser_config = self.template_config.browser.clone();
        browser_config.backend = backend;
        let recovery = remote.as_ref().map(|plan| plan.request.recovery.clone());
        let options = duplicate.unwrap_or_else(|| PanelOptions {
            command: request.url.clone(),
            kind: PanelKind::Browser,
            visible: request.visible,
            browser_config: Some(browser_config),
            remote_session: remote.map(|plan| plan.request),
            ..PanelOptions::default()
        });
        let panel_id = match self.create_agent_child_panel(options, actor_panel.workspace_id, actor_panel.panel_id) {
            Ok(panel_id) => panel_id,
            Err(error) => {
                if let Some(recovery) = &recovery {
                    recovery.cancel_before_launch();
                }
                tracing::error!(request_id = %request.request_id, %error, "failed to create requested browser panel");
                fail_create(
                    &mut self.browser_create_host.io,
                    &request,
                    "panel_create_failed",
                    "Horizon could not create the requested browser panel; inspect local logs",
                );
                return;
            }
        };
        let Some(panel_local_id) = self.board.panel(panel_id).map(|panel| panel.local_id.clone()) else {
            tracing::error!(request_id = %request.request_id, "created browser panel disappeared before registration");
            fail_create(
                &mut self.browser_create_host.io,
                &request,
                "panel_create_failed",
                "Horizon could not register the requested browser panel",
            );
            return;
        };
        self.audit_requested_browser(PendingBrowserCreate {
            request,
            panel_local_id,
            backend,
            startup_orientation,
            started_at,
            ready_since: None,
            user_navigations_at_start: 0,
            stage: CreateStage::Auditing,
            board: self.browser_create_host.io.board_fence(),
        });
    }

    /// Registers the create and audits its dispatch on the coordination
    /// worker. It waits for that audit before it may complete: a journal that
    /// cannot be written closes the panel instead.
    fn audit_requested_browser(&mut self, pending: PendingBrowserCreate) {
        let request_id = pending.request.request_id.clone();
        let audited = (pending.panel_local_id.clone(), pending.request.clone());
        let (backend, startup_orientation) = (pending.backend, pending.startup_orientation);
        self.browser_create_host.io.then(
            move || {
                let (panel_local_id, request) = audited;
                [BrowserCreateAuditStatus::Queued, BrowserCreateAuditStatus::Dispatched]
                    .into_iter()
                    .try_for_each(|status| {
                        manifest::record_create_status(&panel_local_id, &request, backend, startup_orientation, status)
                    })
            },
            move |app, audited| app.apply_create_dispatch_audit(&request_id, audited),
        );
        self.browser_create_host.pending.push(pending);
        self.mark_runtime_dirty();
    }

    /// Lets an audited create start, or closes the panel of one whose
    /// dispatch could not be audited.
    /// A create whose dispatch was audited while Horizon exits or switches
    /// sessions is settled as `host_shutdown`: its panel is being torn down.
    fn apply_create_dispatch_audit(&mut self, request_id: &str, audited: Option<std::io::Result<()>>) {
        let retiring = self.browser_host_retiring();
        let pending = &mut self.browser_create_host.pending;
        let Some(index) = pending
            .iter()
            .position(|pending| pending.request.request_id == request_id)
        else {
            return;
        };
        let (code, message) = match audited {
            Some(Ok(())) if retiring => HOST_RETIRING,
            Some(Ok(())) => {
                pending[index].stage = CreateStage::Starting;
                return;
            }
            Some(Err(error)) => {
                tracing::error!(%request_id, %error, "could not audit requested browser creation");
                ("audit_failed", "Horizon refused to create an unaudited browser panel")
            }
            None => ("audit_failed", "Horizon refused to create an unaudited browser panel"),
        };
        let pending = pending.remove(index);
        // A replaced board took the panel with it; a panel of the same id now is another one.
        if !self.browser_create_host.io.applying_stale()
            && let Some(panel_id) = self.board.panel_id_by_local_id(&pending.panel_local_id)
        {
            self.close_panel(panel_id);
        }
        record_and_complete_failure(&mut self.browser_create_host.io, &pending, code, message);
    }
}

/// Stamps the new panel and assigns it to the requesting agent in one locked
/// transaction, so no other same-workspace agent can claim it in between,
/// then audits the completion and publishes `ready`, on the coordination
/// worker. A step that fails publishes the failure instead, and so does a
/// create whose board retired since it was asked for. Says whether the create
/// was published as ready.
pub(super) fn publish_requested_create(
    live: &BoardFence,
    panel_local_id: &str,
    request: &BrowserCreateRequest,
    workspace: &ManifestWorkspace,
    backend: BackendKind,
    orientation: Option<horizon_core::browser::remote::RemoteOrientation>,
    ready: &BrowserCreateResult,
) -> bool {
    let fail = |code, message| {
        record_create_failure(panel_local_id, request, backend, orientation);
        complete_result(&BrowserCreateResult::failed(request, code, message));
        false
    };
    if !live.holds() {
        return fail(HOST_RETIRING.0, HOST_RETIRING.1);
    }
    if request.deadline_at_millis < manifest::now_millis() {
        return fail(CREATE_TIMEOUT.0, CREATE_TIMEOUT.1);
    }
    if let Err(error) = manifest::publish_requested_panel(
        panel_local_id,
        request.visible,
        workspace,
        host_identity(&request.actor),
    ) {
        tracing::error!(request_id = %request.request_id, %error, "could not publish requested browser panel");
        return if error.kind() == std::io::ErrorKind::PermissionDenied {
            fail(
                "ownership_failed",
                "Horizon could not assign the new browser panel to the requesting agent",
            )
        } else {
            fail(
                "manifest_update_failed",
                "Horizon could not publish the new browser panel's visibility and workspace",
            )
        };
    }
    if let Err(error) = manifest::record_create_status(
        panel_local_id,
        request,
        backend,
        orientation,
        BrowserCreateAuditStatus::Completed,
    ) {
        tracing::error!(request_id = %request.request_id, %error, "could not complete browser creation audit");
        return fail("audit_failed", "Horizon could not complete the browser creation audit");
    }
    complete_result(ready);
    true
}
