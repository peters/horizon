//! Host-side handling for agent-requested browser panel lifecycle changes.

use std::path::Path;
use std::time::{Duration, Instant};

use horizon_core::browser::manifest;
use horizon_core::{Board, PanelId, WorkspaceId, browser_actor};

use super::HorizonApp;
use super::browser_create_lifecycle::PendingBrowserCreate;
use super::browser_host_io::HostIo;
use super::browser_manifest_stamp::{
    HostStateSync, browser_placements, placement_fingerprint, sync_manifest_host_state,
};
use super::browser_recovery::retired_allocations;

const CREATE_REQUEST_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Default)]
pub(super) struct BrowserCreateHostState {
    last_request_poll: Option<Instant>,
    pub(super) catalog: super::browser_provider_catalog::CatalogHostState,
    pub(super) provider_usage: super::browser_provider_usage::UsageHostState,
    /// Claimed `cloud_offers` requests waiting for current prices.
    pub(super) cloud_offers: Vec<manifest::provider_usage::UsageRequest>,
    pub(super) recovery_requests: Vec<manifest::recovery::RecoveryRequest>,
    /// Answers to recovery requests that the coordination worker could not
    /// write yet, for a later frame and until each request's deadline.
    pub(super) recovery_answers: super::answer_retries::RefusedAnswers<manifest::recovery::RecoveryResult>,
    pub(super) pending: Vec<PendingBrowserCreate>,
    /// Closes the host has applied but whose session teardown has not
    /// completed yet; each is published once its teardown signal settles.
    pub(super) pending_closes: Vec<super::browser_close_requests::PendingBrowserClose>,
    /// Exact allocation ownership, recovery handles, and cross-instance leases.
    pub(super) remote_allocations: horizon_core::browser::remote_recovery::RemoteAllocations,
    /// Remote sessions whose release was never established by a board this
    /// host has since replaced: each keeps counting against its provider's
    /// `max_sessions` and keeps its slot leased until exact release is established.
    pub(super) orphaned_remote_holds: Vec<horizon_core::OrphanedRemoteHold>,
    /// Board placement the manifests were last stamped for; a change
    /// re-stamps on the same frame instead of waiting for the next tick.
    stamped_placement: Option<u64>,
    /// Stamps queued on the coordination worker, and the placement of the
    /// latest of them.
    stamps_in_flight: usize,
    queued_placement: Option<u64>,
    /// Counts the ticks that asked for a new stamp, so a stamp that was
    /// running then does not count as the one they asked for.
    stamp_requests: u64,
    /// A placement whose stamp did not complete. Frames do not stamp it again
    /// until the next tick asks, so a manifest that cannot be written is not
    /// retried at the frame rate.
    stamp_failed: Option<u64>,
    /// A claim of the agents' requests runs on the coordination worker.
    pub(super) claiming: bool,
    /// A claim of the agents' recovery requests runs on the coordination worker.
    pub(super) recovery_claiming: bool,
    /// The read of the remote panels' manifest owners runs on the coordination
    /// worker, so one slow read is not queued again on every tick and close.
    pub(super) scope_reading: bool,
    /// Panels whose visibility request runs on the coordination worker, with
    /// the visibility it sets; until it ends, a stamp keeps their manifest's.
    pub(super) visibility_in_flight: Vec<super::browser_visibility_requests::VisibilityInFlight>,
    /// Does the coordination file work in order, off the UI thread.
    pub(super) io: HostIo,
}

#[derive(Clone, Copy)]
pub(super) struct ActorPanel {
    pub(super) panel_id: PanelId,
    pub(super) workspace_id: WorkspaceId,
}

impl BrowserCreateHostState {
    /// Asks for a fresh stamp of every manifest; a stamp that runs now does
    /// not count as that one.
    pub(super) fn forget_stamped_placement(&mut self) {
        self.stamped_placement = None;
        self.stamp_failed = None;
        self.stamp_requests += 1;
    }
}

impl HorizonApp {
    pub(super) fn poll_browser_create_requests(&mut self, ctx: &egui::Context) -> bool {
        let mut changed = self.apply_browser_host_io();
        changed |= self.finish_pending_browser_creates();
        let now = Instant::now();
        let poll_due = self
            .browser_create_host
            .last_request_poll
            .is_none_or(|last| now.saturating_duration_since(last) >= CREATE_REQUEST_POLL_INTERVAL);
        // Every stamp happens once per frame, after rendering, in
        // `restamp_browser_manifests_for_placement`; the tick only requests
        // the cadence-based one by forgetting the last stamped placement.
        if poll_due {
            self.browser_create_host.last_request_poll = Some(now);
            changed |= self.close_ended_browser_panels();
            changed |= self.poll_host_requests(ctx);
            self.browser_create_host.forget_stamped_placement();
        }
        changed
    }

