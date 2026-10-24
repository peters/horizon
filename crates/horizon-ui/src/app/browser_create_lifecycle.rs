//! The pending browser creates a host starts, judges and retires.

use std::time::{Duration, Instant};

use horizon_core::browser::manifest::{self, BrowserCreateRequest, BrowserCreateResult, CreateNavigation};
use horizon_core::browser::{BackendKind, BrowserStatus};
use horizon_core::{Board, PanelId};

use super::HorizonApp;
use super::browser_create_dispatch::publish_requested_create;
use super::browser_create_failure::{CREATE_TIMEOUT, record_and_complete_failure, terminal_create_failure};
use super::browser_host_io::{BoardFence, HostIo};
use super::browser_manifest_stamp::{browser_workspace, host_identity, publish_manifest_host_state};
use super::browser_request_claims::HOST_RETIRING;

/// How long a create with an initial URL waits, after the backend is ready,
/// for that page to commit before reporting the panel with
/// `navigation: pending`. Bounded so creation never waits for a slow first
/// page; the overall create deadline still caps it.
const STARTUP_NAVIGATION_BUDGET: Duration = Duration::from_secs(10);

/// Headroom kept before the create deadline for the host's own completion
/// work (ownership update, audit, result write). Delivery of that result to
/// the caller is covered separately by the MCP controller, which waits
/// `RESULT_DELIVERY_HEADROOM_MILLIS` beyond the request deadline.
const STARTUP_DEADLINE_HEADROOM: Duration = Duration::from_millis(750);

pub(super) struct PendingBrowserCreate {
    pub(super) request: BrowserCreateRequest,
    /// The panel, by its persisted id: board ids restart with a new session.
    pub(super) panel_local_id: String,
    pub(super) backend: BackendKind,
    /// Resolved once from the launch plan; later configuration or device state
    /// must not change the orientation recorded for this creation.
    pub(super) startup_orientation: Option<horizon_core::browser::remote::RemoteOrientation>,
    /// When the host claimed the request, for the reported startup latency.
    pub(super) started_at: Instant,
    /// When the backend first reported `Ready`, which starts the bounded
    /// startup-navigation wait.
    pub(super) ready_since: Option<Instant>,
    /// User navigations the panel had seen when the create started; more
    /// means the user took the panel over before the first page committed.
    pub(super) user_navigations_at_start: u32,
    pub(super) stage: CreateStage,
    /// The board it was asked on; a create of a replaced board is not one of a restored panel.
    pub(super) board: BoardFence,
}

/// Where a pending create stands. Only a starting create is judged each
/// poll; the others wait for their coordination work, which ends them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CreateStage {
    /// Its dispatch is being audited; an unaudited create never completes.
    Auditing,
    /// The browser starts.
    Starting,
    /// Its ownership, audit and result are being published. A panel that
    /// closes or leaves the agent's workspace during those few milliseconds
    /// is still reported ready; the next stamp writes the board's placement.
    Finishing,
}

/// The panel a test wants treated as still being created.
#[cfg(test)]
pub(super) struct PendingBrowserCreateProbe {
    pub(super) panel_local_id: String,
}

/// Whether a pending create may complete, decided from the panel's live
/// browser state rather than from the manifest file's existence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CreateReadiness {
    Waiting,
    Ready(CreateNavigation),
}

/// Live page state a pending create is judged on.
#[derive(Clone, Copy)]
struct PageReadiness<'a> {
    status: &'a BrowserStatus,
    /// URL the live panel reports as committed.
    committed_url: Option<&'a str>,
    /// URL in the manifest the agent reads.
    manifest_url: &'a str,
    /// The browser's navigation failure, when the last navigation failed.
    navigation_error: Option<&'a str>,
}

