//! UI actions and progress for real deployments. Provider/build/session work lives in core.
mod capabilities;
pub(super) mod cards;
mod close;
mod companions;
mod creation;
mod creation_job;
#[cfg(all(test, unix))]
mod creation_tests;
mod first_panel;
mod idle;
mod launch;
mod lifecycle;
mod local_network;
#[cfg(debug_assertions)]
mod log_preview;
mod machine_size;
mod offer_publication;
mod offers;
mod park;
mod preparation;
mod presentation;
mod prices;
mod progress;
mod readiness;
mod rebuild;
mod repository_setup;
mod resize;
mod sessions;
mod setup;
#[cfg(debug_assertions)]
mod stopped_preview;
use super::HorizonApp;
use horizon_core::cloud_panel::CloudConfig;
use horizon_core::{
    PanelKind, PanelOptions,
    cloud_panel::{CloudGroup, CloudLaunch},
    cloud_runtime::{
        self, Event, Stage,
        deployment::{self, Request},
        settings::Settings,
        ssh::Connection,
        state::{Deployment, Session, Store},
    },
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::{Receiver, channel},
};

const DELETED_RESOURCES_MESSAGE: &str = "Worker deleted; managed workspace storage cleanup is complete. Any separately attached network volumes retain their files and credentials and remain billable until deleted.";
/// A live run cost shows elapsed seconds, so it advances once per second.
const RUN_COST_REFRESH: std::time::Duration = std::time::Duration::from_secs(1);
/// UI tests resolve the developer's real Horizon home, so they must never reach the provider with its credential.
#[cfg(not(test))]
const BILLING: cloud_runtime::billing::Fetch = cloud_runtime::billing::fetch;
#[cfg(test)]
const BILLING: cloud_runtime::billing::Fetch = |_, _, _, _| Err(cloud_runtime::billing::BillingError::Unavailable);

