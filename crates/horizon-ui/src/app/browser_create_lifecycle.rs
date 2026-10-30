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

/// How old the manifest snapshot of a starting create may be before the host
/// asks the coordination worker for a fresh one. The read retries for up to
/// two seconds on Windows, so no frame reads it; the snapshot lags by at most
/// this interval, and the startup budget above and the deadline headroom leave
/// room for that lag before the create deadline.
const CREATE_MANIFEST_REFRESH: Duration = Duration::from_millis(250);

pub(super) struct PendingBrowserCreate {
    pub(super) request: BrowserCreateRequest,
    /// When the host claimed the request, for the reported startup latency.
    pub(super) started_at: Instant,
    pub(super) stage: CreateStage,
    /// The board it was asked on; a create of a replaced board is not one of a restored panel.
    pub(super) board: BoardFence,
    /// The panel the create opened: `None` while a remote create still waits
    /// for its credential values or its provider slot, so no panel exists.
    pub(super) panel: Option<CreatedPanel>,
    /// The OS-store credential values a remote create still waits for; `None`
    /// once they are at hand, and for a create that reads none.
    pub(super) remote_credentials: Option<super::browser_remote_credentials::RemoteCredentialRead>,
}

/// The panel a create opened, and what the host judges it by.
pub(super) struct CreatedPanel {
    /// The panel, by its persisted id: board ids restart with a new session.
    pub(super) local_id: String,
    pub(super) backend: BackendKind,
    /// Resolved once from the launch plan; later configuration or device state
    /// must not change the orientation recorded for this creation.
    pub(super) startup_orientation: Option<horizon_core::browser::remote::RemoteOrientation>,
    /// When the backend first reported `Ready`, which starts the bounded
    /// startup-navigation wait.
    pub(super) ready_since: Option<Instant>,
    /// User navigations the panel had seen when the create started; more
    /// means the user took the panel over before the first page committed.
    pub(super) user_navigations_at_start: u32,
    /// The manifest the agent reads, as the coordination worker last read it:
    /// `None` until the first read lands, and while the file is missing.
    pub(super) manifest: Option<manifest::BrowserManifest>,
    /// When the host last asked the worker for that read.
    pub(super) manifest_read_at: Option<Instant>,
    /// A read of that manifest runs on the coordination worker.
    pub(super) manifest_reading: bool,
}

impl PendingBrowserCreate {
    /// A remote create as it waits for the values only the OS store holds, and
    /// then for its provider slot: no panel exists yet.
    pub(super) fn resolving(request: BrowserCreateRequest, started_at: Instant, board: BoardFence) -> Self {
        Self {
            request,
            started_at,
            stage: CreateStage::ReadingCredentials,
            board,
            panel: None,
            remote_credentials: None,
        }
    }
}

/// Where a pending create stands. Only a starting create is judged each
/// poll; the others wait for their coordination work, which ends them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CreateStage {
    /// A remote create's OS-store credential values are read on the workbench
    /// thread; a later frame plans the request once the last value is at hand.
    ReadingCredentials,
    /// A remote create's provider slot lease runs on the coordination worker;
    /// the panel opens when the lease applies.
    Leasing,
    /// Its dispatch is being audited; an unaudited create never completes.
    Auditing,
    /// The browser starts.
    Starting,
    /// Its ownership, audit and result are being published. A panel that
    /// closes or leaves the agent's workspace during those few milliseconds
    /// is still reported ready; the next stamp writes the board's placement.
    Finishing,
}

