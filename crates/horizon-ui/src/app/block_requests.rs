//! Host-side handling of the agents' card requests.
//!
//! A card is agent-authored text drawn in a person's workspace, so the host
//! checks the caller, its workspace, the owner and the document itself before
//! it changes a panel. The claim of the queue runs on the coordination worker
//! and the apply on the UI thread, as the Device panel requests do: claiming
//! takes a file lock and writing an answer syncs a file.

use std::path::{Path, PathBuf};
use std::time::Instant;

use egui::Context;
use horizon_browser_control::block::{BlockDocument, parse_document};
use horizon_browser_control::manifest::block_machines;
use horizon_browser_control::manifest::block_terminal;
use horizon_core::browser::manifest::{
    self,
    block::{self, CardSummary, Operation, Outcome, Request},
};
use horizon_core::{BlockPanelState, CardRuntime, Panel, PanelId, PanelKind, PanelOptions, WorkspaceId};

use super::HorizonApp;
use super::answer_retries::{RefusedAnswer, RefusedAnswers};
use super::block_machine_keys;
use super::browser_requests::{ActorPanel, actor_panel};
use super::device_requests::HOST_REQUEST_POLL;

/// How often a frame comes back while a prompt line waits for the program of its
/// panel: the line goes out on the frame after that program draws.
const PROMPT_REPAINT: std::time::Duration = std::time::Duration::from_millis(100);

/// A request a claim took. Its file is gone, so this host keeps the request
/// until it answers it.
pub(in crate::app) struct ClaimedBlockRequest {
    /// The queue the request came from; its answer is published there.
    pub(in crate::app) root: Option<PathBuf>,
    pub(in crate::app) request: Request,
}

/// One answer to a claimed request, and the queue it goes to.
pub(in crate::app) struct BlockAnswer {
    root: Option<PathBuf>,
    request: Request,
    outcome: Outcome,
}

impl BlockAnswer {
    pub(in crate::app) fn new(root: Option<&Path>, request: Request, outcome: Outcome) -> Self {
        Self {
            root: root.map(Path::to_path_buf),
            request,
            outcome,
        }
    }
}

/// What this host owes the agents whose card requests a claim took.
#[derive(Default)]
pub(in crate::app) struct BlockClaimState {
    /// A claim of this host's requests runs on the coordination worker.
    pub(in crate::app) claiming: bool,
    /// Requests a claim took that no frame applied yet, in claim order.
    pub(in crate::app) claimed: Vec<ClaimedBlockRequest>,
    /// Answers queued on the coordination worker whose outcome did not apply.
    writing: usize,
    /// Jobs of the coordination worker whose answer no frame wrote yet. Such a
    /// job reads a file or starts `ssh-keygen`, so the frame that queued it
    /// answers nothing, and the request pump must keep waking for the answer.
    worker_jobs: usize,
    /// Answers the worker could not write, for a later frame or pump wake and
    /// until each request's deadline.
    refused: RefusedAnswers<BlockAnswer>,
}

impl BlockClaimState {
    /// Whether a claim runs, a request it took waits, or an answer is not yet
    /// written: each needs a later frame or pump wake to settle.
    fn outstanding(&self) -> bool {
        self.claiming
            || !self.claimed.is_empty()
            || self.writing > 0
            || self.worker_jobs > 0
            || !self.refused.is_empty()
    }

    /// Counts one worker job whose answer a later wake writes.
    pub(in crate::app) fn worker_job_started(&mut self) {
        self.worker_jobs += 1;
    }

    /// Counts one worker job that answered, or that ended before it answered.
    pub(in crate::app) fn worker_job_finished(&mut self) {
        self.worker_jobs = self.worker_jobs.saturating_sub(1);
    }
}

/// The document a request carries, as the host draws it.
fn card_document(document: &serde_json::Value) -> Result<BlockDocument, Outcome> {
    let encoded = serde_json::to_string(document).map_err(|error| {
        Outcome::failed(
            "invalid_document",
            &format!("block document is not valid JSON: {error}"),
        )
    })?;
    parse_document(&encoded).map_err(|error| Outcome::failed(error.code(), &error.to_string()))
}