#[derive(Default)]
pub(super) struct Production {
    close: close::State,
    pub(in crate::app) tailnets: crate::app::tailnets::State,
    tailnet: Option<String>,
    pub(super) setup: setup::State,
    pub creating: bool,
    launch: launch::State,
    pending_creation: Option<creation_job::Pending>,
    creation_busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(super) focus_title_on_open: bool,
    title: String,
    repository: String,
    choosing_repository: bool,
    /// Where the new cloud's code comes from: a link to clone, or a folder.
    source: creation::source::State,
    /// What Start depends on, confirmed before it is offered.
    checks: creation::checks::State,
    revision: String,
    profiles: Option<CloudConfig>,
    selected_profile: String,
    /// A CPU worker size chosen for the selected profile; `None` keeps the profile's size.
    size: Option<machine_size::Size>,
    /// Where the new cloud may be placed; any allowed data center by default.
    placement: horizon_core::cloud_panel::Placement,
    /// The provider chosen for the selected profile; `None` keeps the profile's own.
    provider: Option<&'static horizon_core::cloud_runtime::provider::Description>,
    /// Provider prices and stock shown while choosing the size.
    prices: prices::State,
    setup_agent: Option<PanelKind>,
    setup_agents: Vec<horizon_core::cloud_runtime::setup::Agent>,
    session_id: Option<String>,
    pub runtimes: HashMap<u32, Runtime>,
    companions: companions::State,
    /// Prices sent to ready workers for their agents' cloud offers.
    offer_publication: offer_publication::State,
}
#[derive(Default, PartialEq, Eq)]
pub(super) enum Confirmation {
    #[default]
    None,
    Stop,
    Delete,
    Redeploy,
    Rebuild,
    CancelRebuild,
}
/// One line of a cloud operation's output, with the step that printed it and when.
pub(super) struct LogLine {
    pub text: String,
    pub stage: Option<Stage>,
    /// Time into the attempt; `None` for lines outside one, such as idle reports.
    pub at: Option<std::time::Duration>,
    /// Classified once on arrival, so drawing the log does not re-scan each line.
    pub kind: LineKind,
    /// The operation that printed it (`progress::Timeline::attempt`).
    pub attempt: u64,
    /// Which run of its step within the attempt: a step revisited later (Validate
    /// after Build) is a second visit with its own heading and time.
    pub visit: usize,
    /// A note outside any operation (`Runtime::push_note`): shown, never diagnosed.
    pub note: bool,
    /// [`text_key`] of `text`, so a long deploy log can reuse row heights.
    text_key: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LineKind {
    Plain,
    Warning,
    Failure,
}

impl LogLine {
    pub(super) fn new(text: String, stage: Option<Stage>, at: Option<std::time::Duration>) -> Self {
        let kind = if cloud_runtime::diagnosis::is_failure(&text) {
            LineKind::Failure
        } else if text
            .trim_start()
            .get(..7)
            .is_some_and(|start| start.eq_ignore_ascii_case("warning"))
        {
            LineKind::Warning
        } else {
            LineKind::Plain
        };
        let text_key = text_key(&text);
        Self {
            text,
            stage,
            at,
            kind,
            attempt: 0,
            visit: 0,
            note: false,
            text_key,
        }
    }
}

/// FNV-1a of a log line. The output view reuses a wrapped row's height by this key
/// instead of hashing the text again on every frame.
fn text_key(text: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Whether `previous` is an earlier update of `line`'s layer in the same step of the same
/// attempt; a retry pushing the same image keeps the earlier attempt's lines.
fn same_layer(previous: &LogLine, line: &LogLine, layer: &str) -> bool {
    // A failure is never merged: it keeps its place in arrival order, where diagnosis
    // reads newest first, and a later progress update does not overwrite it.
    previous.kind != LineKind::Failure
        && line.kind != LineKind::Failure
        && previous.attempt == line.attempt
        && previous.stage == line.stage
        && previous.visit == line.visit
        && layer_id(&previous.text) == Some(layer)
}

/// The layer a Docker progress line such as `5f70bf18a086: Pushing [==>  ]` reports on.
fn layer_id(line: &str) -> Option<&str> {
    let (id, rest) = line.split_once(": ")?;
    (id.len() == 12 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) && !rest.starts_with("digest")).then_some(id)
}

#[derive(Default)]
pub(super) struct Runtime {
    drawer: Option<cards::Tab>,
    receiver: Option<Receiver<Event>>,
    recovery_receiver: Option<Receiver<cloud_runtime::Result<cloud_runtime::lifecycle::ReconciledDeployment>>>,
    recovery_worker_id: String,
    remote_release: Option<Receiver<cloud_runtime::Result<Deployment>>>,
    remote_release_error: Option<String>,
    repaint_context: Option<egui::Context>,
    sender: Option<std::sync::mpsc::Sender<Event>>,
    cancel: Option<horizon_core::cloud_runtime::Cancellation>,
    stage: Option<Stage>,
    progress: progress::Timeline,
    logs: std::collections::VecDeque<LogLine>,
    /// Lines that arrived after the reader scrolled up. They join `logs` when
    /// follow mode resumes, so the visible history does not shift.
    pending_logs: std::collections::VecDeque<LogLine>,
    /// A reader scrolled away from the latest line in an output view last frame.
    verbose_unpinned: bool,
    /// Output views scrolled up this frame, one bit per place; folded into
    /// `verbose_unpinned` at the next frame, so a view no longer shown stops holding lines.
    unpinned_views: u8,
    /// The views that were scrolled up last frame. Only they keep the held-still list;
    /// a view still following shows the newest lines, held ones included.
    unpinned_last: u8,
    /// The worker operation running or last run. A failure reloads the saved record, so
    /// the stage alone may no longer say whether a stop or a resume was tried.
    operation: Option<lifecycle::Action>,
    /// The header asked for a confirmation; Manage scrolls it into view once.
    reveal_confirmation: bool,
    /// The status the header computed this frame, reused by the body and drawer.
    frame_status: Option<(u64, cards::Status)>,
    /// Counts every change to the output, including lines replaced in place.
    log_generation: u64,
    /// The last failure diagnosis and the output it was read from.
    diagnosis: std::cell::RefCell<Option<(cards::DiagnosisKey, cards::Failure)>>,
    state: Option<Deployment>,
    error: Option<String>,
    confirmation: Confirmation,
    state_unavailable: bool,
    needs_attach: bool,
    /// A freshly deployed cloud that has not yet opened its first panel.
    first_panel_due: bool,
    /// The first Ready of this runtime has been looked at; later ones (a resize, a rebuild) are not
    /// a new deployment.
    first_panel_considered: bool,
    pending_browser_attachments: std::collections::HashSet<String>,
    parking: park::Parking,
    pending_member_attachments: std::collections::HashSet<String>,
    /// What the restored member placeholders of this cloud last showed; the board is
    /// searched for them only when this changes.
    member_wait: Option<horizon_core::CloudWait>,
    pending_session_attachments: std::collections::HashSet<String>,
    next_attachment_attempt: Option<std::time::Instant>,
    needs_desktop: bool,
    pub(in crate::app::cloud_panel) desktop_controller: Option<String>,
    pub(in crate::app::cloud_panel) desktop_last_input: Option<String>,
    desktop: Option<std::sync::Arc<cloud_runtime::tunnel::DesktopTunnel>>,
    browsers: Option<Vec<horizon_core::browser::CloudViewState>>,
    billing: cloud_runtime::billing::BillingMonitor,
    rebuild: Option<rebuild::Attempt>,
    resize: resize::State,
    idle_reports: Option<idle::Reports>,
    /// The newest idle record the current operation's watch read.
    last_idle: Option<cloud_runtime::lifecycle::IdleSample>,
    /// That watch asks the provider; see [`Runtime::idle_confirming`].
    idle_confirming: bool,
    /// Why the worker stopped, when it stopped without an operation the owner started.
    stop_cause: Option<cloud_runtime::lifecycle::StopCause>,
    /// A failure of a ready or reconnecting cloud, held while the provider check that
    /// may explain it as a stop runs.
    unexplained_failure: Option<String>,
    /// The failure just reported may be a stop Horizon did not make.
    failure_needs_check: bool,
    sharing: local_network::Sharing,
    /// The owner's scope for that sharing, kept while it pauses.
    scope: local_network::Editor,
    /// The network that sharing started on; a paused bridge resumes only on it.
    shared: Option<horizon_core::cloud_runtime::local_network::Network>,
    /// When sharing last looked at the clocks and at the network, running or not.
    watch: local_network::Watch,
}
impl Runtime {
    /// Lines kept while the view follows the end. Same depth as a shell or agent panel.
    const FOLLOW_LOG_LINES: usize = horizon_core::PANEL_SCROLLBACK_LIMIT;