/// The backend must report `Ready`. With an initial URL, the page must also
/// have committed (in the live panel and in the manifest the agent reads);
/// a failed first navigation is reported as `failed` at once, an explicit
/// `about:blank` is the committed blank destination, and when the bounded
/// startup wait or the create deadline is about to elapse the panel is
/// reported with `navigation: pending`.
fn create_readiness(
    page: PageReadiness<'_>,
    requested_url: Option<&str>,
    user_navigated: bool,
    ready_since: Option<Instant>,
    deadline: Instant,
    now: Instant,
) -> CreateReadiness {
    if *page.status != BrowserStatus::Ready {
        return CreateReadiness::Waiting;
    }
    let Some(requested) = requested_url.filter(|url| !url.is_empty()) else {
        return CreateReadiness::Ready(CreateNavigation::NotRequested);
    };
    if user_navigated {
        // The user took the panel over before the requested page committed:
        // whatever commits now is theirs, not the requested first page.
        return CreateReadiness::Ready(CreateNavigation::Superseded);
    }
    if requested == "about:blank" {
        // Both drivers skip navigating to the blank page and report it as an
        // empty URL: the panel is already at the requested destination.
        return CreateReadiness::Ready(CreateNavigation::Committed);
    }
    let committed = page.committed_url.is_some_and(|url| !url.is_empty());
    // The manifest the agent reads is flushed on a cadence; during a redirect
    // it can still name an earlier document while the live panel already
    // holds the final one, so both must agree before the create reports the
    // page as committed.
    if committed && page.committed_url == Some(page.manifest_url) {
        return CreateReadiness::Ready(CreateNavigation::Committed);
    }
    if page.navigation_error.is_some() && !committed {
        return CreateReadiness::Ready(CreateNavigation::Failed);
    }
    let ready_since = ready_since.unwrap_or(now);
    let budget_end = (ready_since + STARTUP_NAVIGATION_BUDGET)
        .min(deadline.checked_sub(STARTUP_DEADLINE_HEADROOM).unwrap_or(deadline));
    if now >= budget_end {
        CreateReadiness::Ready(CreateNavigation::Pending)
    } else {
        CreateReadiness::Waiting
    }
}

enum BrowserCreateCompletion {
    Waiting,
    Completed,
    Failed,
    /// Its publication runs on the coordination worker, which ends it.
    Finishing,
}

impl HorizonApp {
    /// Whether an agent create for this panel has not completed yet.
    pub(super) fn browser_create_is_pending(&self, panel_id: PanelId) -> bool {
        let Some(panel) = self.board.panel(panel_id) else {
            return false;
        };
        self.browser_create_host
            .pending
            .iter()
            .any(|pending| pending.panel_local_id == panel.local_id && pending.board.holds())
    }

    pub(super) fn finish_pending_browser_creates(&mut self) -> bool {
        if self.browser_create_host.pending.is_empty() {
            return false;
        }
        let mut changed = false;
        let mut waiting = Vec::new();
        for mut pending in std::mem::take(&mut self.browser_create_host.pending) {
            if pending.stage != CreateStage::Starting {
                waiting.push(pending);
                continue;
            }
            let io = &mut self.browser_create_host.io;
            if browser_create_is_terminal(&self.board, io, &pending) {
                changed = true;
                continue;
            }
            match finish_ready_browser_create(&self.board, io, &mut pending) {
                BrowserCreateCompletion::Waiting => waiting.push(pending),
                BrowserCreateCompletion::Completed => changed = true,
                BrowserCreateCompletion::Failed => {
                    if let Some(panel_id) = self.board.panel_id_by_local_id(&pending.panel_local_id) {
                        self.close_panel(panel_id);
                    }
                    changed = true;
                }
                BrowserCreateCompletion::Finishing => {
                    pending.stage = CreateStage::Finishing;
                    waiting.push(pending);
                    changed = true;
                }
            }
        }
        self.browser_create_host.pending = waiting;
        changed
    }

    /// Refuses every create whose browser still starts, which nothing polls once Horizon exits or switches
    /// sessions; the teardown closes its panel. A create whose coordination work runs ends with that work.
    pub(super) fn retire_starting_browser_creates(&mut self) {
        let host = &mut self.browser_create_host;
        let (starting, waiting): (Vec<_>, _) = std::mem::take(&mut host.pending)
            .into_iter()
            .partition(|pending| pending.stage == CreateStage::Starting);
        host.pending = waiting;
        for pending in &starting {
            record_and_complete_failure(&mut host.io, pending, HOST_RETIRING.0, HOST_RETIRING.1);
        }
    }