impl HorizonApp {
    /// The frame poll. See [`Self::drain_block_panel_requests`].
    pub(super) fn poll_block_panel_requests(&mut self, ctx: &Context) -> bool {
        self.drain_block_panel_requests(ctx, None)
    }

    /// Applies each request a claim took, answers it, retries the answers the
    /// coordination worker could not write, and asks that worker for this
    /// host's waiting requests. `root` selects an explicit queue directory;
    /// `None` uses the process runtime root.
    ///
    /// The unpresented host's request pump calls it on each wake, where it is
    /// the only path that applies a request and no later frame is guaranteed.
    /// Returns whether a request applied.
    pub(super) fn drain_block_panel_requests(&mut self, ctx: &Context, root: Option<&Path>) -> bool {
        let changed = self.drain_claimed_block_requests(ctx);
        self.claim_block_requests(root);
        changed
    }

    /// Applies each request a claim took, in claim order, and answers it. A
    /// `next_click` that waits for a click is applied here too: the host keeps
    /// it and answers it when a click arrives or its deadline passes.
    fn drain_claimed_block_requests(&mut self, ctx: &Context) -> bool {
        // The outcome of a claim arrives on the coordination worker. A frame
        // that also drains the Device requests has drained this queue already.
        self.apply_browser_host_io();
        // Click waits settle before a request applies: a click that waits reaches
        // the wait it was made for, and the counts a request reports are fresh.
        let settled = self.settle_block_clicks();
        // The one line that a request asked Horizon to type into a new panel goes
        // out when that program has drawn; until then a frame keeps coming.
        let prompts = self.write_ready_prompts();
        if self.agent_opens.panels.holds_prompts() {
            ctx.request_repaint_after(PROMPT_REPAINT);
        }
        // A card of Horizon that asks about a setting answers at its deadline or
        // when the person closes it without a press.
        let settings = self.settle_setting_requests(ctx);
        let claimed = std::mem::take(&mut self.block_claim.claimed);
        let changed = !claimed.is_empty();
        for request in claimed {
            let outcome = self.apply_block_request_at(&request.request, request.root.as_deref(), ctx);
            if let Some(outcome) = outcome {
                self.write_block_answer(BlockAnswer::new(request.root.as_deref(), request.request, outcome));
            }
        }
        changed | self.retry_block_answers() | settled | settings | prompts
    }

    /// Applies a request against the process runtime root, as a claim's apply
    /// does. Tests use it to judge one operation without a queue.
    #[cfg(test)]
    pub(super) fn apply_block_request(&mut self, request: &Request, ctx: &Context) -> Outcome {
        self.apply_block_request_at(request, None, ctx)
            .expect("the request answers at once")
    }

    /// Applies one request that the host keeps open, without a queue: a test of a
    /// wait judges the card and writes the answer it reads itself.
    #[cfg(test)]
    pub(super) fn apply_block_request_waiting(&mut self, request: &Request, ctx: &Context) -> Option<Outcome> {
        self.apply_block_request_at(request, None, ctx)
    }

    /// Applies one request against an explicit queue directory, as a test does
    /// for an operation whose answer the worker writes later. `None` means no
    /// frame will answer it: the coordination worker owes the answer.
    #[cfg(test)]
    pub(super) fn apply_block_request_in(&mut self, request: &Request, root: &Path, ctx: &Context) -> Option<Outcome> {
        self.apply_block_request_at(request, Some(root), ctx)
    }