    fn observe(&mut self, event: &Event) {
        self.observe_rebuild(event);
        if matches!(event, Event::Ready(..)) {
            self.sharing.ready_again();
        }
    }
    /// Lines held aside while the reader is scrolled up. They join the visible
    /// history when follow resumes, which then keeps [`Self::FOLLOW_LOG_LINES`].
    const PENDING_LOG_LINES: usize = horizon_core::PANEL_SCROLLBACK_LIMIT;

    /// While the reader is scrolled up, new lines wait aside so the lines on
    /// screen are neither dropped nor shifted.
    fn push_log(&mut self, text: String) {
        let mut line = LogLine::new(text, self.stage, self.progress.elapsed());
        line.attempt = self.progress.attempt();
        line.visit = line.stage.map_or(0, |stage| self.progress.visit(stage));
        self.append_log(line);
    }

    /// A deployment, reconnect, rebuild or resize that failed before it started: that is
    /// this attempt's failure at its `first` step, with its own output, not an earlier
    /// stop's or resume's, nor the step the cloud reached before.
    fn fail_preflight(&mut self, first: Stage, error: String) {
        if self.connected_ready() {
            // Nothing started: the connected cloud and its watch stay as they are.
            self.error = Some(error);
            return;
        }
        self.progress.reset();
        self.operation = None;
        // A redeploy of a deleted cloud that could not start leaves it deleted, so
        // Redeploy stays its way forward; the record's termination is not cleanup to finish.
        if self.stage != Some(Stage::Deleted) {
            self.stage = Some(first);
        }
        self.error = Some(error);
    }

    /// Ready with its connection watch running: an operation that fails before it starts
    /// leaves this as it is and only reports why.
    fn connected_ready(&self) -> bool {
        self.receiver.is_some() && self.stage == Some(Stage::Ready)
    }

    /// A line outside any operation's steps: an idle report, a provider check or a device
    /// release. It has no step or time, so it is not filed under the last step's heading.
    fn push_note(&mut self, text: String) {
        let mut line = LogLine::new(text, None, None);
        line.attempt = self.progress.attempt();
        line.note = true;
        self.append_log(line);
    }

    fn append_log(&mut self, line: LogLine) {
        self.log_generation += 1;
        // An update replaces its layer's line wherever it is: in place, it moves nothing
        // on a scrolled-up screen, and a following view never shows the layer twice.
        if let Some(layer) = layer_id(&line.text)
            && let Some(previous) = self
                .logs
                .iter_mut()
                .chain(self.pending_logs.iter_mut())
                .rev()
                .find(|previous| same_layer(previous, &line, layer))
        {
            *previous = line;
            return;
        }
        if self.verbose_unpinned {
            self.pending_logs.push_back(line);
            while self.pending_logs.len() > Self::PENDING_LOG_LINES {
                self.pending_logs.pop_front();
            }
            return;
        }
        self.accept_followed_logs();
        self.logs.push_back(line);
        self.trim_followed_logs();
    }