    /// Ends a create whose publication finished; one that could not be
    /// published closes its panel, as its failure result says.
    fn finish_published_create(&mut self, request_id: &str, published: Option<bool>) {
        let pending = &mut self.browser_create_host.pending;
        let Some(index) = pending
            .iter()
            .position(|pending| pending.request.request_id == request_id)
        else {
            return;
        };
        let pending = pending.remove(index);
        if published == Some(true) {
            return;
        }
        // A replaced board took the panel with it; a panel of the same id now is another one.
        if !self.browser_create_host.io.applying_stale()
            && let Some(panel_id) = self.board.panel_id_by_local_id(&pending.panel_local_id)
        {
            self.close_panel(panel_id);
        }
        if published.is_none() {
            record_and_complete_failure(
                &mut self.browser_create_host.io,
                &pending,
                "manifest_update_failed",
                "Horizon could not publish the new browser panel's visibility and workspace",
            );
        }
    }
}

fn finish_ready_browser_create(
    board: &Board,
    io: &mut HostIo,
    pending: &mut PendingBrowserCreate,
) -> BrowserCreateCompletion {
    let Some(browser) = pending_panel(board, pending).and_then(|panel| panel.browser()) else {
        return BrowserCreateCompletion::Waiting;
    };
    let now = Instant::now();
    if browser.status == BrowserStatus::Ready {
        pending.ready_since.get_or_insert(now);
    }
    // The driver writes its manifest before the backend is ready and before
    // the initial navigation commits, so the file's existence alone must not
    // complete the create.
    let Some(current) = manifest::read(&pending.panel_local_id) else {
        return BrowserCreateCompletion::Waiting;
    };
    // The manifest read can be slow near a boundary; decide and measure on a
    // clock read taken after it, keeping the earlier instant only for
    // observing `Ready`.
    let now = Instant::now();
    let deadline = create_deadline(pending, now);
    let CreateReadiness::Ready(navigation) = create_readiness(
        PageReadiness {
            status: &browser.status,
            committed_url: browser.url.as_deref(),
            manifest_url: &current.url,
            navigation_error: browser.navigation_error.as_deref(),
        },
        pending.request.url.as_deref(),
        browser.user_navigation_count() > pending.user_navigations_at_start,
        pending.ready_since,
        deadline,
        now,
    ) else {
        return BrowserCreateCompletion::Waiting;
    };
    let navigation_error = (navigation == CreateNavigation::Failed)
        .then(|| browser.navigation_error.clone())
        .flatten();
    let startup_millis =
        u64::try_from(now.saturating_duration_since(pending.started_at).as_millis()).unwrap_or(u64::MAX);
    let Some(workspace) = pending_panel(board, pending).and_then(|panel| browser_workspace(board, panel.workspace_id))
    else {
        record_and_complete_failure(
            io,
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
        let (panel_local_id, visible) = (pending.panel_local_id.clone(), pending.request.visible);
        let request_id = pending.request.request_id.clone();
        io.write_for_board(move || {
            if let Err(error) = publish_manifest_host_state(&panel_local_id, visible, &workspace) {
                tracing::warn!(%request_id, %error, "could not stamp a moved browser panel");
            }
        });
        record_and_complete_failure(
            io,
            pending,
            "workspace_changed",
            "the browser panel left the requesting agent's workspace during creation and was left in place",
        );
        return BrowserCreateCompletion::Completed;
    }
    let result = BrowserCreateResult::ready(
        &pending.request,
        pending.panel_local_id.clone(),
        navigation,
        navigation_error,
        startup_millis,
    );
    let (panel, request, live) = (pending.panel_local_id.clone(), pending.request.clone(), io.live_fence());
    let (backend, orientation) = (pending.backend, pending.startup_orientation);
    io.then(
        move || publish_requested_create(&live, &panel, &request, &workspace, backend, orientation, &result),
        {
            let request_id = pending.request.request_id.clone();
            move |app, published| app.finish_published_create(&request_id, published)
        },
    );
    BrowserCreateCompletion::Finishing
}

/// The pending create's panel on this board, if it still has one.
fn pending_panel<'a>(board: &'a Board, pending: &PendingBrowserCreate) -> Option<&'a horizon_core::Panel> {
    board
        .panel_id_by_local_id(&pending.panel_local_id)
        .and_then(|panel_id| board.panel(panel_id))
}