    /// Applies one request and returns its answer, or `None` for a `next_click`
    /// the host now holds open.
    fn apply_block_request_at(&mut self, request: &Request, root: Option<&Path>, ctx: &Context) -> Option<Outcome> {
        let actor = match self.request_caller(request) {
            Ok(actor) => actor,
            Err(refusal) => return Some(refusal),
        };
        match &request.operation {
            Operation::Create { title, document } => {
                Some(self.create_card(title.as_deref(), document, actor, &request.actor, ctx))
            }
            Operation::Update { panel_id, document } => {
                Some(self.update_card(panel_id, document, actor, &request.actor, ctx))
            }
            Operation::List => Some(Outcome::Panels {
                panels: self.workspace_card_summaries(actor.workspace_id, &request.actor),
            }),
            Operation::Inspect { panel_id } => Some(self.inspect_card(panel_id, actor, &request.actor)),
            Operation::NextClick { panel_id, .. } => self.begin_click_wait(root, request, panel_id.as_deref()),
            Operation::Visibility { panel_id, visible } => {
                Some(self.change_card(panel_id, *visible, actor, &request.actor, ctx))
            }
            Operation::Close { panel_id } => Some(self.close_card(panel_id, actor, &request.actor, ctx)),
            Operation::OpenTerminal {
                ssh,
                remote_command,
                title,
                key,
                machine,
            } => self.answer_open_terminal(
                &OpenTerminalFields::of(
                    ssh.as_ref(),
                    remote_command.as_deref(),
                    title.as_deref(),
                    key.as_deref(),
                    machine.as_deref(),
                ),
                actor,
                request,
                root,
                ctx,
            ),
            Operation::OpenPanel {
                preset,
                directory,
                title,
                prompt,
            } => self.answer_open_panel(
                &super::block_panels::OpenPanelFields {
                    preset,
                    directory,
                    title: title.as_deref(),
                    prompt: prompt.as_deref(),
                },
                request,
                root,
                ctx,
            ),
            Operation::MachineSet { .. }
            | Operation::MachineDelete { .. }
            | Operation::Machines
            | Operation::MachineKey { .. }
            | Operation::MachineKeys
            | Operation::DeleteMachineKey { .. } => self.answer_machine_request(&request.operation, request, root, ctx),
            Operation::ServerStatus => Some(self.answer_server_status()),
            Operation::Presets => Some(self.answer_presets()),
            Operation::ServerLinkRequest { .. } | Operation::BrowserHostRequest { .. } => {
                self.answer_setting_request(&request.operation, actor, request, root, ctx)
            }
        }
    }

    /// Answers `server_status`: how this computer reaches its server, the state of
    /// that link and the store id that this computer saw last. It reads the entry
    /// that the settings hold in memory and the snapshot of the link, so no frame
    /// waits for a file, and it writes nothing.
    fn answer_server_status(&self) -> Outcome {
        let entry = &self.template_config.server;
        let (link, store) = self.server_sync.as_ref().map_or(("off", None), |sync| {
            let store = sync.store_id();
            (sync.state_code(), store)
        });
        Outcome::ServerStatus {
            server: Box::new(block::ServerLink {
                ssh_target: entry.ssh_target.clone(),
                machine: entry.machine.clone(),
                command: entry.command.clone(),
                address: entry.address.clone(),
                link: link.to_owned(),
                store,
            }),
        }
    }

    /// Answers `open_terminal`: a route of its own, or the record of a machine of
    /// this Horizon. A caller that names a machine passes none of the fields that
    /// the record holds.
    fn answer_open_terminal(
        &mut self,
        asked: &OpenTerminalFields<'_>,
        actor: ActorPanel,
        request: &Request,
        root: Option<&Path>,
        ctx: &Context,
    ) -> Option<Outcome> {
        let OpenTerminalFields {
            ssh,
            remote_command,
            title,
            key,
            machine,
        } = *asked;
        if let Err(refusal) = block_machines::check_single_target(ssh.is_some(), machine.is_some()) {
            return Some(Outcome::failed(refusal.code, &refusal.message));
        }
        if let Some(machine) = machine {
            for (field, given) in [
                ("ssh", ssh.is_some()),
                ("key", key.is_some()),
                ("remote_command", remote_command.is_some()),
            ] {
                if given {
                    return Some(Outcome::failed(
                        block_machines::INVALID_REQUEST,
                        &format!(
                            "open_terminal takes {field} from the machine record: leave {field} out when you pass machine"
                        ),
                    ));
                }
            }
            self.answer_machine_terminal(machine, title, actor, request, root, ctx);
            return None;
        }
        let Some(ssh) = ssh else {
            return Some(Outcome::failed(
                block_machines::INVALID_REQUEST,
                "open_terminal needs ssh or machine",
            ));
        };
        let terminal = match block_terminal::check(ssh, remote_command) {
            Ok(terminal) => terminal,
            Err(refusal) => return Some(Outcome::failed(refusal.code, &refusal.message)),
        };
        match key {
            None => Some(self.open_terminal(&terminal, title, None, actor, &request.actor, ctx)),
            Some(key) => {
                // The key lives in a file, so the worker checks it and the frame
                // that applies the check opens the panel.
                self.answer_key_terminal(
                    &block_machine_keys::KeyTerminal {
                        terminal: &terminal,
                        title,
                        key,
                    },
                    request,
                    root,
                    ctx,
                );
                None
            }
        }
    }