    fn accept_followed_logs(&mut self) {
        if self.pending_logs.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending_logs);
        for line in pending {
            // A layer updated while the reader was scrolled up replaces its visible line.
            if let Some(layer) = layer_id(&line.text)
                && let Some(previous) = self
                    .logs
                    .iter_mut()
                    .rev()
                    .find(|previous| same_layer(previous, &line, layer))
            {
                *previous = line;
            } else {
                self.logs.push_back(line);
            }
        }
        self.trim_followed_logs();
    }

    fn trim_followed_logs(&mut self) {
        while self.logs.len() > Self::FOLLOW_LOG_LINES {
            self.logs.pop_front();
        }
    }

    fn needs_provider_check(state: &Deployment) -> bool {
        state.operation == cloud_runtime::CreateState::Requested
            // The worker may run either image until the provider reports which.
            || state.stage == Stage::Replace
            || state
                .image_replacement
                .as_ref()
                .is_some_and(cloud_runtime::state::ImageReplacement::requested)
            || (matches!(state.operation, cloud_runtime::CreateState::Bound { .. })
                && !state.stop_requested
                && state
                    .worker
                    .as_ref()
                    .is_some_and(|worker| !worker.is_starting_or_running()))
    }

    /// A restored cloud resumes readiness for an active bound worker on its recorded image.
    fn reconnects_on_restore(state: &Deployment) -> bool {
        matches!(state.operation, cloud_runtime::CreateState::Bound { .. })
            && !state.stop_requested
            && !Self::needs_provider_check(state)
    }

    /// `siblings` are pinned before the image is built; a record that pinned them already keeps its own.
    fn start_deployment(
        &mut self,
        request: Request,
        siblings: Vec<cloud_runtime::siblings::Binding>,
        ctx: &egui::Context,
    ) {
        if self.receiver.is_some() && self.stage != Some(Stage::Ready) {
            return;
        }
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        self.desktop = None;
        self.progress.reset();
        // A deployment or reconnect is its own operation; an earlier stop or resume that
        // failed no longer names this attempt's failure.
        self.operation = None;
        self.rebuild = None;
        self.stop_cause = None;
        self.sharing.await_ready();
        let (tx, rx) = channel();
        let cancel = cloud_runtime::Cancellation::default();
        self.cancel = Some(cancel.clone());
        self.receiver = Some(rx);
        self.sender = Some(tx.clone());
        let idle = self.listen_idle();
        self.error = None;
        self.state_unavailable = false;
        let ctx = ctx.clone();
        std::thread::spawn(move || run_deployment_with_siblings(&request, &siblings, &cancel, &tx, idle, &ctx));
    }

    /// Only a Ready cloud shows its current run; other stages report their own progress.
    pub(in crate::app::cloud_panel) fn current_run_cost(
        &self,
        now: std::time::SystemTime,
    ) -> Option<cloud_runtime::cost::RunCost> {
        if self.stage != Some(Stage::Ready) || self.worker_terminated() {
            return None;
        }
        cloud_runtime::cost::current_run(self.state.as_ref()?.worker.as_ref()?, now)
    }

    /// The provider confirmed the worker deleted (storage cleanup may still be pending);
    /// the saved stage and worker snapshot can still read as running. A redeploy keeps
    /// that record until Ready, so while one runs it is history.
    pub(in crate::app::cloud_panel) fn worker_terminated(&self) -> bool {
        (self.receiver.is_none() || self.progress.is_deletion())
            && self
                .state
                .as_ref()
                .is_some_and(|state| matches!(state.operation, cloud_runtime::CreateState::Terminated { .. }))
    }

    fn poll_release_and_repaint(&mut self, ctx: &egui::Context) {
        self.poll_remote_release();
        // The idle watch's newest record, or its own report of the stop, comes before a
        // provider check that classifies the stop with it.
        self.poll_idle();
        self.poll_recovery();
        self.poll_resize();
        if self.needs_repaint() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else if self.current_run_cost(std::time::SystemTime::now()).is_some() {
            ctx.request_repaint_after(RUN_COST_REFRESH);
        } else if let Some(due) = self.billing.next_update_in(std::time::Instant::now()) {
            // Billing only refreshes during a frame, and an idle cloud requests none.
            ctx.request_repaint_after(due);
        }
    }

