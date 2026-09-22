//! Host-side handling for agent-requested browser panel closes. Closing a
//! panel drops its state, which stops the session and, for a remote session,
//! releases the provider allocation through the driver's own teardown. The
//! result is published only once that teardown has completed, so a caller
//! that sees `closed` knows the session and any device allocation are gone.

use horizon_core::browser::BrowserShutdownSignal;
use horizon_core::browser::manifest::{self, BrowserCloseAuditStatus, BrowserCloseRequest, BrowserCloseResult};
use horizon_core::{PanelId, PanelKind};

use super::HorizonApp;
use super::browser_requests::{ActorPanel, actor_panel, launched_by_this_host};

/// Why a claimed close request is refused, as the typed result code and its
/// message. Every path leaves the panel untouched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CloseRefusal {
    pub(super) code: &'static str,
    pub(super) message: &'static str,
}

const fn refusal(code: &'static str, message: &'static str) -> CloseRefusal {
    CloseRefusal { code, message }
}

/// A panel the host already closed whose session teardown is still running.
pub(super) struct PendingBrowserClose {
    request: BrowserCloseRequest,
    /// `None` when the panel had no driver: nothing is left to wait for.
    teardown: Option<BrowserShutdownSignal>,
}

impl PendingBrowserClose {
    /// Whether this close may still hold an allocation at `provider`.
    pub(super) fn holds_remote_allocation_at(&self, provider: &str) -> bool {
        self.teardown
            .as_ref()
            .is_some_and(|signal| signal.remote_provider() == Some(provider) && signal.holds_remote_allocation())
    }
}

/// Where a pending close stands at one poll.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PendingCloseState {
    Waiting,
    Complete,
    TimedOut,
}

/// Judge a pending close. The request deadline is authoritative: a teardown
/// first observed complete after it is reported as timed out, because the
/// host polls only every few hundred milliseconds and the signal carries no
/// completion time.
pub(super) const fn pending_close_state(
    teardown_complete: bool,
    deadline_at_millis: i64,
    now_millis: i64,
) -> PendingCloseState {
    if now_millis > deadline_at_millis {
        PendingCloseState::TimedOut
    } else if teardown_complete {
        PendingCloseState::Complete
    } else {
        PendingCloseState::Waiting
    }
}

/// What a completed teardown means for the caller, from the remote release
/// the driver recorded. A local browser records nothing and is simply gone.
/// Whether a finished teardown counts as closed. A local browser, or a
/// remote session whose release the driver established, is closed; a remote
/// teardown that still holds an allocation is a typed failure, whatever the
/// driver managed to report (nothing at all after an unknown allocation or a
/// driver that died before releasing counts as unknown).
pub(super) fn close_outcome(
    holds_remote_allocation: bool,
    release: Option<&horizon_core::browser::RemoteReleaseOutcome>,
) -> Result<(), (&'static str, String)> {
    use horizon_core::browser::RemoteReleaseOutcome;
    if !holds_remote_allocation {
        return Ok(());
    }
    match release {
        Some(RemoteReleaseOutcome::Failed { .. }) => Err((
            "release_failed",
            "browser panel closed but the provider refused release; use browser_remote_allocations to reconcile its exact session".to_string(),
        )),
        Some(RemoteReleaseOutcome::ReleaseUnknown { .. }) => Err((
            "release_unknown",
            "browser panel closed but release is unconfirmed; use browser_remote_allocations to reconcile its exact session".to_string(),
        )),
        None => Err((
            "release_unknown",
            "browser panel closed but its remote session's release was never established; check the provider before allocating again".to_string(),
        )),
        Some(RemoteReleaseOutcome::Released | RemoteReleaseOutcome::AlreadyGone | RemoteReleaseOutcome::NeverAllocated) => {
            Ok(())
        }
    }
}

impl HorizonApp {
    pub(super) fn poll_browser_close_requests(&mut self) -> bool {
        let mut changed = self.finish_pending_browser_closes();
        super::browser_remote_create::trim_remote_slot_leases(self);
        let requests = match manifest::list_close_requests() {
            Ok(requests) => requests,
            Err(error) => {
                tracing::warn!(error = %error, "could not poll browser close requests");
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
            let request = match manifest::claim_close_request(
                &request.request_id,
                &request.actor,
                manifest::host_instance(),
                std::process::id(),
            ) {
                Ok(Some(request)) => request,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(request_id = %request.request_id, error = %error, "could not claim browser close request");
                    continue;
                }
            };
            changed |= self.apply_browser_close_request(&request, actor_panel);
        }
        changed
    }

