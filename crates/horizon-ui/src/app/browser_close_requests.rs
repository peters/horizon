//! Host-side handling for agent-requested browser panel closes. Closing a
//! panel drops its state, which stops the session and, for a remote session,
//! releases the provider allocation through the driver's own teardown. The
//! result is published only once that teardown has completed, so a caller
//! that sees `closed` knows the session and any device allocation are gone.

use std::path::{Path, PathBuf};

use horizon_core::browser::manifest::{self, BrowserCloseAuditStatus, BrowserCloseRequest, BrowserCloseResult};
use horizon_core::browser::remote_release::{self, CloseRelease, CloseReleaseFailure};
use horizon_core::browser::{BrowserShutdownSignal, RemoteAllocation};
use horizon_core::{PanelId, PanelKind};

use super::HorizonApp;
use super::browser_request_claims::{AGENT_GONE, HOST_RETIRING};
use super::browser_requests::{ActorPanel, actor_panel};

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

/// The refusal of a close whose manifest names another agent as the live owner.
const OWNERSHIP_CHANGED: CloseRefusal = refusal("ownership_changed", "browser panel ownership changed");

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

/// What a completed teardown means for the caller, from the provider evidence
/// the host holds for its exact session. A local browser records nothing and
/// is simply gone. Whether a finished teardown counts as closed is decided by
/// [`remote_release::close_release`]: a driver release the provider did not
/// confirm is settled by the exact session's own record, so a close reports
/// `release_failed` only when that record still shows the session as held.
pub(super) fn close_release(
    holds_remote_allocation: bool,
    release: Option<&horizon_core::browser::RemoteReleaseOutcome>,
    recovery: Option<horizon_core::browser::RemoteRecoveryStatus>,
) -> CloseRelease {
    if !holds_remote_allocation {
        return CloseRelease::Established;
    }
    remote_release::close_release(release, recovery)
}

impl HorizonApp {
    /// Publishes the closes whose teardown settled; the coordination worker
    /// claims new requests.
    pub(super) fn poll_browser_close_requests(&mut self) -> bool {
        let changed = self.finish_pending_browser_closes();
        super::browser_remote_slots::trim_remote_slot_leases(self);
        changed
    }

    /// Decide whether `request` may close a panel, from board state and the
    /// live ownership a manifest read found. Pure: nothing is written, so the
    /// refusal paths are testable without a driver.
    pub(super) fn close_target(
        &self,
        request: &BrowserCloseRequest,
        actor_panel: ActorPanel,
        owned_by_actor: bool,
        now_millis: i64,
    ) -> Result<PanelId, CloseRefusal> {
        let panel_id = self.board_close_target(request, actor_panel, now_millis)?;
        if !owned_by_actor {
            return Err(OWNERSHIP_CHANGED);
        }
        Ok(panel_id)
    }

    /// The refusals that board state alone decides, for a close whose manifest
    /// no thread has read yet: the coordination worker reads its ownership.
    fn board_close_target(
        &self,
        request: &BrowserCloseRequest,
        actor_panel: ActorPanel,
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
        Ok(panel_id)
    }

    /// Audits the dispatch of a claimed close on the coordination worker: the
    /// worker reads the ownership its manifest names and writes the dispatch
    /// record, so neither runs on the UI thread. The board then closes the
    /// panel, or refuses it with the ownership the worker read; a journal that
    /// cannot be written refuses the close instead of leaving a closed panel
    /// with no dispatch record.
    pub(super) fn apply_browser_close_request(&mut self, request: &BrowserCloseRequest, actor_panel: ActorPanel) {
        // Nothing the manifest carries is needed to refuse a close the board
        // cannot apply, so it is refused without waiting for the worker.
        if let Err(refused) = self.board_close_target(request, actor_panel, manifest::now_millis()) {
            self.fail_close(request, refused.code, refused.message);
            return;
        }
        let root = self.close_files_root();
        let audited = request.clone();
        let request = request.clone();
        self.browser_create_host.io.then(
            move || dispatch_close(&root, &audited),
            move |app, dispatched| match dispatched {
                Some(CloseDispatch { owned, audited: true }) => app.close_dispatched_browser_panel(request, owned),
                Some(_) => {}
                // The audit's outcome is unknown; nothing closes without it.
                None => app.fail_close(&request, "audit_failed", "Horizon refused an unaudited close"),
            },
        );
    }