    /// Sessions, members or browsers are still being attached.
    fn attaching(&self) -> bool {
        self.needs_attach
            || !self.pending_browser_attachments.is_empty()
            || !self.pending_session_attachments.is_empty()
            || !self.pending_member_attachments.is_empty()
    }

    fn needs_repaint(&self) -> bool {
        self.remote_release.is_some()
            || self.resize.busy()
            || self.recovery_receiver.is_some()
            || (self.receiver.is_some() && self.stage != Some(Stage::Ready))
            || self.needs_attach
            || self.first_panel_due
            || self.needs_desktop
            || !self.pending_browser_attachments.is_empty()
            || !self.pending_session_attachments.is_empty()
            || !self.pending_member_attachments.is_empty()
    }
}
impl HorizonApp {
    pub(super) fn prepare_production_clouds(&mut self, ctx: &egui::Context) {
        self.sync_cloud_companion_session(ctx);
        if self.pending_startup_runtime_state.is_some() || self.startup_receiver.is_some() {
            return;
        }
        self.restore_cloud_state(ctx);
        let mut finished = Vec::new();
        let mut removed = Vec::new();
        let mut resumed = Vec::new();
        for (&id, runtime) in &mut self.cloud_prototype.production.runtimes {
            runtime.repaint_context.get_or_insert_with(|| ctx.clone());
            let events: Vec<_> = runtime
                .receiver
                .as_ref()
                .map_or_else(Vec::new, |rx| rx.try_iter().collect());
            for event in events {
                runtime.observe(&event);
                match event {
                    Event::Snapshot(state) => {
                        runtime.stage = Some(state.stage);
                        runtime.state = Some(*state);
                    }
                    Event::Stopped(state) => {
                        runtime.progress.stage(Stage::Stopped, std::time::Instant::now());
                        runtime.stage = Some(Stage::Stopped);
                        runtime.state = Some(*state);
                        runtime.desktop = None;
                        runtime.error = None;
                        runtime.receiver = None;
                        runtime.cancel = None;
                        // A stop can end a rebuild that met it; its steps no longer apply.
                        runtime.rebuild = None;
                        runtime.stop_cause = None;
                    }
                    Event::Resumed => {
                        runtime.receiver = None;
                        resumed.push(id);
                    }
                    Event::ClosedBrowsers(ids) => {
                        if let Some(browsers) = &mut runtime.browsers {
                            browsers.retain(|b| !ids.contains(&b.id));
                        }
                        removed.extend(ids.into_iter().map(|local| (id, local)));
                    }
                    Event::DesktopControl { active, last } => {
                        runtime.desktop_controller = active;
                        runtime.desktop_last_input = last;
                    }
                    Event::Desktop(tunnel) => {
                        runtime.desktop = Some(tunnel);
                        runtime.needs_desktop = true;
                    }
                    Event::Browsers(browsers) => {
                        runtime.browsers = Some(browsers);
                    }
                    Event::Deleted(at) => {
                        runtime.progress.stage(Stage::Deleted, at);
                        runtime.stage = Some(Stage::Deleted);
                        runtime.desktop = None;
                        runtime.error = Some(DELETED_RESOURCES_MESSAGE.into());
                        finished.push(id);
                    }
                    Event::Stage(stage, at) => {
                        runtime.progress.stage(stage, at);
                        runtime.stage = Some(stage);
                    }
                    Event::Progress(progress) => runtime.progress.update(progress),
                    Event::Output(line) => runtime.push_log(line),
                    Event::Ready(state, at) => {
                        runtime.progress.stage(Stage::Ready, at);
                        runtime.browsers = None;
                        runtime.needs_attach = true;
                        runtime.note_ready_for_first_panel(&state);
                        runtime.state = Some(*state);
                        runtime.stage = Some(Stage::Ready);
                        runtime.error = None;
                        runtime.stop_cause = None;
                        finished.push(id);
                    }
                    Event::Failed(error, at) => {
                        runtime.show_failure(error, at);
                        finished.push(id);
                    }
                }
            }
            runtime.poll_release_and_repaint(ctx);
        }
        self.follow_cloud_billing(ctx);
        self.finish_failed_cloud_operations(finished, ctx);
        self.reconcile_sharing(ctx);
        self.finish_closing_clouds(ctx);
        self.reconnect_resumed(resumed, ctx);
        self.remove_closed_cloud_browsers(removed);
        self.sync_resized_profiles();
        self.sync_cloud_members();
        self.start_first_cloud_panels(ctx);
        self.cloud_prototype.groups.reconcile(&mut self.board);
        self.sync_board_cloud_groups();
        self.prepare_cloud_companions(ctx);
        self.publish_cloud_offers(ctx);
        self.retain_cloud_workspaces();
    }