    /// Decide whether `request` may close a panel, from board state and the
    /// live ownership the caller read from the manifest. Pure: nothing is
    /// written, so the refusal paths are testable without a driver.
    pub(super) fn close_target(
        &self,
        request: &BrowserCloseRequest,
        actor_panel: ActorPanel,
        owned_by_actor: bool,
        now_millis: i64,
    ) -> Result<PanelId, CloseRefusal> {
        if request.deadline_at_millis < now_millis {
            return Err(refusal("request_expired", "browser close request expired"));
        }
        let panel_id = self.board.panel_id_by_local_id(&request.panel_local_id).ok_or(refusal(
            "panel_not_in_host",
            "browser panel is not hosted by the requesting agent's Horizon instance",
        ))?;
        let panel = self
            .board
            .panel(panel_id)
            .ok_or(refusal("panel_closed", "browser panel is not live"))?;
        if panel.kind != PanelKind::Browser {
            return Err(refusal("not_browser_panel", "target panel is not a browser panel"));
        }
        if panel.workspace_id != actor_panel.workspace_id {
            return Err(refusal(
                "panel_outside_workspace",
                "browser panel is outside the requesting agent's Horizon workspace",
            ));
        }
        if self.browser_create_is_pending(panel_id) {
            return Err(refusal(
                "create_pending",
                "browser panel is still being created; wait for browser_create to return",
            ));
        }
        if !owned_by_actor {
            return Err(refusal("ownership_changed", "browser panel ownership changed"));
        }
        Ok(panel_id)
    }

    fn apply_browser_close_request(&mut self, request: &BrowserCloseRequest, actor_panel: ActorPanel) -> bool {
        let owned_by_actor = manifest::read(&request.panel_local_id)
            .and_then(|manifest| {
                manifest
                    .live_owner(manifest::now_millis())
                    .map(|owner| owner.name.clone())
            })
            .as_deref()
            == Some(request.actor.as_str());
        let panel_id = match self.close_target(request, actor_panel, owned_by_actor, manifest::now_millis()) {
            Ok(panel_id) => panel_id,
            Err(refused) => {
                complete_close_failure(request, refused.code, refused.message);
                return false;
            }
        };
        // A journal that cannot be written refuses the close instead of
        // leaving a closed panel with no dispatch record.
        if let Err(error) = manifest::record_close_status(request, BrowserCloseAuditStatus::Dispatched) {
            tracing::warn!(request_id = %request.request_id, %error, "could not audit browser close dispatch");
            complete_close_failure(request, "audit_failed", "Horizon refused an unaudited close");
            return false;
        }
        let teardown = self.close_browser_panel_for_agent(panel_id, actor_panel);
        self.browser_create_host.pending_closes.push(PendingBrowserClose {
            request: request.clone(),
            teardown,
        });
        self.mark_runtime_dirty();
        true
    }

    /// The close itself, shared with the tests: the app's own close path
    /// (session teardown, render caches, transcript) plus fullscreen and
    /// focus. Focus returns to the requesting agent only when the closed
    /// panel held it; the board otherwise keeps its own choice.
    pub(super) fn close_browser_panel_for_agent(
        &mut self,
        panel_id: PanelId,
        actor_panel: ActorPanel,
    ) -> Option<BrowserShutdownSignal> {
        if self.fullscreen_panel == Some(panel_id) {
            self.fullscreen_panel = None;
        }
        let was_focused = self.board.focused == Some(panel_id);
        let teardown = self.close_panel_returning_teardown(panel_id);
        if was_focused {
            self.board.focus(actor_panel.panel_id);
        }
        teardown
    }

    /// Hand every pending close's teardown to the board before a shutdown
    /// or session switch builds its progress from the board, so exit still
    /// waits for the remote release and profile cleanup; the requests are
    /// settled as `host_shutdown` because nothing will poll them again.
    pub(super) fn retire_pending_browser_closes_for_shutdown(&mut self) {
        self.refresh_remote_recovery_scope();
        for pending in std::mem::take(&mut self.browser_create_host.pending_closes) {
            complete_close_failure(
                &pending.request,
                "host_shutdown",
                "Horizon is shutting down; the panel is closed and its session teardown continues with the exit",
            );
            if let Some(signal) = pending.teardown {
                self.board.retire_browser_shutdown_signal(signal);
            }
        }
    }