    /// The Horizon home that keeps this host's close requests, audits and
    /// results. Production builds the session store from `HorizonHome::resolve()`,
    /// the same root the requesting agent's driver uses for its default paths.
    fn close_files_root(&self) -> PathBuf {
        self.session_store.home().root().to_path_buf()
    }

    /// Closes the panel of an audited close, decided again on the board as it
    /// is now and on the ownership the worker read.
    fn close_dispatched_browser_panel(&mut self, request: BrowserCloseRequest, owned: bool) {
        if self.browser_host_retiring() {
            self.fail_close(&request, HOST_RETIRING.0, HOST_RETIRING.1);
            return;
        }
        let target = actor_panel(&self.board, &request.actor)
            .ok_or(refusal("workspace_unavailable", AGENT_GONE))
            .and_then(|actor_panel| {
                let panel_id = self.close_target(&request, actor_panel, owned, manifest::now_millis())?;
                Ok((panel_id, actor_panel))
            });
        let (panel_id, actor_panel) = match target {
            Ok(target) => target,
            Err(refused) => {
                self.fail_close(&request, refused.code, refused.message);
                return;
            }
        };
        let teardown = self.close_browser_panel_for_agent(panel_id, actor_panel);
        self.browser_create_host
            .pending_closes
            .push(PendingBrowserClose { request, teardown });
        self.mark_runtime_dirty();
    }

