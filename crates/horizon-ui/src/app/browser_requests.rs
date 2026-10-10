//! Host-side handling for agent-requested browser panel lifecycle changes.

use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::{Duration, Instant};

use horizon_core::browser::manifest::{
    self, AgentIdentity, BrowserCreateAuditStatus, BrowserCreateRequest, BrowserCreateResult, CreateNavigation,
    HostStampOutcome, ManifestWorkspace,
};
use horizon_core::browser::{BackendAvailability, BackendKind, BrowserStatus};
use horizon_core::{Board, PanelId, PanelKind, PanelOptions, WorkspaceId, browser_actor};

use super::HorizonApp;
use super::browser_host_io::HostIo;
use super::browser_recovery::retired_allocations;
use super::browser_request_claims::HOST_RETIRING;

const CREATE_REQUEST_POLL_INTERVAL: Duration = Duration::from_millis(500);
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

#[derive(Default)]
pub(super) struct BrowserCreateHostState {
    last_request_poll: Option<Instant>,
    pub(super) catalog: super::browser_provider_catalog::CatalogHostState,
    pub(super) provider_usage: super::browser_provider_usage::UsageHostState,
    /// Claimed `cloud_offers` requests waiting for current prices.
    pub(super) cloud_offers: Vec<manifest::provider_usage::UsageRequest>,
    pub(super) recovery_requests: Vec<manifest::recovery::RecoveryRequest>,
    pending: Vec<PendingBrowserCreate>,
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
    /// Panels whose visibility request runs on the coordination worker, with
    /// the visibility it sets; no stamp writes their manifests until it ends.
    pub(super) visibility_in_flight: Vec<(String, bool)>,
    /// Does the coordination file work in order, off the UI thread.
    pub(super) io: HostIo,
}

struct PendingBrowserCreate {
    request: BrowserCreateRequest,
    /// The panel, by its persisted id: board ids restart with a new session.
    panel_local_id: String,
    backend: BackendKind,
    /// Resolved once from the launch plan; later configuration or device state
    /// must not change the orientation recorded for this creation.
    startup_orientation: Option<horizon_core::browser::remote::RemoteOrientation>,
    /// When the host claimed the request, for the reported startup latency.
    started_at: Instant,
    /// When the backend first reported `Ready`, which starts the bounded
    /// startup-navigation wait.
    ready_since: Option<Instant>,
    /// User navigations the panel had seen when the create started; more
    /// means the user took the panel over before the first page committed.
    user_navigations_at_start: u32,
    stage: CreateStage,
}

/// Where a pending create stands. Only a starting create is judged each
/// poll; the others wait for their coordination work, which ends them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CreateStage {
    /// Its dispatch is being audited; an unaudited create never completes.
    Auditing,
    /// The browser starts.
    Starting,
    /// Its ownership, audit and result are being published.
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

#[derive(Clone, Copy)]
pub(super) struct ActorPanel {
    pub(super) panel_id: PanelId,
    pub(super) workspace_id: WorkspaceId,
}

enum BrowserCreateCompletion {
    Waiting,
    Completed,
    Failed,
    /// Its publication runs on the coordination worker, which ends it.
    Finishing,
}

/// One browser panel's host-owned state as the board currently has it.
struct BrowserPlacement {
    local_id: String,
    visible: bool,
    workspace: ManifestWorkspace,
}