    /// The checks every request passes before Horizon applies it: the host it
    /// belongs to, its deadline, and the calling agent panel.
    pub(super) fn request_caller(&self, request: &Request) -> Result<ActorPanel, Outcome> {
        if request.host_instance != manifest::host_instance() {
            return Err(Outcome::failed("wrong_host", "Request belongs to another Horizon host"));
        }
        if request.deadline_at_millis <= manifest::now_millis() {
            return Err(request_expired());
        }
        actor_panel(&self.board, &request.actor).ok_or_else(|| {
            Outcome::failed(
                "caller_unavailable",
                "The calling agent panel is no longer in this host",
            )
        })
    }

    /// The mark of a panel that an agent panel opened: the display name of the
    /// kind of that panel, which is Horizon's own text and never a string of the
    /// request.
    pub(super) fn opener_mark(&self, actor: ActorPanel) -> String {
        self.board
            .panel(actor.panel_id)
            .map_or_else(|| "Agent".to_owned(), |panel| panel.kind.display_name().to_owned())
    }

    /// The cards of one workspace as `list` reports them.
    fn workspace_card_summaries(&mut self, workspace: WorkspaceId, caller: &str) -> Vec<CardSummary> {
        let cards: Vec<PanelId> = self.workspace_cards(workspace).iter().map(|panel| panel.id).collect();
        cards
            .into_iter()
            .filter_map(|id| self.card_summary(id, caller))
            .collect()
    }

    /// One card as `list` reports it.
    fn card_summary(&mut self, id: PanelId, caller: &str) -> Option<CardSummary> {
        let panel = self.board.panel(id)?;
        let block = panel.block()?;
        let (panel_id, title, revision) = (panel.local_id.clone(), block.title.clone(), block.revision);
        let owned_by_caller = block.owner_actor.as_deref() == Some(caller);
        Some(CardSummary {
            panel_id,
            title,
            revision,
            owned_by_caller,
            pending_clicks: self.card_pending_clicks(id),
            listening: self.block_listening(id),
        })
    }

    fn create_card(
        &mut self,
        title: Option<&str>,
        document: &serde_json::Value,
        actor: ActorPanel,
        owner: &str,
        ctx: &Context,
    ) -> Outcome {
        let document = match card_document(document) {
            Ok(document) => document,
            Err(refusal) => return refusal,
        };
        let card_count = self
            .workspace_cards(actor.workspace_id)
            .iter()
            .filter(|panel| card_owner(panel) == Some(owner))
            .count();
        if card_count >= block::MAX_CARDS_PER_OWNER {
            return Outcome::failed(
                "too_many_cards",
                &format!(
                    "This agent already owns the maximum of {} cards in this workspace; close one before creating another",
                    block::MAX_CARDS_PER_OWNER
                ),
            );
        }
        let title = title.map(BlockPanelState::normalize_title);
        let size = crate::block_widget::default_card_size(&document, self.canvas_rect(ctx).width());
        let state = BlockPanelState {
            title: title
                .clone()
                .unwrap_or_else(|| BlockPanelState::DEFAULT_TITLE.to_owned()),
            document,
            revision: 1,
            owner_actor: Some(owner.to_owned()),
            updated_at_millis: manifest::now_millis(),
        };
        let revision = state.revision;
        let options = PanelOptions {
            kind: PanelKind::Block,
            block: Some(state),
            name: title,
            size: Some(size),
            ..PanelOptions::default()
        };
        let focused = self.board.focused;
        let beside = self.new_card_beside(actor, owner);
        let Ok(id) = self.create_agent_child_panel(options, actor.workspace_id, beside) else {
            return Outcome::failed("panel_unavailable", "Horizon could not create a card in this workspace");
        };
        // Creating a card must not steal keyboard input from the caller.
        if let Some(focused) = focused {
            self.board.focus(focused);
        }
        self.mark_runtime_dirty();
        ctx.request_repaint();
        let Some(panel) = self.board.panel(id) else {
            return Outcome::failed("panel_unavailable", "The new card is not in this workspace");
        };
        Outcome::Created {
            panel_id: panel.local_id.clone(),
            revision,
        }
    }