    /// Publishes a refused or failed close on the coordination worker.
    pub(super) fn fail_close(&mut self, request: &BrowserCloseRequest, code: &str, message: &str) {
        let root = self.close_files_root();
        let request = request.clone();
        let (code, message) = (code.to_string(), message.to_string());
        self.browser_create_host
            .io
            .write(move || complete_close_failure(&root, &request, &code, &message));
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
            self.fail_close(
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
    /// board so application exit still waits for it. A teardown whose release
    /// the provider did not confirm keeps the close open while its exact
    /// session is checked, so the answer comes from provider evidence.
    fn finish_pending_browser_closes(&mut self) -> bool {
        if self.browser_create_host.pending_closes.is_empty() {
            return false;
        }
        let now = manifest::now_millis();
        let mut changed = false;
        let mut waiting = Vec::new();
        for pending in std::mem::take(&mut self.browser_create_host.pending_closes) {
            let complete = pending.teardown.as_ref().is_none_or(BrowserShutdownSignal::is_complete);
            let state = pending_close_state(complete, pending.request.deadline_at_millis, now);
            match state {
                PendingCloseState::Waiting => waiting.push(pending),
                PendingCloseState::TimedOut if !complete => {
                    changed = true;
                    self.fail_close(
                        &pending.request,
                        "teardown_timeout",
                        "browser panel closed but its session teardown has not completed; call browser_list to confirm it is gone and check the provider before allocating again",
                    );
                    self.retire_close_teardown(pending.teardown);
                }
                PendingCloseState::Complete | PendingCloseState::TimedOut => {
                    let past_deadline = state == PendingCloseState::TimedOut;
                    match settled_close(&pending, past_deadline) {
                        Some(Ok(())) => {
                            changed = true;
                            self.publish_closed_close(pending.request);
                        }
                        Some(Err(failure)) => {
                            changed = true;
                            self.fail_close(&pending.request, failure.code, &failure.message);
                            // The provider may still hold the session: the board
                            // keeps counting it against the provider's limit.
                            self.retire_close_teardown(pending.teardown);
                        }
                        // The exact session's check has not answered yet.
                        None => waiting.push(pending),
                    }
                }
            }
        }
        self.browser_create_host.pending_closes = waiting;
        changed
    }

    /// Publish a close whose release the provider established.
    fn publish_closed_close(&mut self, request: BrowserCloseRequest) {
        let root = self.close_files_root();
        self.browser_create_host.io.write(move || {
            match manifest::record_close_status_in(&root, &request, BrowserCloseAuditStatus::Completed) {
                Ok(()) => complete_close_result(&root, &BrowserCloseResult::closed(&request)),
                Err(error) => {
                    tracing::warn!(request_id = %request.request_id, %error, "could not audit browser close completion");
                    complete_close_failure(
                        &root,
                        &request,
                        "audit_failed",
                        "browser panel closed but its completion could not be audited",
                    );
                }
            }
        });
    }

    /// Hand a teardown the host stopped observing to the board, so exit still
    /// joins it and an unreleased allocation keeps counting.
    fn retire_close_teardown(&mut self, teardown: Option<BrowserShutdownSignal>) {
        if let Some(signal) = teardown {
            self.board.retire_browser_shutdown_signal(signal);
        }
    }
}

/// Decide one closed panel's answer from provider evidence. `None` keeps the
/// close open because the exact session's check has not answered yet; the
/// check runs at the provider, so the frame never waits for it.
fn settled_close(pending: &PendingBrowserClose, past_deadline: bool) -> Option<Result<(), CloseReleaseFailure>> {
    // A panel without a teardown signal (a cloud browser panel, or a browser
    // panel with no session signal) has no driver left to wait for and no
    // provider session of its own.
    let Some(teardown) = pending.teardown.as_ref() else {
        return Some(Ok(()));
    };
    let allocation = teardown.remote_recovery();
    let answer = close_release(
        teardown.holds_remote_allocation(),
        teardown.remote_release().as_ref(),
        allocation.as_ref().map(RemoteAllocation::status),
    );
    if !answer.needs_check() {
        return Some(answer.refusal().map_or(Ok(()), Err));
    }
    if past_deadline {
        return Some(Err(CloseReleaseFailure::unconfirmed(remote_release::CHECK_PENDING)));
    }
    // The exact session is retired once the driver ended, so the check reads
    // the provider and records its own status there.
    allocation.as_ref()?.reconcile();
    None
}

/// What the coordination worker found and wrote for one close request.
struct CloseDispatch {
    /// Whether the agent the request names holds the panel's live lease.
    owned: bool,
    /// Whether the dispatch state reached the panel's audit journal.
    audited: bool,
}

/// Reads the ownership the request's manifest names and audits the dispatch, in
/// the Horizon home that hosts the panel, on the coordination worker.
fn dispatch_close(root: &Path, request: &BrowserCloseRequest) -> CloseDispatch {
    let owned = owned_by_actor(root, request);
    let audited = manifest::record_close_status_in(root, request, BrowserCloseAuditStatus::Dispatched);
    if let Err(error) = &audited {
        tracing::warn!(request_id = %request.request_id, %error, "could not audit browser close dispatch");
        complete_close_failure(root, request, "audit_failed", "Horizon refused an unaudited close");
    }
    CloseDispatch {
        owned,
        audited: audited.is_ok(),
    }
}

/// Whether the request's actor owns the panel now, by the live owner its
/// manifest in `root` names.
fn owned_by_actor(root: &Path, request: &BrowserCloseRequest) -> bool {
    manifest::read_at(&manifest::manifest_path_for_root(root, &request.panel_local_id))
        .and_then(|manifest| {
            manifest
                .live_owner(manifest::now_millis())
                .map(|owner| owner.name.clone())
        })
        .as_deref()
        == Some(request.actor.as_str())
}

fn complete_close_failure(root: &Path, request: &BrowserCloseRequest, code: &str, message: &str) {
    if let Err(error) = manifest::record_close_status_in(root, request, BrowserCloseAuditStatus::Failed) {
        tracing::warn!(request_id = %request.request_id, %error, "could not append failed browser close audit");
    }
    complete_close_result(root, &BrowserCloseResult::failed(request, code, message));
}

fn complete_close_result(root: &Path, result: &BrowserCloseResult) {
    if let Err(error) = manifest::complete_close_request_in(root, result) {
        tracing::error!(request_id = %result.request_id, %error, "could not publish browser close result");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    use horizon_core::browser::manifest::BrowserCloseOutcome;
    use horizon_core::browser::{BrowserPanelState, RemoteRecoveryStatus, RemoteReleaseOutcome};
    use horizon_core::{Panel, PanelContent, PanelOptions, WorkspaceId};

    use super::*;
    use crate::app::browser_create_lifecycle::PendingBrowserCreateProbe;
    use crate::app::test_support::test_app;

    fn agent_options() -> PanelOptions {
        let (command, args) = ("/bin/sh", vec!["-c".to_string(), "exit 0".to_string()]);
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

    /// A manifest that names `actor` as the panel's live owner.
    fn live_manifest(actor: &str) -> manifest::BrowserManifest {
        manifest::BrowserManifest {
            host: Some(manifest::host_instance().to_string()),
            owner: Some(manifest::ManifestOwner {
                name: actor.to_string(),
                tty: None,
                updated_at: manifest::now_millis(),
            }),
            ..manifest::BrowserManifest::default()
        }
    }

    #[test]
    fn a_blocked_coordination_worker_never_stalls_the_close_request_frame() {
        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        let agent_id = app.board.create_panel(agent_options(), alpha).expect("agent panel");
        let actor = horizon_core::browser_actor(&app.board.panel(agent_id).expect("agent").local_id);
        let actor_panel = ActorPanel {
            panel_id: agent_id,
            workspace_id: alpha,
        };
        let (browser_id, browser_local_id) = inert_browser(&mut app, 9101, alpha);
        // The panel's manifest names the requesting agent as its live owner, so
        // the close applies once the worker read it.
        let path = manifest::manifest_path_for_root(app.session_store.home().root(), &browser_local_id);
        manifest::write_at(&path, &live_manifest(&actor)).expect("live manifest");
        let close = request(&actor, &browser_local_id, i64::MAX);

        // The coordination worker waits, as it does for a manifest whose read
        // waits on a blocked file.
        let (release, released) = channel::<()>();
        app.browser_create_host.io.write(move || {
            let _ = released.recv();
        });

        let started = Instant::now();
        app.apply_browser_close_request(&close, actor_panel);
        let elapsed = started.elapsed();
        release.send(()).expect("release the worker");
        assert!(
            elapsed < Duration::from_millis(1500),
            "the frame waited {elapsed:?} for the close's manifest read"
        );
        assert!(app.browser_create_host.io.busy(), "the close waits on the worker");
        assert!(
            app.board.panel(browser_id).is_some(),
            "nothing closes before the dispatch audit lands"
        );

        app.settle_browser_host_io();
        assert!(
            app.board.panel(browser_id).is_none(),
            "the close applies once the worker read the ownership and audited it"
        );
        assert_eq!(
            app.browser_create_host.pending_closes.len(),
            1,
            "the close waits for its session teardown"
        );

        // The close audits and publishes its result in this Horizon home.
        let published_by = Instant::now() + Duration::from_secs(10);
        while !app.browser_create_host.pending_closes.is_empty() {
            assert!(Instant::now() < published_by, "the close did not publish its result");
            app.poll_browser_close_requests();
            app.settle_browser_host_io();
            std::thread::sleep(Duration::from_millis(5));
        }
        let root = app.session_store.home().root();
        let journal = std::fs::read_to_string(manifest::audit_path_for_root(root, &browser_local_id))
            .expect("close audit journal");
        assert!(
            journal.contains("\"status\":\"dispatched\""),
            "the worker audited the dispatch it read the ownership for: {journal}"
        );
        assert!(
            journal.contains("\"status\":\"completed\""),
            "the close reports completed: {journal}"
        );
    }

    #[test]
    fn the_close_reads_the_panel_manifest_on_the_coordination_worker() {
        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        let agent_id = app.board.create_panel(agent_options(), alpha).expect("agent panel");
        let actor = horizon_core::browser_actor(&app.board.panel(agent_id).expect("agent").local_id);
        let actor_panel = ActorPanel {
            panel_id: agent_id,
            workspace_id: alpha,
        };
        let (browser_id, browser_local_id) = inert_browser(&mut app, 9103, alpha);
        // The manifest is a FIFO, so the read of it blocks until the writer
        // publishes the ownership. Only a reader on another thread leaves the
        // frame free to return.
        let path = manifest::manifest_path_for_root(app.session_store.home().root(), &browser_local_id);
        std::fs::create_dir_all(path.parent().expect("manifest directory")).expect("manifest directory");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .expect("mkfifo runs")
                .success(),
            "a FIFO stands in for the manifest"
        );
        let bytes = serde_json::to_vec(&live_manifest(&actor)).expect("manifest bytes");
        let (permit, permitted) = channel::<()>();
        let (opened, reader) = channel::<()>();
        let (reported, released) = channel::<bool>();
        let written = path.clone();
        let writer = std::thread::spawn(move || {
            use std::io::Write as _;
            let mut manifest = std::fs::OpenOptions::new()
                .write(true)
                .open(&written)
                .expect("a reader opens the manifest");
            opened.send(()).expect("the reader is known");
            // A read that ran on the frame would wait for this permit, so the
            // writer gives up on its own instead of hanging the test.
            let prompt = permitted.recv_timeout(Duration::from_secs(5)).is_ok();
            reported.send(prompt).expect("the outcome is reported");
            manifest.write_all(&bytes).expect("the ownership is published");
            drop(manifest);
            std::fs::remove_file(&written).expect("the FIFO is replaced");
            std::fs::write(&written, &bytes).expect("the manifest stays readable");
        });

        let started = Instant::now();
        app.apply_browser_close_request(&request(&actor, &browser_local_id, i64::MAX), actor_panel);
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(1500),
            "the frame waited {elapsed:?} for the manifest read"
        );
        assert!(app.browser_create_host.io.busy(), "the close waits on the worker");
        reader
            .recv_timeout(Duration::from_secs(10))
            .expect("the worker is the reader of the manifest");
        assert!(
            app.board.panel(browser_id).is_some(),
            "nothing closes before the ownership is read"
        );

        permit.send(()).expect("the ownership is published");
        assert!(
            released
                .recv_timeout(Duration::from_secs(10))
                .expect("the writer reports"),
            "the frame did not wait for the manifest read"
        );
        writer.join().expect("the writer ends");
        app.settle_browser_host_io();
        assert!(
            app.board.panel(browser_id).is_none(),
            "the close applies from the ownership the worker read"
        );
    }

    #[test]
    fn a_close_the_worker_reads_as_another_agents_keeps_the_panel() {
        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        let agent_id = app.board.create_panel(agent_options(), alpha).expect("agent panel");
        let actor = horizon_core::browser_actor(&app.board.panel(agent_id).expect("agent").local_id);
        let actor_panel = ActorPanel {
            panel_id: agent_id,
            workspace_id: alpha,
        };
        let (browser_id, browser_local_id) = inert_browser(&mut app, 9102, alpha);
        // Board state alone permits this close; only the manifest names a live
        // owner that is not the requesting agent, and the coordination worker
        // is what reads it.
        let path = manifest::manifest_path_for_root(app.session_store.home().root(), &browser_local_id);
        manifest::write_at(&path, &live_manifest("horizon:other-agent")).expect("live manifest of another agent");

        app.apply_browser_close_request(&request(&actor, &browser_local_id, i64::MAX), actor_panel);
        app.settle_browser_host_io();

        assert!(
            app.board.panel(browser_id).is_some(),
            "a close the worker read as another agent's leaves the panel"
        );
        assert!(
            app.browser_create_host.pending_closes.is_empty(),
            "a refused close has no teardown to wait for"
        );
        let root = app.session_store.home().root();
        let journal = std::fs::read_to_string(manifest::audit_path_for_root(root, &browser_local_id))
            .expect("close audit journal");
        assert!(
            journal.contains("\"status\":\"dispatched\""),
            "the dispatch is audited with the ownership read: {journal}"
        );
        assert!(
            journal.contains("\"status\":\"failed\""),
            "the refused close publishes a failure: {journal}"
        );
    }

    #[test]
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
    fn a_completed_teardown_is_a_close_only_when_provider_evidence_established_the_release() {
        let refused = RemoteReleaseOutcome::Failed {
            error: "unknown error".into(),
            message: "busy".into(),
        };
        assert_eq!(
            close_release(false, None, None),
            CloseRelease::Established,
            "a local browser is simply gone"
        );
        for release in [
            RemoteReleaseOutcome::Released,
            RemoteReleaseOutcome::AlreadyGone,
            RemoteReleaseOutcome::NeverAllocated,
        ] {
            assert_eq!(close_release(false, Some(&release), None), CloseRelease::Established);
            assert_eq!(close_release(true, Some(&release), None), CloseRelease::Established);
        }

        // The reported bug: the provider did not confirm the release request,
        // and its exact session record proved the session was gone.
        for established in [
            RemoteRecoveryStatus::Released,
            RemoteRecoveryStatus::PersistenceUnavailable,
        ] {
            assert_eq!(
                close_release(true, Some(&refused), Some(established)),
                CloseRelease::Established,
                "{established:?} proves the release"
            );
        }

        let retained = close_release(true, Some(&refused), Some(RemoteRecoveryStatus::Active))
            .refusal()
            .expect("a session the provider still holds fails the close");
        assert_eq!(retained.code, "release_failed");
        assert!(retained.message.contains("still reports its exact session as active"));
        assert!(!retained.message.contains("busy"), "provider messages stay private");
        assert!(retained.message.contains("reconcile"));

        let rejected = close_release(true, None, Some(RemoteRecoveryStatus::AuthenticationRequired))
            .refusal()
            .expect("a release that could not run is not a close");
        assert_eq!(rejected.code, "release_unknown");
        assert!(
            rejected.message.contains("the original credential was rejected"),
            "{}",
            rejected.message
        );

        let never = close_release(true, None, None)
            .refusal()
            .expect("a held allocation with no report is unknown");
        assert_eq!(never.code, "release_unknown");
        assert!(never.message.contains("no exact session identity"), "{}", never.message);
        let unknown = close_release(
            true,
            Some(&RemoteReleaseOutcome::ReleaseUnknown {
                attempts: 3,
                reason: "timed out".into(),
            }),
            Some(RemoteRecoveryStatus::ProviderUnavailable),
        )
        .refusal()
        .expect("an unanswered release is not a close");
        assert_eq!(unknown.code, "release_unknown");
        assert!(!unknown.message.contains("timed out"));
        assert!(
            unknown.message.contains("no trustworthy session result"),
            "{}",
            unknown.message
        );

        // A retired session whose release nothing established is checked at
        // the provider before the close answers.
        assert!(close_release(true, Some(&refused), Some(RemoteRecoveryStatus::Unresolved)).needs_check());
    }

    /// Every close result the host published in its Horizon home, oldest id first.
    fn published_close_results(app: &mut HorizonApp) -> Vec<BrowserCloseResult> {
        app.settle_browser_host_io();
        let directory = app.session_store.home().root().join("runtime").join("browser-close");
        let mut results: Vec<BrowserCloseResult> = std::fs::read_dir(directory)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".result.json"))
            .filter_map(|entry| std::fs::read(entry.path()).ok())
            .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
            .collect();
        results.sort_by_key(|result| result.request_id.clone());
        results
    }