    fn retain_cloud_workspaces(&mut self) {
        for group in &self.cloud_prototype.groups.0 {
            if let Some(ws) = self.board.workspace_id_by_local_id(&group.workspace) {
                self.board.retain_workspace_when_empty(ws);
            }
        }
    }
    /// Keeps each bound worker's billing fresh in the background; other clouds forget theirs.
    fn follow_cloud_billing(&mut self, ctx: &egui::Context) {
        let repaint = {
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        };
        let root = self.cloud_prototype.root.as_deref();
        for runtime in self.cloud_prototype.production.runtimes.values_mut() {
            runtime.billing.follow(runtime.state.as_ref(), root, BILLING, &repaint);
        }
    }
    fn finish_failed_cloud_operations(&mut self, finished: Vec<u32>, ctx: &egui::Context) {
        for id in finished {
            if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id)
                && runtime.error.is_some()
            {
                runtime.receiver = None;
                let check = std::mem::take(&mut runtime.failure_needs_check) && runtime.recovery_receiver.is_none();
                if check {
                    self.check_failure_with_provider(id, ctx);
                } else {
                    runtime.cancel = None;
                }
            }
        }
    }
    fn remove_closed_cloud_browsers(&mut self, removed: Vec<(u32, String)>) {
        for (cloud, local) in removed {
            let member = self
                .cloud_prototype
                .groups
                .0
                .iter()
                .any(|group| group.issue == cloud && group.panels.contains(&local));
            if member
                && let Some(id) = self.board.panel_id_by_local_id(&local)
                && self
                    .board
                    .panel(id)
                    .is_some_and(|panel| panel.kind == PanelKind::Browser)
            {
                self.board.close_panel(id);
                self.panel_render_caches.browser_ui_state.remove(&id);
            }
        }
    }
    pub(super) fn cloud_state_matches_session(&self) -> bool {
        self.cloud_prototype.production.session_id == self.active_session.as_ref().map(|s| s.session_id.clone())
    }

    fn restore_cloud_state(&mut self, ctx: &egui::Context) {
        let session = self.active_session.as_ref().map(|s| s.session_id.clone());
        if !self.cloud_prototype.initialized || self.cloud_prototype.production.session_id != session {
            // Clouds that do not survive this restore never paint again, so their
            // row-height caches would otherwise stay in egui temp data.
            let kept: Vec<u32> = self.board.cloud_groups.0.iter().map(|group| group.issue).collect();
            let mut dropped: Vec<u32> = self
                .cloud_prototype
                .groups
                .0
                .iter()
                .map(|group| group.issue)
                .chain(self.cloud_prototype.production.runtimes.keys().copied())
                .filter(|id| !kept.contains(id))
                .collect();
            dropped.sort_unstable();
            dropped.dedup();
            for id in dropped {
                cards::forget_log_heights(ctx, id);
            }
            self.cloud_prototype.initialized = true;
            self.cloud_prototype.production.pending_creation = None;
            self.cloud_prototype.production.close = close::State::default();
            self.cloud_prototype.production.session_id = session;
            self.cloud_prototype.root = Some(horizon_core::HorizonHome::resolve().root().join("cloud"));
            self.cloud_prototype.groups = self.board.cloud_groups.clone();
            self.cloud_prototype.production.runtimes.clear();
            self.cloud_prototype.ready = true;
            let reconnect: Vec<_> = self
                .cloud_prototype
                .groups
                .0
                .iter()
                .filter_map(|group| {
                    let launch = group.remote.as_ref()?;
                    let result = cloud_runtime::state::cloud_directory(self.cloud_prototype.root.as_ref()?, &launch.id)
                        .and_then(|root| Store::lock(&root))
                        .and_then(|store| store.load());
                    let state = match result {
                        Ok(Some(state)) => state,
                        Ok(None) if !launch.deployment_started => return None,
                        other => {
                            let message = other.err().map_or_else(
                                || "Deployment record is missing; reconcile its worker before continuing".into(),
                                |error| error.to_string(),
                            );
                            self.cloud_prototype.production.runtimes.insert(
                                group.issue,
                                Runtime {
                                    error: Some(message),
                                    state_unavailable: true,
                                    resize: cloud_runtime::state::cloud_directory(
                                        self.cloud_prototype.root.as_ref()?,
                                        &launch.id,
                                    )
                                    .map_or_else(|_| resize::State::default(), |root| resize::restored(&root)),
                                    ..Runtime::default()
                                },
                            );
                            return None;
                        }
                    };
                    self.cloud_prototype.production.runtimes.insert(
                        group.issue,
                        Runtime {
                            stage: Some(state.stage),
                            state: Some(state.clone()),
                            ..Runtime::default()
                        },
                    );
                    Runtime::reconnects_on_restore(&state).then_some(group.issue)
                })
                .collect();
            self.sync_resized_profiles();
            for id in reconnect {
                self.start_production_deployment(id, ctx);
            }
            // A debug build can show a synthetic deploy log. Release builds omit it.
            #[cfg(debug_assertions)]
            log_preview::seed(self, ctx);
            #[cfg(debug_assertions)]
            stopped_preview::seed(self, ctx);
        }
    }
    /// Reconnects each resumed worker. The reconnect that finishes a resume is still that
    /// resume, so a failure there offers Resume worker.
    fn reconnect_resumed(&mut self, resumed: Vec<u32>, ctx: &egui::Context) {
        for id in resumed {
            self.start_production_deployment(id, ctx);
            if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id) {
                runtime.operation = Some(lifecycle::Action::Resume);
            }
        }
    }

    /// Waits for a provider check, which may hold the record a deployment reads first.
    fn start_production_deployment(&mut self, id: u32, ctx: &egui::Context) {
        let runtimes = &self.cloud_prototype.production.runtimes;
        if runtimes.get(&id).is_some_and(Runtime::checking_provider) {
            return;
        }
        if let Some((request, siblings)) = self.prepare_production_deployment(id) {
            self.cloud_prototype
                .production
                .runtimes
                .entry(id)
                .or_default()
                .start_deployment(request, siblings, ctx);
        }
    }

    fn persist_cloud_before_allocation(&mut self, id: u32) -> bool {
        let result = (|| {
            let session = self
                .active_session
                .as_ref()
                .filter(|session| session.persistent)
                .ok_or_else(|| {
                    "Save this workspace in a persistent Horizon session before allocating a worker".to_string()
                })?;
            if !self.auto_save_runtime_state() {
                return Err(
                    "Could not save this workspace before allocating a worker; retry after fixing session storage"
                        .into(),
                );
            }
            self.session_store
                .sync_runtime_state(&session.session_id)
                .map_err(|error| error.to_string())
        })();
        if let Err(error) = result {
            self.cloud_prototype.production.runtimes.entry(id).or_default().error = Some(error.clone());
            self.cloud_prototype.error = Some(error);
            return false;
        }
        true
    }

    pub(in crate::app) fn prepare_cloud_remote_panel(
        &mut self,
        index: usize,
        options: &mut PanelOptions,
    ) -> horizon_core::Result<()> {
        let group = &self.cloud_prototype.groups.0[index];
        let Some(launch) = &group.remote else { return Ok(()) };
        let runtime = self.cloud_prototype.production.runtimes.get(&group.issue);
        let state = runtime
            .and_then(|r| r.state.as_ref())
            .filter(|s| s.stage == Stage::Ready)
            .ok_or_else(|| horizon_core::Error::Config("Deploy or reconnect the cloud before adding panels".into()))?;
        if let Some(reason) = group.unavailable_panel_reason(options.kind) {
            return Err(horizon_core::Error::Config(reason.into()));
        }
        let root = self
            .cloud_prototype
            .root
            .as_ref()
            .ok_or_else(|| horizon_core::Error::Config("No cloud settings".into()))?;
        let result = (|| -> cloud_runtime::Result<()> {
            let settings = Settings::load(&root.join("settings.json"))?;
            let store = Store::lock(&cloud_runtime::state::cloud_directory(root, &launch.id)?)?;
            let mut saved = store
                .load()?
                .ok_or(cloud_runtime::Error::Invalid("Missing cloud deployment"))?;
            let id = options
                .local_id
                .clone()
                .unwrap_or_else(horizon_core::cloud_runtime::new_id);
            let worker = state
                .worker
                .as_ref()
                .ok_or(cloud_runtime::Error::Invalid("Cloud has no worker"))?;
            let connection = Connection::new(worker, &settings, store.root())?;
            if options.kind == PanelKind::Browser {
                let observed =
                    runtime.and_then(|runtime| runtime.browsers.as_ref()?.iter().find(|browser| browser.id == id));
                capabilities::prepare_browser(
                    &launch.profile.capabilities,
                    &state.browserstack_targets,
                    observed,
                    options,
                )?;
                options.cloud_connection = Some(connection);
                options.local_id = Some(id);
                options.cwd = None;
                return Ok(());
            }
            if options.kind == PanelKind::Device {
                let endpoint = runtime
                    .and_then(|r| r.desktop.as_ref())
                    .ok_or(cloud_runtime::Error::Invalid("Desktop tunnel is connecting"))?
                    .endpoint;
                options.command = Some(endpoint.to_string());
                options.local_id = Some(id);
                options.cwd = None;
                return Ok(());
            }
            let agent = match options.kind {
                PanelKind::Codex => "codex",
                PanelKind::Claude => "claude",
                PanelKind::Grok => "grok",
                _ => "shell",
            };
            let session = saved
                .sessions
                .iter()
                .find(|session| session.panel_id == id)
                .cloned()
                .unwrap_or_else(|| Session {
                    panel_id: id.clone(),
                    agent: agent.into(),
                    tmux: id.clone(),
                    branch: String::new(),
                    worktree: cloud_runtime::siblings::shared_worktree(saved.siblings.as_ref()),
                });
            if !saved.sessions.iter().any(|s| s.panel_id == id) {
                saved.sessions.push(session.clone());
                store.save(&saved)?;
            }
            let worker = state
                .worker
                .as_ref()
                .ok_or(cloud_runtime::Error::Invalid("Cloud has no worker"))?;
            options.cloud_connection = Some(connection);
            options.command = Some("ssh".into());
            options.args = Connection::new(worker, &settings, store.root())?.attach_args(&session, &launch.revision)?;
            options.cwd = None;
            options.local_id = Some(id);
            options.session_binding = None;
            Ok(())
        })();
        result.map_err(|e| horizon_core::Error::Config(e.to_string()))
    }
}