    /// Re-stamp the manifests as soon as the board placement they depend on
    /// changes. This runs at the end of every frame, after queued workspace
    /// changes and the moves made while rendering, so moving or hiding a
    /// panel starts revoking the old workspace's access on that frame rather
    /// than on a later tick.
    pub(super) fn restamp_browser_manifests_for_placement(&mut self) -> bool {
        let placement = Some(placement_fingerprint(&self.board));
        let host = &self.browser_create_host;
        if host.stamped_placement == placement || host.stamp_failed == placement {
            return false;
        }
        self.stamp_current_placement()
    }

    /// Stamp every owned manifest for the current placement on the coordination worker, and remember the
    /// placement only if all of them are current. A failed write is retried on the next tick, or at once when
    /// the placement changes again. A placement is queued once: a slow disk never queues a stamp per frame or
    /// per tick, while a panel moved and then closed still has the stamp of its move queued before it closes.
    /// A panel whose visibility request runs keeps the visibility its manifest has (host writes of a manifest
    /// all run on the worker, in order), and is stamped again once the request ends. Returns whether a stamp
    /// was queued. Until the stamp lands, a moved panel's remote allocation already expects its new
    /// workspace, which refuses recovery through the old one.
    fn stamp_current_placement(&mut self) -> bool {
        // A queued placement already set what its allocations expect.
        let placement = placement_fingerprint(&self.board);
        if self.browser_create_host.queued_placement == Some(placement) {
            return false;
        }
        // The authoritative placement is set before any file is touched, also
        // while an earlier stamp still runs.
        let mut placements = browser_placements(&self.board);
        let allocations: Vec<_> = placements
            .iter()
            .filter_map(|placement| self.stamped_allocation(&placement.local_id, &placement.workspace.local_id))
            .collect();
        let host = &self.browser_create_host;
        let mut skipped = false;
        for placement in &mut placements {
            if host
                .visibility_in_flight
                .iter()
                .any(|running| running.local_id == placement.local_id)
            {
                (placement.visible, skipped) = (None, true);
            }
        }
        let root = self.host_manifest_root().to_path_buf();
        let (requested, board) = (host.stamp_requests, host.io.board_fence());
        self.browser_create_host.stamps_in_flight += 1;
        self.browser_create_host.queued_placement = Some(placement);
        self.browser_create_host.io.then(
            move || {
                // The placement of a replaced board is not written over the one that stands.
                if !board.holds() {
                    return (HostStateSync::default(), Vec::new());
                }
                let sync = sync_manifest_host_state(&root, &placements, |panel, confirmed| {
                    for stamped in allocations.iter().filter(|stamped| stamped.local_id == panel) {
                        stamped.allocation.confirm_scope(confirmed);
                    }
                });
                if sync.changed {
                    tracing::debug!("stamped browser manifests for a new board placement");
                }
                (sync, retired_allocations(&root, allocations))
            },
            move |app, outcome| {
                let (sync, retired) = outcome.map_or((None, Vec::new()), |(sync, retired)| (Some(sync), retired));
                app.keep_stamped_scopes(&retired);
                let host = &mut app.browser_create_host;
                host.stamps_in_flight -= 1;
                if host.stamps_in_flight == 0 {
                    host.queued_placement = None;
                }
                // A stamp of a replaced board says nothing about the board that stands.
                let asked_again = host.stamp_requests != requested || host.io.applying_stale();
                let current = sync.is_some_and(|sync| sync.complete) && !skipped;
                host.stamped_placement = (current && !asked_again).then_some(placement);
                host.stamp_failed = (!current && !asked_again).then_some(placement);
                // A placement that changed while this stamp ran is stamped now,
                // also when no later frame comes, as during an exit.
                if placement_fingerprint(&app.board) != placement {
                    app.stamp_current_placement();
                }
            },
        );
        true
    }

    /// Root of the Horizon home whose manifests this host stamps. Production
    /// constructs the session store from `HorizonHome::resolve()`, the same
    /// root the drivers and the MCP server use for their default paths.
    pub(super) fn host_manifest_root(&self) -> &Path {
        self.session_store.home().root()
    }

    fn poll_host_requests(&mut self, ctx: &egui::Context) -> bool {
        self.claim_host_requests();
        self.poll_browser_close_requests()
            | self.poll_remote_recovery()
            | self.poll_provider_usage(ctx)
            | self.poll_provider_catalog()
    }
}