    #[test]
    fn a_close_without_a_teardown_signal_is_answered_at_once() {
        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        let (_, browser_local_id) = inert_browser(&mut app, 9203, alpha);
        app.browser_create_host.pending_closes.push(PendingBrowserClose {
            request: request("horizon:agent", &browser_local_id, i64::MAX),
            teardown: None,
        });

        assert!(app.poll_browser_close_requests());
        let published = published_close_results(&mut app);
        assert_eq!(published.len(), 1, "the close is answered once: {published:?}");
        assert_eq!(published[0].outcome, BrowserCloseOutcome::Closed, "{published:?}");
        assert!(
            app.browser_create_host.pending_closes.is_empty(),
            "nothing is left to wait for"
        );

        // A driver-less close that is first observed after its deadline is
        // still answered, not left waiting for a teardown that never existed.
        app.browser_create_host.pending_closes.push(PendingBrowserClose {
            request: request("horizon:agent", &browser_local_id, 0),
            teardown: None,
        });
        app.poll_browser_close_requests();
        assert_eq!(
            published_close_results(&mut app).len(),
            2,
            "the late close is answered too"
        );
        assert!(app.browser_create_host.pending_closes.is_empty());
    }

    #[test]
    fn a_refused_release_the_provider_proved_released_is_a_closed_panel() {
        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        let (_, browser_local_id) = inert_browser(&mut app, 9201, alpha);
        let allocation = RemoteAllocation::default();
        allocation.cancel_before_launch();
        assert!(allocation.is_released());

        app.browser_create_host.pending_closes.push(PendingBrowserClose {
            request: request("horizon:agent", &browser_local_id, i64::MAX),
            teardown: Some(BrowserShutdownSignal::completed_remote_recovery_for_test(
                "grid",
                Some(RemoteReleaseOutcome::Failed {
                    error: "release refused".into(),
                    message: "the provider did not confirm the exact session release".into(),
                }),
                Some(allocation),
            )),
        });

        assert!(app.poll_browser_close_requests());
        let published = published_close_results(&mut app);
        assert_eq!(published.len(), 1, "the close is answered once: {published:?}");
        assert_eq!(published[0].outcome, BrowserCloseOutcome::Closed, "{published:?}");
    }