fn run_deployment(
    request: &Request,
    cancel: &cloud_runtime::Cancellation,
    tx: &std::sync::mpsc::Sender<Event>,
    idle: std::sync::mpsc::Sender<idle::Report>,
    ctx: &egui::Context,
) {
    run_deployment_with_siblings(request, &[], cancel, tx, idle, ctx);
}

fn run_deployment_with_siblings(
    request: &Request,
    siblings: &[cloud_runtime::siblings::Binding],
    cancel: &cloud_runtime::Cancellation,
    tx: &std::sync::mpsc::Sender<Event>,
    idle: std::sync::mpsc::Sender<idle::Report>,
    ctx: &egui::Context,
) {
    let emit = |event| {
        let _ = tx.send(event);
        ctx.request_repaint();
    };
    let settings = request.settings.clone();
    let root = request.state_root.clone();
    match deployment::deploy_with_siblings(request, siblings, cancel, &emit) {
        Ok(state) => {
            idle::watch(&state, &settings, &root, cancel, idle, ctx);
            presentation::watch(&state, &settings, &root, cancel, tx, ctx);
        }
        Err(error) => {
            report_failure(&root, &error, &emit);
        }
    }
}

/// Reports the saved record with the failure, so the card shows what was kept. A
/// record found busy is read once its lock is released, since that may be an idle
/// stop finishing, which the card then shows instead of the failure. Returns
/// whether the record could be read, after that wait.
fn report_failure(root: &std::path::Path, error: &cloud_runtime::Error, emit: &dyn Fn(Event)) -> bool {
    let load = || Store::lock(root).and_then(|store| store.load());
    let (read, saved) = if matches!(error, cloud_runtime::Error::Busy) {
        match idle::after_busy(load, std::time::Duration::from_secs(1)).map(idle::stopped_while_busy) {
            Some(Ok(events)) => {
                for event in events {
                    emit(event);
                }
                return true;
            }
            Some(Err(state)) => (true, Some(*state)),
            None => (false, None),
        }
    } else {
        let loaded = load();
        (loaded.is_ok(), loaded.ok().flatten())
    };
    if let Some(state) = saved {
        emit(Event::Snapshot(Box::new(state)));
    }
    emit(Event::failed(error.to_string()));
    read
}