    fn update_card(
        &mut self,
        panel_id: &str,
        document: &serde_json::Value,
        actor: ActorPanel,
        owner: &str,
        ctx: &Context,
    ) -> Outcome {
        let id = match self.owned_card(panel_id, actor, owner) {
            Ok(id) => id,
            Err(refusal) => return refusal,
        };
        let document = match card_document(document) {
            Ok(document) => document,
            Err(refusal) => return refusal,
        };
        // The room of the canvas decides how wide a wide card opens, and the
        // card below holds a borrow of the board for the rest of the update.
        let canvas_width = self.canvas_rect(ctx).width();
        let Some(state) = self.board.panel_mut(id).and_then(Panel::block_mut) else {
            return Outcome::failed("not_block_panel", "Target is not a card");
        };
        let was_wide = crate::block_widget::asks_wide(&state.document);
        let revision = state.replace_document(document);
        state.updated_at_millis = manifest::now_millis();
        let now_wide = crate::block_widget::asks_wide(&state.document);
        // The person keeps a selection only while its row is still drawn.
        if let Some(card) = self.panel_render_caches.card_runtime.get_mut(&id) {
            card.retain_selection(&state.document);
        }
        // The width that the document asks for now, before the borrow of the
        // card ends: a wide card opens as wide as its board needs.
        let width = crate::block_widget::wide_card_width(&state.document, canvas_width);
        if !was_wide && now_wide {
            self.widen_card(id, width);
        }
        self.mark_runtime_dirty();
        ctx.request_repaint();
        Outcome::Updated {
            panel_id: panel_id.to_owned(),
            revision,
        }
    }

    /// Widens a card whose document now asks for the wide card to `width`,
    /// unless the person resized the card by hand: the person's size outranks
    /// the document, and the work keeps one card.
    fn widen_card(&mut self, id: PanelId, width: f32) {
        let hand_resized = self
            .panel_render_caches
            .card_runtime
            .get(&id)
            .is_some_and(CardRuntime::hand_resized);
        let Some(size) = self.board.panel(id).map(|panel| panel.layout.size) else {
            return;
        };
        if hand_resized || width <= size[0] {
            return;
        }
        self.board.resize_panel(id, [width, size[1]]);
    }

    fn inspect_card(&mut self, panel_id: &str, actor: ActorPanel, owner: &str) -> Outcome {
        let panel = match self.card_panel(panel_id, actor.workspace_id) {
            Ok(panel) => panel,
            Err(refusal) => return refusal,
        };
        let Some(state) = panel.block() else {
            return Outcome::failed("not_block_panel", "Target is not a card");
        };
        let id = panel.id;
        let title = state.title.clone();
        let revision = state.revision;
        let document = state.document.clone();
        let visible = panel.visible;
        let owned_by_caller = state.owner_actor.as_deref() == Some(owner);
        let agent_present = self.card_owner_panel(panel).is_some();
        Outcome::Inspected {
            panel_id: panel_id.to_owned(),
            title,
            revision,
            document,
            visible,
            agent_present,
            owned_by_caller,
            pending_clicks: self.card_pending_clicks(id),
            listening: self.block_listening(id),
        }
    }

    fn change_card(&mut self, panel_id: &str, visible: bool, actor: ActorPanel, owner: &str, ctx: &Context) -> Outcome {
        let id = match self.owned_card(panel_id, actor, owner) {
            Ok(id) => id,
            Err(refusal) => return refusal,
        };
        self.board.set_panel_visible(id, visible);
        if !visible && self.fullscreen_panel == Some(id) {
            self.fullscreen_panel = None;
        }
        if !visible && self.board.focused.is_none() {
            self.board.focus(actor.panel_id);
        }
        self.mark_runtime_dirty();
        ctx.request_repaint();
        Outcome::Visibility {
            panel_id: panel_id.to_owned(),
            visible,
        }
    }