/// Outcome of stamping every browser manifest: whether any file changed and
/// whether every manifest this host owns is now current. An incomplete sync
/// keeps the placement fingerprint uncommitted so the next tick retries.
#[derive(Clone, Copy)]
struct HostStateSync {
    changed: bool,
    complete: bool,
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
    pub(super) fn poll_browser_create_requests(&mut self) -> bool {
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
            changed |= self.poll_host_requests();
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

    /// Stamp every owned manifest for the current placement on the
    /// coordination worker, and remember the placement only if all of them
    /// are current. A failed write is retried on the next tick, or at once
    /// when the placement changes again. A placement is queued once: a slow disk
    /// never queues a stamp per frame or per tick, while a panel moved and
    /// then closed still has the stamp of its move queued before it closes.
    /// A manifest whose visibility request runs is left to that request, and
    /// stamped once it ends. Returns whether a stamp was queued.
    ///
    /// Until the stamp lands, a moved panel's remote allocation already
    /// expects its new workspace, which refuses recovery through the old one.
    fn stamp_current_placement(&mut self) -> bool {
        // The authoritative placement is set before any file is touched, also
        // while an earlier stamp still runs.
        let placement = placement_fingerprint(&self.board);
        let mut placements = browser_placements(&self.board);
        let allocations: Vec<_> = placements
            .iter()
            .filter_map(|placement| self.stamped_allocation(&placement.local_id, &placement.workspace.local_id))
            .collect();
        let host = &self.browser_create_host;
        if host.queued_placement == Some(placement) {
            return false;
        }
        let before = placements.len();
        placements.retain(|placement| {
            !host
                .visibility_in_flight
                .iter()
                .any(|(local_id, _)| *local_id == placement.local_id)
        });
        let skipped = placements.len() < before;
        let root = self.host_manifest_root().to_path_buf();
        let requested = host.stamp_requests;
        self.browser_create_host.stamps_in_flight += 1;
        self.browser_create_host.queued_placement = Some(placement);
        self.browser_create_host.io.then(
            move || {
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
                let asked_again = host.stamp_requests != requested;
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
    fn host_manifest_root(&self) -> &Path {
        self.session_store.home().root()
    }

    fn poll_host_requests(&mut self) -> bool {
        self.claim_host_requests();
        self.poll_browser_close_requests()
            | self.poll_remote_recovery()
            | self.poll_provider_usage()
            | self.poll_provider_catalog()
    }

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
        if let Some(panel_id) = self.board.panel_id_by_local_id(&pending.panel_local_id) {
            self.close_panel(panel_id);
        }
        record_and_complete_failure(&mut self.browser_create_host.io, &pending, code, message);
    }

    /// Whether an agent create for this panel has not completed yet.
    pub(super) fn browser_create_is_pending(&self, panel_id: PanelId) -> bool {
        let Some(panel) = self.board.panel(panel_id) else {
            return false;
        };
        self.browser_create_host
            .pending
            .iter()
            .any(|pending| pending.panel_local_id == panel.local_id)
    }

    /// Register a create as still pending, for tests of paths that must
    /// refuse to touch a panel while its create has not returned.
    #[cfg(test)]
    pub(super) fn mark_browser_create_pending_for_tests(&mut self, probe: PendingBrowserCreateProbe) {
        self.browser_create_host.pending.push(PendingBrowserCreate {
            request: BrowserCreateRequest::for_tests(&probe.panel_local_id),
            panel_local_id: probe.panel_local_id,
            backend: BackendKind::default(),
            startup_orientation: None,
            started_at: Instant::now(),
            ready_since: None,
            user_navigations_at_start: 0,
            stage: CreateStage::Starting,
        });
    }

    fn finish_pending_browser_creates(&mut self) -> bool {
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
        if let Some(panel_id) = self.board.panel_id_by_local_id(&pending.panel_local_id) {
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
/// do not make the sync incomplete; every I/O failure does (including a
/// permission failure on the lock or the write), so the caller retries on
/// the next frame.
fn sync_manifest_host_state(
    root: &Path,
    placements: &[BrowserPlacement],
    mut confirm_scope: impl FnMut(&str, bool),
) -> HostStateSync {
    let mut sync = HostStateSync {
        changed: false,
        complete: true,
    };
    for placement in placements {
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
pub(super) fn launched_by_this_host(host_instance: Option<&str>) -> bool {
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
        io.write(move || {
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
    let (panel_local_id, request) = (pending.panel_local_id.clone(), pending.request.clone());
    let (backend, orientation) = (pending.backend, pending.startup_orientation);
    io.then(
        move || publish_requested_create(&panel_local_id, &request, &workspace, backend, orientation, &result),
        {
            let request_id = pending.request.request_id.clone();
            move |app, published| app.finish_published_create(&request_id, published)
        },
    );
    BrowserCreateCompletion::Finishing
}

/// Stamps the new panel and assigns it to the requesting agent in one locked
/// transaction, so no other same-workspace agent can claim it in between,
/// then audits the completion and publishes `ready`, on the coordination
/// worker. A step that fails publishes the failure instead. Says whether the
/// create was published as ready.
fn publish_requested_create(
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
        record_and_complete_failure(
            io,
            pending,
            "create_timeout",
            "the browser panel did not become controllable before the create deadline",
        );
        return true;
    }
    false
}

/// The typed, fixed-text failure a create reports for a panel that will
/// not become controllable. A remote lifecycle names its own terminal
/// outcome (the provider refused, the allocation is unknown, or the device
/// did not meet the target); provider-reported detail never reaches the
/// result.
fn terminal_create_failure(browser: &horizon_core::browser::BrowserPanelState) -> Option<(&'static str, &'static str)> {
    match &browser.status {
        BrowserStatus::Starting | BrowserStatus::Ready => None,
        BrowserStatus::Error { .. } | BrowserStatus::Stopped { .. } => Some(browser.remote_failure().map_or_else(
            || match &browser.status {
                BrowserStatus::Error { .. } => (
                    "backend_start_failed",
                    "the selected browser backend did not start; inspect the local logs",
                ),
                _ => (
                    "backend_stopped",
                    "the selected browser backend stopped before it became controllable",
                ),
            },
            |failure| (failure.code, failure.message),
        )),
    }
}

/// Audits and publishes a failed create on the coordination worker.
fn record_and_complete_failure(io: &mut HostIo, pending: &PendingBrowserCreate, code: &str, message: &str) {
    let result = BrowserCreateResult::failed(&pending.request, code, message);
    let (panel_local_id, request) = (pending.panel_local_id.clone(), pending.request.clone());
    let (backend, orientation) = (pending.backend, pending.startup_orientation);
    io.write(move || {
        record_create_failure(&panel_local_id, &request, backend, orientation);
        complete_result(&result);
    });
}

fn record_create_failure(
    panel_local_id: &str,
    request: &BrowserCreateRequest,
    backend: BackendKind,
    orientation: Option<horizon_core::browser::remote::RemoteOrientation>,
) {
    if let Err(error) = manifest::record_create_status(
        panel_local_id,
        request,
        backend,
        orientation,
        BrowserCreateAuditStatus::Failed,
    ) {
        tracing::warn!(request_id = %request.request_id, %error, "could not append failed browser creation audit");
    }
}

/// Publishes a refused create on the coordination worker.
pub(super) fn fail_create(io: &mut HostIo, request: &BrowserCreateRequest, code: &str, message: &str) {
    let result = BrowserCreateResult::failed(request, code, message);
    io.write(move || complete_result(&result));
}

fn complete_result(result: &BrowserCreateResult) {
    if let Err(error) = manifest::complete_create_request(result) {
        tracing::error!(request_id = %result.request_id, %error, "could not publish browser create result");
    }
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

fn refuse_expired_create(io: &mut HostIo, request: &BrowserCreateRequest) -> bool {
    if request.deadline_at_millis >= manifest::now_millis() {
        return false;
    }
    fail_create(
        io,
        request,
        "request_expired",
        "browser create request expired before Horizon could accept it",
    );
    true
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

    fn page<'a>(status: &'a BrowserStatus, committed: Option<&'a str>, manifest: &'a str) -> PageReadiness<'a> {
        PageReadiness {
            status,
            committed_url: committed,
            manifest_url: manifest,
            navigation_error: None,
        }
    }

    #[test]
    fn remote_lifecycle_failures_report_their_own_typed_codes_with_fixed_text() {
        use horizon_core::browser::{BrowserPanelState, RemoteReleaseOutcome, RemoteSessionEvent};

        let mut rejected = BrowserPanelState::inert_remote("ios_phone", "grid");
        rejected.apply_remote_session_event_for_tests(RemoteSessionEvent::DeviceRejected {
            label: "ios_phone".into(),
            reason: "device model is Emulator 3000, target requires iPhone 16".into(),
            released: RemoteReleaseOutcome::Released,
        });
        rejected.status = BrowserStatus::Error {
            message: "remote session released at once".into(),
        };
        let (code, message) = terminal_create_failure(&rejected).expect("terminal");
        assert_eq!(code, "remote_device_rejected");
        assert!(
            !message.contains("Emulator"),
            "provider text never reaches the result: {message}"
        );

        let mut refused = BrowserPanelState::inert_remote("ios_phone", "grid");
        refused.apply_remote_session_event_for_tests(RemoteSessionEvent::AllocationFailed {
            label: "ios_phone".into(),
            reason: "session not created: no device available".into(),
            refusal: horizon_core::browser::AllocationRefusal::DeviceUnavailable,
        });
        refused.status = BrowserStatus::Stopped { code: None };
        let (code, message) = terminal_create_failure(&refused).expect("terminal");
        assert_eq!(code, "remote_device_unavailable");
        assert!(!message.contains("no device available"));

        let mut unknown = BrowserPanelState::inert_remote("ios_phone", "grid");
        unknown.apply_remote_session_event_for_tests(RemoteSessionEvent::AllocationUnknown {
            label: "ios_phone".into(),
            reason: "timed out".into(),
        });
        unknown.status = BrowserStatus::Error {
            message: "unknown".into(),
        };
        assert_eq!(
            terminal_create_failure(&unknown).map(|(code, _)| code),
            Some("remote_allocation_unknown")
        );

        let mut local = BrowserPanelState::inert();
        local.status = BrowserStatus::Error {
            message: "chrome died".into(),
        };
        assert_eq!(
            terminal_create_failure(&local).map(|(code, _)| code),
            Some("backend_start_failed")
        );
        local.status = BrowserStatus::Stopped { code: Some(1) };
        assert_eq!(
            terminal_create_failure(&local).map(|(code, _)| code),
            Some("backend_stopped")
        );
        local.status = BrowserStatus::Ready;
        assert_eq!(terminal_create_failure(&local), None);
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
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
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
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
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
            visible: true,
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
            visible: true,
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
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
    fn a_placement_change_restamps_before_the_next_poll_tick() {
        let (_temp, mut app) = test_app();
        let alpha = app.board.create_workspace("alpha");
        let beta = app.board.create_workspace("beta");
        let agent_id = app.board.create_panel(agent_options(), alpha).expect("agent panel");

        app.poll_browser_create_requests();
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
        app.poll_browser_create_requests();
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
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
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
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
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
    #[cfg_attr(windows, ignore = "agent panels launch through a POSIX login shell (#688)")]
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