/// What the coordination worker read of a starting create's manifest. A read
/// that did not run, because its job panicked, reports no manifest just as a
/// missing file does.
struct StartingCreateManifest(Option<manifest::BrowserManifest>);

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
        self.browser_create_host.pending.iter().any(|pending| {
            pending
                .panel
                .as_ref()
                .is_some_and(|created| created.local_id == panel.local_id)
                && pending.board.holds()
        })
    }

    pub(super) fn finish_pending_browser_creates(&mut self) -> bool {
        if self.browser_create_host.pending.is_empty() {
            return false;
        }
        let mut changed = false;
        let mut waiting = Vec::new();
        for mut pending in std::mem::take(&mut self.browser_create_host.pending) {
            match pending.stage {
                CreateStage::ReadingCredentials | CreateStage::Leasing => {
                    match self.advance_remote_create(pending) {
                        Some(pending) => waiting.push(pending),
                        None => changed = true,
                    }
                    continue;
                }
                CreateStage::Auditing | CreateStage::Finishing => {
                    waiting.push(pending);
                    continue;
                }
                CreateStage::Starting => {}
            }
            let io = &mut self.browser_create_host.io;
            if browser_create_is_terminal(&self.board, io, &pending) {
                changed = true;
                continue;
            }
            self.refresh_starting_create_manifest(&mut pending);
            let io = &mut self.browser_create_host.io;
            let Some(panel) = pending.panel.as_mut() else {
                waiting.push(pending);
                continue;
            };
            match finish_ready_browser_create(&self.board, io, &pending.request, panel, pending.started_at) {
                BrowserCreateCompletion::Waiting => waiting.push(pending),
                BrowserCreateCompletion::Completed => changed = true,
                BrowserCreateCompletion::Failed => {
                    let local_id = pending.panel.as_ref().map(|panel| panel.local_id.as_str());
                    if let Some(panel_id) = local_id.and_then(|local_id| self.board.panel_id_by_local_id(local_id)) {
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

    /// Refuses every create that still waits on this side of its coordination
    /// work, which nothing polls once Horizon exits or switches sessions: a
    /// create that starts its browser, and a remote create whose credential
    /// values have not arrived. The teardown closes the panel of a starting
    /// create; a create whose coordination work already runs ends with that
    /// work, which refuses it once the board is retiring.
    pub(super) fn retire_starting_browser_creates(&mut self) {
        let host = &mut self.browser_create_host;
        let (refused, waiting): (Vec<_>, _) = std::mem::take(&mut host.pending)
            .into_iter()
            .partition(|pending| matches!(pending.stage, CreateStage::Starting | CreateStage::ReadingCredentials));
        host.pending = waiting;
        for pending in &refused {
            record_and_complete_failure(
                &mut host.io,
                &pending.request,
                pending.panel.as_ref(),
                HOST_RETIRING.0,
                HOST_RETIRING.1,
            );
        }
    }

    /// Asks the coordination worker for the manifest of a starting create
    /// when the snapshot is older than [`CREATE_MANIFEST_REFRESH`] and no read
    /// of it runs. The frame judges the create on the last snapshot instead of
    /// reading the file, so a slow disk or a lock another process holds never
    /// stalls a frame.
    fn refresh_starting_create_manifest(&mut self, pending: &mut PendingBrowserCreate) {
        let Some(panel) = pending.panel.as_mut() else { return };
        let now = Instant::now();
        if !manifest_refresh_due(panel.manifest_read_at, panel.manifest_reading, now) {
            return;
        }
        panel.manifest_reading = true;
        panel.manifest_read_at = Some(now);
        let request_id = pending.request.request_id.clone();
        let path = manifest::manifest_path_for_root(self.host_manifest_root(), &panel.local_id);
        self.browser_create_host.io.then(
            move || StartingCreateManifest(manifest::read_at(&path)),
            move |app, read| app.apply_starting_create_manifest(&request_id, read),
        );
    }

    /// Keeps a read of a starting create's manifest for the create that asked
    /// for it. A read whose board was replaced, or whose panel left the board,
    /// is dropped: the create it would have answered has ended.
    fn apply_starting_create_manifest(&mut self, request_id: &str, read: Option<StartingCreateManifest>) {
        let stale = self.browser_create_host.io.applying_stale();
        let Some(index) = self
            .browser_create_host
            .pending
            .iter()
            .position(|pending| pending.request.request_id == request_id)
        else {
            return;
        };
        let on_board = self.browser_create_host.pending[index]
            .panel
            .as_ref()
            .is_some_and(|panel| self.board.panel_id_by_local_id(&panel.local_id).is_some());
        let Some(panel) = self.browser_create_host.pending[index].panel.as_mut() else {
            return;
        };
        panel.manifest_reading = false;
        if stale || !on_board {
            return;
        }
        panel.manifest = read.and_then(|read| read.0);
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
            && let Some(panel) = pending.panel.as_ref()
            && let Some(panel_id) = self.board.panel_id_by_local_id(&panel.local_id)
        {
            self.close_panel(panel_id);
        }
        if published.is_none() {
            record_and_complete_failure(
                &mut self.browser_create_host.io,
                &pending.request,
                pending.panel.as_ref(),
                "manifest_update_failed",
                "Horizon could not publish the new browser panel's visibility and workspace",
            );
        }
    }
}

fn finish_ready_browser_create(
    board: &Board,
    io: &mut HostIo,
    request: &BrowserCreateRequest,
    panel: &mut CreatedPanel,
    started_at: Instant,
) -> BrowserCreateCompletion {
    let Some(browser) = board
        .panel_id_by_local_id(&panel.local_id)
        .and_then(|panel_id| board.panel(panel_id))
        .and_then(|panel| panel.browser())
    else {
        return BrowserCreateCompletion::Waiting;
    };
    let now = Instant::now();
    if browser.status == BrowserStatus::Ready {
        panel.ready_since.get_or_insert(now);
    }
    // The driver writes its manifest before the backend is ready and before
    // the initial navigation commits, so the file's existence alone must not
    // complete the create. The host judges it on the snapshot that the
    // coordination worker last read, so a missing manifest keeps the create
    // waiting here, as the absent file did.
    let Some(current) = panel.manifest.as_ref() else {
        return BrowserCreateCompletion::Waiting;
    };
    let deadline = create_deadline(request.deadline_at_millis, now);
    let CreateReadiness::Ready(navigation) = create_readiness(
        PageReadiness {
            status: &browser.status,
            committed_url: browser.url.as_deref(),
            manifest_url: &current.url,
            navigation_error: browser.navigation_error.as_deref(),
        },
        request.url.as_deref(),
        browser.user_navigation_count() > panel.user_navigations_at_start,
        panel.ready_since,
        deadline,
        now,
    ) else {
        return BrowserCreateCompletion::Waiting;
    };
    let navigation_error = (navigation == CreateNavigation::Failed)
        .then(|| browser.navigation_error.clone())
        .flatten();
    let startup_millis = u64::try_from(now.saturating_duration_since(started_at).as_millis()).unwrap_or(u64::MAX);
    let Some(workspace) = board
        .panel_id_by_local_id(&panel.local_id)
        .and_then(|panel_id| board.panel(panel_id))
        .and_then(|panel| browser_workspace(board, panel.workspace_id))
    else {
        record_and_complete_failure(
            io,
            request,
            Some(panel),
            "workspace_unavailable",
            "Horizon could not determine the new browser panel's workspace",
        );
        return BrowserCreateCompletion::Failed;
    };
    if !workspace.authorizes(host_identity(&request.actor)) {
        // The user moved a panel while the browser started. Keep the panel
        // where they put it and report the lost workspace instead of closing
        // it or handing an uncontrollable panel back as ready.
        let (panel_local_id, visible) = (panel.local_id.clone(), request.visible);
        let request_id = request.request_id.clone();
        io.write_for_board(move || {
            if let Err(error) = publish_manifest_host_state(&panel_local_id, visible, &workspace) {
                tracing::warn!(%request_id, %error, "could not stamp a moved browser panel");
            }
        });
        record_and_complete_failure(
            io,
            request,
            Some(panel),
            "workspace_changed",
            "the browser panel left the requesting agent's workspace during creation and was left in place",
        );
        return BrowserCreateCompletion::Completed;
    }
    let result = BrowserCreateResult::ready(
        request,
        panel.local_id.clone(),
        navigation,
        navigation_error,
        startup_millis,
    );
    let (panel_local_id, live) = (panel.local_id.clone(), io.live_fence());
    let (backend, orientation) = (panel.backend, panel.startup_orientation);
    let published_request = request.clone();
    io.then(
        move || {
            publish_requested_create(
                &live,
                &panel_local_id,
                &published_request,
                &workspace,
                backend,
                orientation,
                &result,
            )
        },
        {
            let request_id = request.request_id.clone();
            move |app, published| app.finish_published_create(&request_id, published)
        },
    );
    BrowserCreateCompletion::Finishing
}

/// The pending create's panel on this board, if it still has one.
fn pending_panel<'a>(board: &'a Board, pending: &PendingBrowserCreate) -> Option<&'a horizon_core::Panel> {
    let panel = pending.panel.as_ref()?;
    board
        .panel_id_by_local_id(&panel.local_id)
        .and_then(|panel_id| board.panel(panel_id))
}

/// Whether a starting create asks for a fresh manifest snapshot: never while a
/// read of it runs, and not before [`CREATE_MANIFEST_REFRESH`] passed since the
/// last ask. The snapshot lags by that interval at most, and the startup budget
/// leaves room for it before the create deadline.
fn manifest_refresh_due(read_at: Option<Instant>, reading: bool, now: Instant) -> bool {
    !reading && read_at.is_none_or(|read_at| now.saturating_duration_since(read_at) >= CREATE_MANIFEST_REFRESH)
}

/// The create deadline as an `Instant`, derived from the request's wall-clock
/// deadline relative to now.
fn create_deadline(deadline_at_millis: i64, now: Instant) -> Instant {
    let remaining = deadline_at_millis - manifest::now_millis();
    now + Duration::from_millis(u64::try_from(remaining).unwrap_or(0))
}

fn browser_create_is_terminal(board: &Board, io: &mut HostIo, pending: &PendingBrowserCreate) -> bool {
    let Some(browser) = pending_panel(board, pending).and_then(|panel| panel.browser()) else {
        record_and_complete_failure(
            io,
            &pending.request,
            pending.panel.as_ref(),
            "panel_closed",
            "the requested browser panel closed before it became controllable",
        );
        return true;
    };
    if let Some((code, message)) = terminal_create_failure(browser) {
        record_and_complete_failure(io, &pending.request, pending.panel.as_ref(), code, message);
        return true;
    }
    if pending.request.deadline_at_millis < manifest::now_millis() {
        record_and_complete_failure(
            io,
            &pending.request,
            pending.panel.as_ref(),
            CREATE_TIMEOUT.0,
            CREATE_TIMEOUT.1,
        );
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
                started_at: Instant::now(),
                stage: CreateStage::Starting,
                board: self.browser_create_host.io.board_fence(),
                panel: Some(CreatedPanel {
                    local_id: probe.panel_local_id,
                    backend: BackendKind::default(),
                    startup_orientation: None,
                    ready_since: None,
                    user_navigations_at_start: 0,
                    manifest: None,
                    manifest_read_at: None,
                    manifest_reading: false,
                }),
                remote_credentials: None,
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
            .map(|pending| pending.panel.as_ref().map_or("", |panel| panel.local_id.as_str()))
            .collect();
        assert_eq!(left, ["auditing"], "its dispatch audit ends it");
        std::mem::forget(release);
        std::mem::forget(app);
    }

    /// A browser panel that has not finished starting, placed in `workspace`.
    fn starting_browser(app: &mut HorizonApp, id: u64, workspace: horizon_core::WorkspaceId) -> String {
        use horizon_core::browser::BrowserPanelState;
        use horizon_core::{Panel, PanelContent, PanelId};
        let mut state = BrowserPanelState::inert();
        state.status = BrowserStatus::Starting;
        let panel = Panel::from_content(
            PanelId(id),
            workspace,
            PanelKind::Browser,
            PanelContent::Browser(Box::new(state)),
        );
        let local_id = panel.local_id.clone();
        app.board.panels.push(panel);
        app.board.assign_panel_to_workspace(PanelId(id), workspace);
        local_id
    }

    /// The manifest the agent reads for `local_id`, in this host's own home.
    fn written_manifest(app: &HorizonApp, local_id: &str, url: &str) {
        let path = manifest::manifest_path_for_root(app.host_manifest_root(), local_id);
        manifest::write_at(
            &path,
            &manifest::BrowserManifest {
                url: url.into(),
                ..manifest::BrowserManifest::default()
            },
        )
        .expect("live manifest");
    }

    /// What one read that a starting create started left on it, once every job
    /// the frame asked for applied. `meanwhile` runs while the read waits, as a
    /// create that ends, a panel that closes or a board that is replaced would.
    fn snapshot_after_one_read(meanwhile: impl FnOnce(&mut HorizonApp, &str)) -> Option<String> {
        let (_temp, mut app) = test_app();
        let workspace = app.board.create_workspace("alpha");
        let local_id = starting_browser(&mut app, 9_170, workspace);
        app.mark_browser_create_pending_for_tests(PendingBrowserCreateProbe {
            panel_local_id: local_id.clone(),
        });
        written_manifest(&app, &local_id, "https://example.test/");
        let (release, released) = std::sync::mpsc::channel::<()>();
        app.browser_create_host.io.write(move || {
            let _ = released.recv();
        });
        app.finish_pending_browser_creates();
        meanwhile(&mut app, &local_id);
        release.send(()).expect("release the worker");
        app.settle_browser_host_io();
        app.browser_create_host
            .pending
            .iter()
            .find(|pending| pending.panel.as_ref().is_some_and(|panel| panel.local_id == local_id))
            .and_then(|pending| pending.panel.as_ref())
            .and_then(|panel| panel.manifest.as_ref())
            .map(|manifest| manifest.url.clone())
    }

    #[test]
    fn a_blocked_manifest_read_never_stalls_the_frame_that_judges_a_starting_create() {
        let (_temp, mut app) = test_app();
        let workspace = app.board.create_workspace("alpha");
        let local_id = starting_browser(&mut app, 9_150, workspace);
        app.mark_browser_create_pending_for_tests(PendingBrowserCreateProbe {
            panel_local_id: local_id.clone(),
        });
        written_manifest(&app, &local_id, "https://example.test/");
        // The worker waits, as it does for a manifest read whose file another
        // process holds for the read's two-second bound.
        let (release, released) = std::sync::mpsc::channel::<()>();
        app.browser_create_host.io.write(move || {
            let _ = released.recv();
        });

        let started = Instant::now();
        assert!(!app.finish_pending_browser_creates(), "the read is not done yet");
        let elapsed = started.elapsed();
        release.send(()).expect("release the worker");
        assert!(
            elapsed < Duration::from_millis(1500),
            "the frame waited {elapsed:?} for the manifest read"
        );
        assert!(
            app.browser_create_host.io.busy(),
            "the read waits on the coordination worker"
        );
        assert!(
            app.browser_create_host.pending[0]
                .panel
                .as_ref()
                .is_some_and(|panel| panel.manifest.is_none()),
            "the first frame judges the create on no snapshot yet"
        );

        app.settle_browser_host_io();
        let pending = app.browser_create_host.pending.first().expect("the create still waits");
        assert_eq!(
            pending
                .panel
                .as_ref()
                .and_then(|panel| panel.manifest.as_ref())
                .map(|manifest| manifest.url.as_str()),
            Some("https://example.test/"),
            "the read applies to the create that asked for it"
        );
        assert!(
            pending.panel.as_ref().is_some_and(|panel| !panel.manifest_reading),
            "no read of it is in flight"
        );
    }

    #[test]
    fn a_manifest_read_is_dropped_when_its_create_ended_its_panel_left_or_its_board_was_replaced() {
        assert_eq!(
            snapshot_after_one_read(|_, _| {}).as_deref(),
            Some("https://example.test/"),
            "a read that still has its create, its panel and its board applies"
        );
        assert_eq!(
            snapshot_after_one_read(|app, _| app.browser_create_host.pending.clear()),
            None,
            "the read of a create that ended is dropped"
        );
        assert_eq!(
            snapshot_after_one_read(|app, local_id| {
                let panel_id = app.board.panel_id_by_local_id(local_id).expect("panel");
                app.board.panels.retain(|panel| panel.id != panel_id);
            }),
            None,
            "the read of a panel that left the board is dropped"
        );
        assert_eq!(
            snapshot_after_one_read(|app, _| app.browser_create_host.io.replace_board()),
            None,
            "a read asked for on a replaced board is dropped"
        );
    }

    #[test]
    fn a_starting_create_asks_for_its_manifest_at_most_once_per_interval() {
        let now = Instant::now();
        assert!(manifest_refresh_due(None, false, now), "the first frame asks");
        assert!(
            !manifest_refresh_due(Some(now), false, now),
            "a frame inside the interval judges the snapshot it holds"
        );
        assert!(
            !manifest_refresh_due(Some(now), false, now + CREATE_MANIFEST_REFRESH / 2),
            "halfway through the interval is still inside it"
        );
        assert!(
            !manifest_refresh_due(Some(now), true, now + CREATE_MANIFEST_REFRESH * 10),
            "a read that is still running is never asked for twice"
        );
        assert!(
            manifest_refresh_due(Some(now), false, now + CREATE_MANIFEST_REFRESH),
            "the frame at the interval asks again"
        );
    }

    /// The URL of the manifest snapshot the create holds, as the frame judges it.
    fn snapshot_url(app: &HorizonApp, local_id: &str) -> Option<String> {
        app.browser_create_host
            .pending
            .iter()
            .find(|pending| pending.panel.as_ref().is_some_and(|panel| panel.local_id == local_id))
            .and_then(|pending| pending.panel.as_ref())
            .and_then(|panel| panel.manifest.as_ref())
            .map(|manifest| manifest.url.clone())
    }

    #[test]
    fn a_starting_create_reads_its_manifest_again_only_after_the_interval() {
        let (_temp, mut app) = test_app();
        let workspace = app.board.create_workspace("alpha");
        let local_id = starting_browser(&mut app, 9_180, workspace);
        app.mark_browser_create_pending_for_tests(PendingBrowserCreateProbe {
            panel_local_id: local_id.clone(),
        });
        written_manifest(&app, &local_id, "https://example.test/");

        app.finish_pending_browser_creates();
        app.settle_browser_host_io();
        assert_eq!(
            snapshot_url(&app, &local_id).as_deref(),
            Some("https://example.test/"),
            "the first frame asks the worker for the manifest"
        );

        // Frames inside the interval judge the snapshot they hold, whatever the
        // file says by then; the assertions stop when the interval is close.
        written_manifest(&app, &local_id, "https://later.test/");
        while last_manifest_read(&app).is_some_and(|read_at| read_at.elapsed() < CREATE_MANIFEST_REFRESH / 2) {
            app.finish_pending_browser_creates();
            assert_eq!(
                snapshot_url(&app, &local_id).as_deref(),
                Some("https://example.test/"),
                "a frame inside the interval reads nothing, so a slow disk cannot mark the frame"
            );
        }

        // The frame after the interval asks again, and holds the newer file.
        if let Some(read_at) = last_manifest_read(&app) {
            let wait = (read_at + CREATE_MANIFEST_REFRESH).saturating_duration_since(Instant::now());
            std::thread::sleep(wait + Duration::from_millis(20));
        }
        app.finish_pending_browser_creates();
        app.settle_browser_host_io();
        assert_eq!(
            snapshot_url(&app, &local_id).as_deref(),
            Some("https://later.test/"),
            "the frame after the interval asks again"
        );
    }

    /// When the frame last asked the worker for this create's manifest.
    fn last_manifest_read(app: &HorizonApp) -> Option<Instant> {
        app.browser_create_host
            .pending
            .first()
            .and_then(|pending| pending.panel.as_ref())
            .and_then(|panel| panel.manifest_read_at)
    }
}