    /// Publish every close whose teardown has completed or whose request
    /// deadline passed first. A timed-out teardown keeps being joined by the
    /// board so application exit still waits for it.
    fn finish_pending_browser_closes(&mut self) -> bool {
        if self.browser_create_host.pending_closes.is_empty() {
            return false;
        }
        let now = manifest::now_millis();
        let mut changed = false;
        let mut waiting = Vec::new();
        for pending in std::mem::take(&mut self.browser_create_host.pending_closes) {
            let complete = pending.teardown.as_ref().is_none_or(BrowserShutdownSignal::is_complete);
            match pending_close_state(complete, pending.request.deadline_at_millis, now) {
                PendingCloseState::Waiting => waiting.push(pending),
                PendingCloseState::Complete => {
                    changed = true;
                    let release = pending
                        .teardown
                        .as_ref()
                        .and_then(BrowserShutdownSignal::remote_release);
                    let holds = pending
                        .teardown
                        .as_ref()
                        .is_some_and(BrowserShutdownSignal::holds_remote_allocation);
                    if let Err((code, message)) = close_outcome(holds, release.as_ref()) {
                        complete_close_failure(&pending.request, code, &message);
                        // The provider may still hold the session: the board
                        // keeps counting it against the provider's limit.
                        if let Some(signal) = pending.teardown {
                            self.board.retire_browser_shutdown_signal(signal);
                        }
                        continue;
                    }
                    match manifest::record_close_status(&pending.request, BrowserCloseAuditStatus::Completed) {
                        Ok(()) => complete_close_result(&BrowserCloseResult::closed(&pending.request)),
                        Err(error) => {
                            tracing::warn!(request_id = %pending.request.request_id, %error, "could not audit browser close completion");
                            complete_close_failure(
                                &pending.request,
                                "audit_failed",
                                "browser panel closed but its completion could not be audited",
                            );
                        }
                    }
                }
                PendingCloseState::TimedOut => {
                    changed = true;
                    complete_close_failure(
                        &pending.request,
                        "teardown_timeout",
                        "browser panel closed but its session teardown has not completed; call browser_list to confirm it is gone and check the provider before allocating again",
                    );
                    if let Some(signal) = pending.teardown {
                        self.board.retire_browser_shutdown_signal(signal);
                    }
                }
            }
        }
        self.browser_create_host.pending_closes = waiting;
        changed
    }
}

fn complete_close_failure(request: &BrowserCloseRequest, code: &str, message: &str) {
    if let Err(error) = manifest::record_close_status(request, BrowserCloseAuditStatus::Failed) {
        tracing::warn!(request_id = %request.request_id, %error, "could not append failed browser close audit");
    }
    complete_close_result(&BrowserCloseResult::failed(request, code, message));
}