    #[test]
    fn a_close_whose_exact_session_was_not_checked_yet_waits_for_the_provider() {
        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        let (_, browser_local_id) = inert_browser(&mut app, 9202, alpha);
        // No provider answers on this endpoint, so the check itself fails and
        // the close reports the cause instead of a refusal.
        let allocation = RemoteAllocation::unresolved_for_test("http://127.0.0.1:1/wd/hub", "fixture-session")
            .expect("unresolved allocation");

        app.browser_create_host.pending_closes.push(PendingBrowserClose {
            request: request("horizon:agent", &browser_local_id, manifest::now_millis() + 60_000),
            teardown: Some(BrowserShutdownSignal::completed_remote_recovery_for_test(
                "grid",
                Some(RemoteReleaseOutcome::Failed {
                    error: "release refused".into(),
                    message: "private provider text".into(),
                }),
                Some(allocation),
            )),
        });

        // The first poll starts the check and keeps the close open.
        app.poll_browser_close_requests();
        assert_eq!(
            app.browser_create_host.pending_closes.len(),
            1,
            "the close waits for the check at the provider"
        );
        assert!(
            published_close_results(&mut app).is_empty(),
            "nothing is published before the provider answers"
        );

        let deadline = Instant::now() + Duration::from_secs(20);
        let published = loop {
            app.poll_browser_close_requests();
            let published = published_close_results(&mut app);
            if let Some(result) = published.first() {
                break result.clone();
            }
            assert!(Instant::now() < deadline, "the close is never answered");
            std::thread::sleep(Duration::from_millis(10));
        };
        let BrowserCloseOutcome::Failed { code, message } = published.outcome else {
            panic!("an unchecked release is not a close: {published:?}");
        };
        assert_eq!(code, "release_unknown");
        assert!(message.contains("no trustworthy session result"), "{message}");
        assert!(!message.contains("private provider text"), "{message}");
    }
}