    /// Answers `close`: the caller's own card, or the card of an owner panel that
    /// left the workspace, so a later coordinator can clean up after a plan of an
    /// earlier run. A card with a living owner stays closed to every other agent.
    fn close_card(&mut self, panel_id: &str, actor: ActorPanel, owner: &str, ctx: &Context) -> Outcome {
        let panel = match self.card_panel(panel_id, actor.workspace_id) {
            Ok(panel) => panel,
            Err(refusal) => return refusal,
        };
        let by_owner = card_owner(panel) == Some(owner);
        if !by_owner && self.card_owner_panel(panel).is_some() {
            return Outcome::failed(
                "not_owner",
                "Only the agent that created this card may close it: its owner is still in this workspace",
            );
        }
        let id = panel.id;
        if self.fullscreen_panel == Some(id) {
            self.fullscreen_panel = None;
        }
        let focused = self.board.focused == Some(id);
        let _ = self.close_panel_returning_teardown(id);
        if focused {
            self.board.focus(actor.panel_id);
        }
        self.mark_runtime_dirty();
        ctx.request_repaint();
        Outcome::Closed {
            panel_id: panel_id.to_owned(),
            by_owner,
            closed_by: self
                .board
                .panel(actor.panel_id)
                .map_or_else(String::new, |panel| panel.local_id.clone()),
        }
    }

    /// The panel a new card of `owner` takes a free place beside: the card that
    /// agent created last in this workspace, or the agent panel itself.
    fn new_card_beside(&self, actor: ActorPanel, owner: &str) -> PanelId {
        self.workspace_cards(actor.workspace_id)
            .iter()
            .rfind(|panel| card_owner(panel) == Some(owner))
            .map_or(actor.panel_id, |panel| panel.id)
    }

    /// The cards of one workspace, in board order.
    pub(in crate::app) fn workspace_cards(&self, workspace: WorkspaceId) -> Vec<&Panel> {
        self.board
            .panels
            .iter()
            .filter(|panel| panel.workspace_id == workspace && panel.kind == PanelKind::Block)
            .collect()
    }

    /// A card of the caller's workspace, by its local id. A panel of another
    /// workspace is unavailable rather than readable: the caller never learns
    /// what another workspace holds.
    fn card_panel(&self, panel_id: &str, workspace: WorkspaceId) -> Result<&Panel, Outcome> {
        let panel = self
            .board
            .panel_id_by_local_id(panel_id)
            .and_then(|id| self.board.panel(id))
            .filter(|panel| panel.workspace_id == workspace)
            .ok_or_else(|| Outcome::failed("panel_unavailable", "Panel is unavailable in this workspace"))?;
        if panel.block().is_none() {
            return Err(Outcome::failed("not_block_panel", "Target is not a card"));
        }
        Ok(panel)
    }

    /// The panel of a card that this caller owns; every other agent of the
    /// workspace reads it and changes nothing.
    pub(super) fn owned_card(&self, panel_id: &str, actor: ActorPanel, owner: &str) -> Result<PanelId, Outcome> {
        let panel = self.card_panel(panel_id, actor.workspace_id)?;
        if card_owner(panel) != Some(owner) {
            return Err(Outcome::failed(
                "not_owner",
                "Only the agent that created this card may change it",
            ));
        }
        Ok(panel.id)
    }

    /// Claims this host's waiting card requests on the coordination worker, at
    /// most every [`HOST_REQUEST_POLL`]. One claim runs at a time, so a slow
    /// disk never queues one per tick.
    pub(in crate::app) fn claim_block_requests(&mut self, root: Option<&Path>) {
        let now = Instant::now();
        if self.block_claim.claiming
            || self
                .panel_render_caches
                .block_request_poll
                .is_some_and(|last| now.duration_since(last) < HOST_REQUEST_POLL)
        {
            return;
        }
        self.panel_render_caches.block_request_poll = Some(now);
        self.block_claim.claiming = true;
        let claiming = root.map(Path::to_path_buf);
        let board = self.browser_create_host.io.board_fence();
        self.browser_create_host.io.then(
            move || board.holds().then(|| claim_requests(claiming.as_deref())),
            |app, claimed| app.apply_block_claim(claimed.flatten()),
        );
    }