fn complete_close_result(result: &BrowserCloseResult) {
    if let Err(error) = manifest::complete_close_request(result) {
        tracing::error!(request_id = %result.request_id, %error, "could not publish browser close result");
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::browser::BrowserPanelState;
    use horizon_core::{Panel, PanelContent, PanelOptions, WorkspaceId};

    use super::*;
    use crate::app::browser_requests::PendingBrowserCreateProbe;
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

    /// A browser panel without a driver, placed in `workspace`.
    fn inert_browser(app: &mut HorizonApp, id: u64, workspace: WorkspaceId) -> (PanelId, String) {
        let panel = Panel::from_content(
            PanelId(id),
            workspace,
            PanelKind::Browser,
            PanelContent::Browser(Box::new(BrowserPanelState::inert())),
        );
        let local_id = panel.local_id.clone();
        app.board.panels.push(panel);
        app.board.assign_panel_to_workspace(PanelId(id), workspace);
        (PanelId(id), local_id)
    }

    fn request(actor: &str, panel_local_id: &str, deadline_at_millis: i64) -> BrowserCloseRequest {
        BrowserCloseRequest::for_tests(actor, panel_local_id, deadline_at_millis)
    }

    #[test]
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
    fn a_claimed_close_removes_the_owned_panel_and_returns_focus_only_when_it_held_it() {
        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        let agent_id = app.board.create_panel(agent_options(), alpha).expect("agent panel");
        let actor_panel = ActorPanel {
            panel_id: agent_id,
            workspace_id: alpha,
        };
        let (browser_id, browser_local_id) = inert_browser(&mut app, 9001, alpha);
        let (other_id, _) = inert_browser(&mut app, 9002, alpha);
        app.board.focus(browser_id);
        let request = request("horizon:agent", &browser_local_id, i64::MAX);
        let target = app
            .close_target(&request, actor_panel, true, 0)
            .expect("an owned browser panel in the agent's workspace may be closed");
        assert_eq!(target, browser_id);

        let teardown = app
            .close_browser_panel_for_agent(target, actor_panel)
            .expect("a permanent close hands back its teardown signal");
        assert!(app.board.panel(browser_id).is_none(), "the panel is gone");
        assert!(
            teardown.wait(std::time::Duration::from_secs(5)),
            "a driver-less panel's teardown completes on its own"
        );
        assert_eq!(
            pending_close_state(teardown.is_complete(), i64::MAX, 0),
            PendingCloseState::Complete,
            "only then is the close reported"
        );
        assert_eq!(
            app.board.focused,
            Some(agent_id),
            "focus returns to the agent panel because the closed panel held it"
        );
        assert_eq!(
            app.close_target(&request, actor_panel, true, 0).expect_err("gone").code,
            "panel_not_in_host"
        );

        // A close of an unfocused panel leaves the board's focus alone.
        app.board.focus(agent_id);
        let _ = app.close_browser_panel_for_agent(other_id, actor_panel);
        assert!(app.board.panel(other_id).is_none());
        assert_eq!(app.board.focused, Some(agent_id));
    }

    #[test]
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
    fn refusals_keep_the_panel_and_name_the_reason() {
        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        let beta = app.board.create_workspace("beta");
        let agent_id = app.board.create_panel(agent_options(), alpha).expect("agent panel");
        let actor_panel = ActorPanel {
            panel_id: agent_id,
            workspace_id: alpha,
        };
        let (browser_id, browser_local_id) = inert_browser(&mut app, 9002, alpha);
        let (_, elsewhere_local_id) = inert_browser(&mut app, 9003, beta);
        let agent_local_id = app.board.panel(agent_id).expect("agent").local_id.clone();

        let expired = request("horizon:agent", &browser_local_id, 0);
        assert_eq!(
            app.close_target(&expired, actor_panel, true, 1)
                .expect_err("expired")
                .code,
            "request_expired"
        );
        let not_browser = request("horizon:agent", &agent_local_id, i64::MAX);
        assert_eq!(
            app.close_target(&not_browser, actor_panel, true, 0)
                .expect_err("agent panel")
                .code,
            "not_browser_panel"
        );
        let other_workspace = request("horizon:agent", &elsewhere_local_id, i64::MAX);
        assert_eq!(
            app.close_target(&other_workspace, actor_panel, true, 0)
                .expect_err("other workspace")
                .code,
            "panel_outside_workspace"
        );
        let unowned = request("horizon:agent", &browser_local_id, i64::MAX);
        assert_eq!(
            app.close_target(&unowned, actor_panel, false, 0)
                .expect_err("ownership changed")
                .code,
            "ownership_changed"
        );
        app.mark_browser_create_pending_for_tests(PendingBrowserCreateProbe {
            panel_id: browser_id,
            panel_local_id: browser_local_id.clone(),
        });
        assert_eq!(
            app.close_target(&unowned, actor_panel, true, 0)
                .expect_err("create pending")
                .code,
            "create_pending"
        );
        assert!(app.board.panel(browser_id).is_some(), "every refusal leaves the panel");
    }

    #[test]
    fn a_close_is_reported_only_once_teardown_completed_within_the_deadline() {
        assert_eq!(pending_close_state(false, 100, 50), PendingCloseState::Waiting);
        assert_eq!(pending_close_state(false, 100, 100), PendingCloseState::Waiting);
        assert_eq!(pending_close_state(true, 100, 50), PendingCloseState::Complete);
        assert_eq!(pending_close_state(true, 100, 100), PendingCloseState::Complete);
        assert_eq!(
            pending_close_state(true, 100, 101),
            PendingCloseState::TimedOut,
            "the deadline is authoritative even when teardown is observed complete afterwards"
        );
        assert_eq!(pending_close_state(false, 100, 101), PendingCloseState::TimedOut);
    }

    #[test]
    fn a_completed_teardown_is_a_close_only_when_the_release_was_established() {
        use horizon_core::browser::RemoteReleaseOutcome;
        assert!(close_outcome(false, None).is_ok(), "a local browser is simply gone");
        assert!(close_outcome(false, Some(&RemoteReleaseOutcome::Released)).is_ok());
        assert!(close_outcome(false, Some(&RemoteReleaseOutcome::AlreadyGone)).is_ok());
        assert!(close_outcome(false, Some(&RemoteReleaseOutcome::NeverAllocated)).is_ok());
        let never = close_outcome(true, None).expect_err("a held allocation with no report is unknown");
        assert_eq!(never.0, "release_unknown");
        assert!(never.1.contains("never established"), "{}", never.1);
        let failed = close_outcome(
            true,
            Some(&RemoteReleaseOutcome::Failed {
                error: "unknown error".into(),
                message: "busy".into(),
            }),
        )
        .expect_err("a refused release is not a close");
        assert_eq!(failed.0, "release_failed");
        assert!(!failed.1.contains("busy"), "provider messages stay private");
        assert!(failed.1.contains("browser_remote_allocations"));
        let unknown = close_outcome(
            true,
            Some(&RemoteReleaseOutcome::ReleaseUnknown {
                attempts: 3,
                reason: "timed out".into(),
            }),
        )
        .expect_err("an unanswered release is not a close");
        assert_eq!(unknown.0, "release_unknown");
        assert!(!unknown.1.contains("timed out"));
        assert!(unknown.1.contains("browser_remote_allocations"));
    }
}