/// The create deadline as an `Instant`, derived from the request's wall-clock
/// deadline relative to now.
fn create_deadline(pending: &PendingBrowserCreate, now: Instant) -> Instant {
    let remaining = pending.request.deadline_at_millis - manifest::now_millis();
    now + Duration::from_millis(u64::try_from(remaining).unwrap_or(0))
}

fn browser_create_is_terminal(board: &Board, io: &mut HostIo, pending: &PendingBrowserCreate) -> bool {
    let Some(browser) = pending_panel(board, pending).and_then(|panel| panel.browser()) else {
        record_and_complete_failure(
            io,
            pending,
            "panel_closed",
            "the requested browser panel closed before it became controllable",
        );
        return true;
    };
    if let Some((code, message)) = terminal_create_failure(browser) {
        record_and_complete_failure(io, pending, code, message);
        return true;
    }
    if pending.request.deadline_at_millis < manifest::now_millis() {
        record_and_complete_failure(io, pending, CREATE_TIMEOUT.0, CREATE_TIMEOUT.1);
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::test_app;
    use horizon_core::{PanelKind, PanelOptions};

    impl HorizonApp {
        /// Register a create as still pending, for tests of paths that must
        /// refuse to touch a panel while its create has not returned.
        pub(in crate::app) fn mark_browser_create_pending_for_tests(&mut self, probe: PendingBrowserCreateProbe) {
            self.browser_create_host.pending.push(PendingBrowserCreate {
                request: BrowserCreateRequest::for_tests(&probe.panel_local_id),
                panel_local_id: probe.panel_local_id,
                backend: BackendKind::default(),
                startup_orientation: None,
                started_at: Instant::now(),
                ready_since: None,
                user_navigations_at_start: 0,
                stage: CreateStage::Starting,
                board: self.browser_create_host.io.board_fence(),
            });
        }
    }

    fn page<'a>(status: &'a BrowserStatus, committed: Option<&'a str>, manifest: &'a str) -> PageReadiness<'a> {
        PageReadiness {
            status,
            committed_url: committed,
            manifest_url: manifest,
            navigation_error: None,
        }
    }

    #[test]
    fn create_readiness_waits_for_the_backend_and_the_committed_first_page() {
        let now = Instant::now();
        let deadline = now + Duration::from_mins(1);
        let ready = BrowserStatus::Ready;
        let example = Some("https://example.test/");
        assert_eq!(
            create_readiness(
                page(&BrowserStatus::Starting, None, ""),
                None,
                false,
                None,
                deadline,
                now
            ),
            CreateReadiness::Waiting,
            "a manifest that exists before the backend is ready does not complete the create"
        );
        assert_eq!(
            create_readiness(page(&ready, None, ""), None, false, Some(now), deadline, now),
            CreateReadiness::Ready(CreateNavigation::NotRequested)
        );
        assert_eq!(
            create_readiness(page(&ready, None, ""), example, false, Some(now), deadline, now),
            CreateReadiness::Waiting,
            "a requested page that has not committed keeps the create pending"
        );
        assert_eq!(
            create_readiness(page(&ready, example, ""), example, false, Some(now), deadline, now),
            CreateReadiness::Waiting,
            "the manifest the agent reads must carry the committed URL too"
        );
        assert_eq!(
            create_readiness(
                page(&ready, Some("https://example.test/final"), "https://example.test/"),
                example,
                false,
                Some(now),
                deadline,
                now
            ),
            CreateReadiness::Waiting,
            "a manifest still naming the pre-redirect document is not committed yet"
        );
        assert_eq!(
            create_readiness(
                page(&ready, example, "https://example.test/"),
                example,
                false,
                Some(now),
                deadline,
                now
            ),
            CreateReadiness::Ready(CreateNavigation::Committed)
        );
    }

    #[test]
    fn create_readiness_reports_blank_failed_and_superseded_first_pages() {
        let now = Instant::now();
        let deadline = now + Duration::from_mins(1);
        let ready = BrowserStatus::Ready;
        let example = Some("https://example.test/");
        assert_eq!(
            create_readiness(
                page(&ready, None, ""),
                Some("about:blank"),
                false,
                Some(now),
                deadline,
                now
            ),
            CreateReadiness::Ready(CreateNavigation::Committed),
            "an explicit blank page is already the requested destination"
        );
        let failed = PageReadiness {
            navigation_error: Some("could not navigate to https://down.test/"),
            ..page(&ready, None, "")
        };
        assert_eq!(
            create_readiness(failed, Some("https://down.test/"), false, Some(now), deadline, now),
            CreateReadiness::Ready(CreateNavigation::Failed),
            "a failed first navigation is reported at once instead of after the startup wait"
        );
        assert_eq!(
            create_readiness(
                page(&ready, Some("https://user.test/"), "https://user.test/"),
                example,
                true,
                Some(now),
                deadline,
                now
            ),
            CreateReadiness::Ready(CreateNavigation::Superseded),
            "a user navigation during startup is not the requested commit"
        );
        let late = now + STARTUP_NAVIGATION_BUDGET;
        assert_eq!(
            create_readiness(
                page(&ready, None, ""),
                Some("https://slow.test/"),
                false,
                Some(now),
                deadline,
                late
            ),
            CreateReadiness::Ready(CreateNavigation::Pending),
            "after the bounded startup wait the panel is reported with a pending navigation"
        );
        let near_deadline = now + Duration::from_secs(3);
        assert_eq!(
            create_readiness(
                page(&ready, None, ""),
                Some("https://slow.test/"),
                false,
                Some(now),
                near_deadline,
                now + Duration::from_millis(2_300)
            ),
            CreateReadiness::Ready(CreateNavigation::Pending),
            "the startup wait never runs into the create deadline"
        );
        assert_eq!(
            create_readiness(
                page(&ready, None, ""),
                Some("https://slow.test/"),
                false,
                Some(now),
                near_deadline,
                now + Duration::from_secs(2)
            ),
            CreateReadiness::Waiting
        );
    }

    #[test]
    fn a_create_left_to_its_work_by_a_session_switch_is_not_one_of_the_restored_panel() {
        let (_temp, mut app) = test_app();
        let workspace = app.board.create_workspace("alpha");
        let editor = PanelOptions {
            kind: PanelKind::Editor,
            ..PanelOptions::default()
        };
        let panel = app.board.create_panel(editor, workspace).expect("panel");
        let local_id = app.board.panel(panel).expect("panel").local_id.clone();
        let saved = horizon_core::RuntimeState::from_board(
            &app.board,
            horizon_core::WindowConfig::default(),
            horizon_core::CanvasViewState::default(),
        );
        let target = app
            .session_store
            .create_session_from_runtime(saved)
            .expect("target session");
        app.mark_browser_create_pending_for_tests(PendingBrowserCreateProbe {
            panel_local_id: local_id.clone(),
        });
        app.browser_create_host.pending[0].stage = CreateStage::Auditing;
        assert!(app.browser_create_is_pending(panel));
        // Its audit is still queued when the switch ends its bounded wait.
        let (release, released) = std::sync::mpsc::channel::<()>();
        app.browser_create_host.io.write(move || {
            let _ = released.recv();
        });
        app.activate_runtime_session(&egui::Context::default(), &target);
        assert!(app.pending_session_switch.is_none(), "the switch completed");
        assert_eq!(app.browser_create_host.pending.len(), 1, "its work still ends it");
        let restored = app.board.panel_id_by_local_id(&local_id).expect("the restored panel");
        assert!(!app.browser_create_is_pending(restored));
        release.send(()).expect("the worker waits");
        app.settle_browser_host_io();
    }

    #[test]
    fn retiring_refuses_the_creates_that_start_and_leaves_the_others_to_their_work() {
        let (_temp, mut app) = test_app();
        for id in ["starting", "auditing"] {
            app.mark_browser_create_pending_for_tests(PendingBrowserCreateProbe {
                panel_local_id: id.into(),
            });
        }
        app.browser_create_host.pending[1].stage = CreateStage::Auditing;
        // The worker stays held, so the refusal is queued but never written to
        // the coordination root of the machine that runs the test.
        let (release, released) = std::sync::mpsc::channel::<()>();
        app.browser_create_host.io.write(move || {
            let _ = released.recv();
        });
        app.retire_starting_browser_creates();
        let left: Vec<_> = app
            .browser_create_host
            .pending
            .iter()
            .map(|pending| pending.panel_local_id.as_str())
            .collect();
        assert_eq!(left, ["auditing"], "its dispatch audit ends it");
        std::mem::forget(release);
        std::mem::forget(app);
    }
}