    /// Keeps each request the claim took, in claim order, or refuses it at once
    /// when no frame will apply it because the board it was sent to was
    /// replaced.
    fn apply_block_claim(&mut self, claimed: Option<Vec<ClaimedBlockRequest>>) {
        self.block_claim.claiming = false;
        let Some(claimed) = claimed else { return };
        let retiring = self.browser_host_retiring();
        for request in claimed {
            if retiring {
                self.write_block_answer(BlockAnswer::new(
                    request.root.as_deref(),
                    request.request,
                    request_expired(),
                ));
            } else {
                self.block_claim.claimed.push(request);
            }
        }
    }

    /// Writes one answer on the coordination worker. A write that fails, or a
    /// job that panics, keeps the answer: the request file is gone, so this
    /// host writes the same result again on a later frame or pump wake until
    /// the request's deadline.
    pub(in crate::app) fn write_block_answer(&mut self, answer: BlockAnswer) {
        let request_id = answer.request.request_id.clone();
        let deadline_at_millis = answer.request.deadline_at_millis;
        let writing = answer.request.clone();
        let (root, outcome) = (answer.root.clone(), answer.outcome.clone());
        self.block_claim.writing += 1;
        self.browser_create_host.io.then(
            move || complete_block_request(root.as_deref(), &writing, outcome),
            move |app, written| {
                app.block_claim.writing -= 1;
                if matches!(written, Some(Ok(()))) {
                    return;
                }
                tracing::debug!(%request_id, "the coordination worker could not answer a card request yet");
                app.block_claim
                    .refused
                    .keep(RefusedAnswer::new(answer, request_id, deadline_at_millis));
            },
        );
    }

    /// Writes the answers the worker could not write again, at most every
    /// retry interval and until each request's deadline.
    pub(in crate::app) fn retry_block_answers(&mut self) -> bool {
        let due = self.block_claim.refused.due(manifest::now_millis());
        let retried = !due.is_empty();
        for answer in due {
            self.write_block_answer(answer.result);
        }
        retried
    }

    /// Whether this host still owes an answer to a card request a claim took.
    /// The request pump keeps waking while it does, so each answer lands also
    /// when the host runs no frame. A prompt line that waits for the program of
    /// its panel keeps the pump awake for the same reason.
    pub(in crate::app) fn block_claims_outstanding(&self) -> bool {
        self.block_claim.outstanding() || self.agent_opens.panels.holds_prompts()
    }
}

/// The fields of one `open_terminal` request, as the dispatch reads them.
struct OpenTerminalFields<'a> {
    ssh: Option<&'a block_terminal::TerminalRoute>,
    remote_command: Option<&'a str>,
    title: Option<&'a str>,
    key: Option<&'a str>,
    machine: Option<&'a str>,
}

impl<'a> OpenTerminalFields<'a> {
    /// The fields of the operation, as borrowed values of the request.
    fn of(
        ssh: Option<&'a block_terminal::TerminalRoute>,
        remote_command: Option<&'a str>,
        title: Option<&'a str>,
        key: Option<&'a str>,
        machine: Option<&'a str>,
    ) -> Self {
        Self {
            ssh,
            remote_command,
            title,
            key,
            machine,
        }
    }
}

pub(in crate::app) fn card_owner(panel: &Panel) -> Option<&str> {
    panel.block()?.owner_actor.as_deref()
}

/// The refusal of a request whose deadline passed before Horizon applied it.
fn request_expired() -> Outcome {
    Outcome::failed("request_expired", "Block panel request expired before dispatch")
}

/// The requests this host claims, in queue order. A queue that cannot be read
/// leaves its requests for a later tick.
fn claim_requests(root: Option<&Path>) -> Vec<ClaimedBlockRequest> {
    let host = manifest::host_instance();
    let claimed = match root {
        Some(root) => block::claim_at(root, host),
        None => block::claim(host),
    };
    match claimed {
        Ok(requests) => requests
            .into_iter()
            .map(|request| ClaimedBlockRequest {
                root: root.map(Path::to_path_buf),
                request,
            })
            .collect(),
        Err(error) => {
            tracing::warn!(%error, "could not poll block panel requests");
            Vec::new()
        }
    }
}

/// Writes the result envelope of one request.
fn complete_block_request(root: Option<&Path>, request: &Request, outcome: Outcome) -> std::io::Result<()> {
    match root {
        Some(root) => block::complete_at(root, request, outcome),
        None => block::complete(request, outcome),
    }
}

#[cfg(test)]
mod tests;
