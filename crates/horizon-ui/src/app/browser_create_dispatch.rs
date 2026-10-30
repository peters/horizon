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
use super::browser_request_claims::{AGENT_GONE, HOST_RETIRING, fail_create, refuse_expired_create};
use super::browser_requests::{ActorPanel, actor_panel};

impl HorizonApp {
    /// Starts a create that the worker claimed on the board as it is now. A
    /// create that duplicates an existing panel reads that panel's manifest on
    /// the coordination worker first: the read retries for up to two seconds on
    /// Windows, so the frame that starts the create never does it. Only a
    /// duplicate create takes that extra ordered job; every other create starts
    /// on this frame, as before.
    pub(super) fn start_claimed_browser_create(&mut self, request: BrowserCreateRequest) {
        let Some(source) = request.duplicate_from.clone() else {
            self.start_claimed_browser_create_on_board(request, None);
            return;
        };
        // The app's home is the root the drivers use for their default paths:
        // production builds the session store from `HorizonHome::resolve()`.
        let path = manifest::manifest_path_for_root(self.host_manifest_root(), &source);
        self.browser_create_host.io.then(
            move || manifest::read_at(&path),
            move |app, source| app.start_claimed_browser_create_on_board(request, source.flatten().as_ref()),
        );
    }

    /// Starts or refuses the create on the board as it is once its source
    /// manifest was read, with the reasons `apply_claimed_requests` uses: a
    /// board that retires meanwhile, or an agent that left it. A manifest that
    /// could not be read reaches [`Self::duplicate_browser_options`] as an
    /// absent one, which refuses the duplicate as it does today.
    fn start_claimed_browser_create_on_board(
        &mut self,
        request: BrowserCreateRequest,
        source_manifest: Option<&manifest::BrowserManifest>,
    ) {
        if self.browser_host_retiring() {
            fail_create(
                &mut self.browser_create_host.io,
                &request,
                HOST_RETIRING.0,
                HOST_RETIRING.1,
            );
            return;
        }
        match actor_panel(&self.board, &request.actor) {
            Some(actor_panel) => self.start_requested_browser(request, actor_panel, source_manifest),
            None => fail_create(
                &mut self.browser_create_host.io,
                &request,
                "workspace_unavailable",
                AGENT_GONE,
            ),
        }
    }

    pub(super) fn start_requested_browser(
        &mut self,
        mut request: BrowserCreateRequest,
        actor_panel: ActorPanel,
        source_manifest: Option<&manifest::BrowserManifest>,
    ) {
        // Startup latency counts everything the host does from accepting the
        // request, including panel creation and the audit writes.
        let started_at = Instant::now();
        if refuse_expired_create(&mut self.browser_create_host.io, &request) {
            return;
        }
        let duplicate = match self.prepare_browser_duplicate(&mut request, actor_panel, source_manifest) {
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
            manifest: None,
            manifest_read_at: None,
            manifest_reading: false,
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

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    use horizon_core::browser::BrowserPanelState;
    use horizon_core::{Panel, PanelContent, PanelId, PanelOptions, WorkspaceId, browser_actor};

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

    /// A browser panel without a started driver, placed in `workspace`. Its
    /// duplicate decision is refused as unavailable, so a duplicate of it never
    /// opens a second panel.
    fn inert_browser(app: &mut HorizonApp, id: u64, workspace: WorkspaceId) -> String {
        let panel = Panel::from_content(
            PanelId(id),
            workspace,
            PanelKind::Browser,
            PanelContent::Browser(Box::new(BrowserPanelState::inert())),
        );
        let local_id = panel.local_id.clone();
        app.board.panels.push(panel);
        app.board.assign_panel_to_workspace(PanelId(id), workspace);
        local_id
    }

    #[test]
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
    fn a_duplicate_whose_source_manifest_read_waits_starts_only_once_after_it() {
        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        let agent = app.board.create_panel(agent_options(), alpha).expect("agent panel");
        let actor = browser_actor(&app.board.panel(agent).expect("agent").local_id);
        let source = inert_browser(&mut app, 9_300, alpha);
        let path = manifest::manifest_path_for_root(app.host_manifest_root(), &source);
        manifest::write_at(
            &path,
            &manifest::BrowserManifest {
                url: "https://example.test/".into(),
                host: Some(manifest::host_instance().to_string()),
                ..manifest::BrowserManifest::default()
            },
        )
        .expect("live manifest");
        let mut request = BrowserCreateRequest::for_tests("agent");
        request.actor = actor;
        request.host_instance = Some(manifest::host_instance().to_string());
        request.duplicate_from = Some(source);
        let panels_before = app.board.panels.len();

        // The worker waits, as it does for a manifest read whose file another
        // process holds for the read's two-second bound.
        let (release, released) = channel::<()>();
        app.browser_create_host.io.write(move || {
            let _ = released.recv();
        });

        let started = Instant::now();
        app.start_claimed_browser_create(request);
        let elapsed = started.elapsed();
        release.send(()).expect("release the worker");
        assert!(
            elapsed < Duration::from_millis(1500),
            "the frame waited {elapsed:?} for the source manifest read"
        );
        assert!(
            app.browser_create_host.io.busy(),
            "the create waits for its source manifest"
        );
        assert_eq!(
            app.board.panels.len(),
            panels_before,
            "nothing starts before the read lands"
        );
        assert!(app.browser_create_host.pending.is_empty());

        app.settle_browser_host_io();
        assert!(
            !app.browser_create_host.io.busy(),
            "the request is answered once its source manifest was read"
        );
        assert_eq!(
            app.board.panels.len(),
            panels_before,
            "a source without a started driver is refused, so no duplicate panel opens"
        );
        assert!(
            app.browser_create_host.pending.is_empty(),
            "the refused duplicate leaves no create behind"
        );
        // A second round of applies answers nothing more for the request.
        app.settle_browser_host_io();
        assert_eq!(app.board.panels.len(), panels_before);
    }
}