pub(super) fn actor_panel(board: &Board, actor: &str) -> Option<ActorPanel> {
    board
        .panels
        .iter()
        .find(|panel| panel.kind.is_agent() && browser_actor(&panel.local_id) == actor)
        .map(|panel| ActorPanel {
            panel_id: panel.id,
            workspace_id: panel.workspace_id,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::test_app;
    use horizon_core::browser::manifest::ManifestWorkspace;
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
    fn a_placement_change_restamps_before_the_next_poll_tick() {
        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        let beta = app.board.create_workspace("beta");
        let agent_id = app.board.create_panel(agent_options(), alpha).expect("agent panel");

        app.poll_browser_create_requests(&egui::Context::default());
        assert!(
            app.browser_create_host.last_request_poll.is_some(),
            "the first tick polls"
        );
        assert!(
            app.browser_create_host.stamped_placement.is_none(),
            "the tick only requests a stamp; the end-of-frame check performs it"
        );
        // The tick's recovery poll started a stamp; the tick asked for a later one.
        app.settle_browser_host_io();
        assert!(app.browser_create_host.stamped_placement.is_none());
        assert!(
            app.restamp_browser_manifests_for_placement(),
            "the end of the frame starts a stamp"
        );
        app.settle_browser_host_io();
        let stamped = app.browser_create_host.stamped_placement.expect("the stamp lands");

        // The last tick is now, however long the stamps took, so the next one
        // is not due during the checks below.
        let first_poll = Instant::now();
        app.browser_create_host.last_request_poll = Some(first_poll);
        app.poll_browser_create_requests(&egui::Context::default());
        assert_eq!(
            app.browser_create_host.last_request_poll,
            Some(first_poll),
            "an unchanged board waits for the poll interval"
        );
        assert_eq!(app.browser_create_host.stamped_placement, Some(stamped));

        app.board.assign_panel_to_workspace(agent_id, beta);
        assert!(
            app.restamp_browser_manifests_for_placement(),
            "a placement change starts a stamp on the same frame"
        );
        app.settle_browser_host_io();
        assert_eq!(
            app.browser_create_host.last_request_poll,
            Some(first_poll),
            "the end-of-frame re-stamp does not advance the request poll cadence"
        );
        let after_move = app.browser_create_host.stamped_placement;
        assert_ne!(after_move, Some(stamped), "the moved placement is the one stamped");

        let first_poll = Instant::now();
        app.browser_create_host.last_request_poll = Some(first_poll);
        app.poll_browser_create_requests(&egui::Context::default());
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
    fn a_manifest_lock_held_elsewhere_never_stalls_the_frame_that_stamps() {
        use horizon_core::browser::BrowserPanelState;
        use horizon_core::{Panel, PanelContent};

        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        app.board.create_panel(agent_options(), alpha).expect("agent panel");
        let browser = Panel::from_content(
            PanelId(900),
            alpha,
            PanelKind::Browser,
            PanelContent::Browser(Box::new(BrowserPanelState::inert())),
        );
        let path = manifest::manifest_path_for_root(app.host_manifest_root(), &browser.local_id);
        app.board.panels.push(browser);
        app.board.assign_panel_to_workspace(PanelId(900), alpha);
        manifest::write_at(
            &path,
            &manifest::BrowserManifest {
                host: Some(manifest::host_instance().to_string()),
                ..manifest::BrowserManifest::default()
            },
        )
        .expect("live manifest");
        // Another process holds the manifest's lock, as a driver whose write
        // waits for a slow disk would.
        let lock = std::fs::File::create(path.with_extension("json.lock")).expect("lock file");
        lock.lock().expect("the test holds the lock");

        let started = Instant::now();
        assert!(app.restamp_browser_manifests_for_placement());
        // The worker waits up to the lock's two-second bound; the frame only queues.
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "the frame waited {:?} for the lock",
            started.elapsed()
        );
        assert_eq!(
            app.browser_create_host.stamps_in_flight, 1,
            "the stamp waits on the worker"
        );
        assert!(app.browser_create_host.stamped_placement.is_none());

        std::thread::sleep(Duration::from_millis(100));
        assert!(
            manifest::read_at(&path).expect("manifest").workspace.is_none(),
            "nothing is written while the lock is held"
        );
        drop(lock);
        app.settle_browser_host_io();
        // A stamp that gave up waiting for the lock is queued again by the
        // next tick; one that landed stays as it is.
        app.browser_create_host.forget_stamped_placement();
        app.restamp_browser_manifests_for_placement();
        app.settle_browser_host_io();
        assert!(
            manifest::read_at(&path).expect("manifest").workspace.is_some(),
            "the stamp lands once the lock is free"
        );
    }

    #[test]
    fn a_stamp_that_cannot_be_written_waits_for_the_next_tick() {
        use horizon_core::browser::BrowserPanelState;
        use horizon_core::{Panel, PanelContent};

        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        app.board.create_panel(agent_options(), alpha).expect("agent panel");
        let browser = Panel::from_content(
            PanelId(900),
            alpha,
            PanelKind::Browser,
            PanelContent::Browser(Box::new(BrowserPanelState::inert())),
        );
        let path = manifest::manifest_path_for_root(app.host_manifest_root(), &browser.local_id);
        app.board.panels.push(browser);
        app.board.assign_panel_to_workspace(PanelId(900), alpha);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("directory");
        std::fs::write(&path, "invalid manifest").expect("unreadable manifest fixture");

        assert!(app.restamp_browser_manifests_for_placement());
        app.settle_browser_host_io();
        assert!(app.browser_create_host.stamped_placement.is_none());
        assert!(
            !app.restamp_browser_manifests_for_placement(),
            "a frame does not stamp the placement that failed again"
        );
        app.browser_create_host.forget_stamped_placement();
        assert!(
            app.restamp_browser_manifests_for_placement(),
            "the next tick tries again"
        );
        app.settle_browser_host_io();
    }

    #[test]
    fn a_stamp_asked_for_on_a_replaced_board_writes_nothing() {
        use horizon_core::browser::BrowserPanelState;
        use horizon_core::{Panel, PanelContent};

        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        app.board.create_panel(agent_options(), alpha).expect("agent panel");
        let browser = Panel::from_content(
            PanelId(900),
            alpha,
            PanelKind::Browser,
            PanelContent::Browser(Box::new(BrowserPanelState::inert())),
        );
        let path = manifest::manifest_path_for_root(app.host_manifest_root(), &browser.local_id);
        app.board.panels.push(browser);
        app.board.assign_panel_to_workspace(PanelId(900), alpha);
        let live = manifest::BrowserManifest {
            host: Some(manifest::host_instance().to_string()),
            ..manifest::BrowserManifest::default()
        };
        manifest::write_at(&path, &live).expect("live manifest");
        let (release, released) = std::sync::mpsc::channel::<()>();
        app.browser_create_host.io.write(move || {
            let _ = released.recv();
        });
        assert!(app.restamp_browser_manifests_for_placement());
        // A session switch replaces the board before the stamp runs.
        app.browser_create_host.io.replace_board();
        release.send(()).expect("the worker waits");
        app.settle_browser_host_io();
        assert!(
            manifest::read_at(&path).expect("manifest").workspace.is_none(),
            "the old board's placement is not written"
        );
        assert!(app.browser_create_host.stamped_placement.is_none());
    }

    #[test]
    fn a_placement_write_queued_before_a_session_switch_keeps_the_restored_panels_manifest() {
        let (_temp, mut app) = test_app();
        let root = app.host_manifest_root().to_path_buf();
        let path = manifest::manifest_path_for_root(&root, "browser-1");
        let target = app
            .session_store
            .create_session_from_runtime(horizon_core::RuntimeState::default())
            .expect("target session");
        // The worker is still busy when the switch ends its wait.
        let (release, released) = std::sync::mpsc::channel::<()>();
        app.browser_create_host.io.write(move || {
            let _ = released.recv();
        });
        let old = ManifestWorkspace::new(manifest::host_instance(), "old-workspace", Vec::new());
        let stamped = root.clone();
        app.browser_create_host.io.write_for_board(move || {
            let _ = manifest::sync_host_state_in(&stamped, "browser-1", Some(false), &old);
        });
        app.activate_runtime_session(&egui::Context::default(), &target);
        assert!(app.pending_session_switch.is_none(), "the switch completed");
        // The restored board's panel with the same persisted id has its own manifest.
        let restored = manifest::BrowserManifest {
            host: Some(manifest::host_instance().to_string()),
            ..manifest::BrowserManifest::default()
        };
        manifest::write_at(&path, &restored).expect("restored manifest");
        let before = std::fs::read(&path).expect("manifest");
        release.send(()).expect("the worker waits");
        app.settle_browser_host_io();
        assert_eq!(
            std::fs::read(&path).expect("manifest"),
            before,
            "the old board's placement is not written"
        );
    }

    #[test]
    fn only_the_exact_horizon_actor_matches_a_panel() {
        assert!(actor_panel(&Board::new(), "horizon:missing").is_none());
        assert!(actor_panel(&Board::new(), "external").is_none());
    }
}
