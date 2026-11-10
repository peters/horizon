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
use horizon_core::browser::manifest::{
    self,
    block::{self, CardSummary, Operation, Outcome, Request},
};
use horizon_core::{BlockPanelState, Panel, PanelId, PanelKind, PanelOptions, WorkspaceId, browser_actor};

use super::HorizonApp;
use super::answer_retries::{RefusedAnswer, RefusedAnswers};
use super::browser_requests::{ActorPanel, actor_panel};
use super::device_requests::HOST_REQUEST_POLL;

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
    /// Answers the worker could not write, for a later frame or pump wake and
    /// until each request's deadline.
    refused: RefusedAnswers<BlockAnswer>,
}

impl BlockClaimState {
    /// Whether a claim runs, a request it took waits, or an answer is not yet
    /// written: each needs a later frame or pump wake to settle.
    fn outstanding(&self) -> bool {
        self.claiming || !self.claimed.is_empty() || self.writing > 0 || !self.refused.is_empty()
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

    /// Applies each request a claim took, in claim order, and answers it.
    fn drain_claimed_block_requests(&mut self, ctx: &Context) -> bool {
        // The outcome of a claim arrives on the coordination worker. A frame
        // that also drains the Device requests has drained this queue already.
        self.apply_browser_host_io();
        let claimed = std::mem::take(&mut self.block_claim.claimed);
        let changed = !claimed.is_empty();
        for request in claimed {
            let outcome = self.apply_block_request_at(&request.request, ctx);
            self.write_block_answer(BlockAnswer::new(request.root.as_deref(), request.request, outcome));
        }
        changed | self.retry_block_answers()
    }

    /// Applies a request against the process runtime root, as a claim's apply
    /// does. Tests use it to judge one operation without a queue.
    #[cfg(test)]
    fn apply_block_request(&mut self, request: &Request, ctx: &Context) -> Outcome {
        self.apply_block_request_at(request, ctx)
    }

    /// Applies one request.
    fn apply_block_request_at(&mut self, request: &Request, ctx: &Context) -> Outcome {
        if request.host_instance != manifest::host_instance() {
            return Outcome::failed("wrong_host", "Request belongs to another Horizon host");
        }
        if request.deadline_at_millis <= manifest::now_millis() {
            return request_expired();
        }
        let Some(actor) = actor_panel(&self.board, &request.actor) else {
            return Outcome::failed(
                "caller_unavailable",
                "The calling agent panel is no longer in this host",
            );
        };
        match &request.operation {
            Operation::Create { title, document } => {
                self.create_card(title.as_deref(), document, actor, &request.actor, ctx)
            }
            Operation::Update { panel_id, document } => {
                self.update_card(panel_id, document, actor, &request.actor, ctx)
            }
            Operation::List => Outcome::Panels {
                panels: self
                    .workspace_cards(actor.workspace_id)
                    .into_iter()
                    .map(|panel| CardSummary {
                        panel_id: panel.local_id.clone(),
                        title: panel.block().map_or_else(String::new, |block| block.title.clone()),
                        revision: panel.block().map_or(0, |block| block.revision),
                        owned_by_caller: card_owner(panel) == Some(request.actor.as_str()),
                    })
                    .collect(),
            },
            Operation::Inspect { panel_id } => self.inspect_card(panel_id, actor, &request.actor),
            Operation::Visibility { panel_id, visible } => {
                self.change_card(panel_id, *visible, actor, &request.actor, ctx)
            }
            Operation::Close { panel_id } => self.close_card(panel_id, actor, &request.actor, ctx),
        }
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
        let state = BlockPanelState {
            title: title
                .clone()
                .unwrap_or_else(|| BlockPanelState::DEFAULT_TITLE.to_owned()),
            document,
            revision: 1,
            owner_actor: Some(owner.to_owned()),
        };
        let revision = state.revision;
        let options = PanelOptions {
            kind: PanelKind::Block,
            block: Some(state),
            name: title,
            ..PanelOptions::default()
        };
        let focused = self.board.focused;
        let Ok(id) = self.create_agent_child_panel(options, actor.workspace_id, actor.panel_id) else {
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
        let Some(state) = self.board.panel_mut(id).and_then(Panel::block_mut) else {
            return Outcome::failed("not_block_panel", "Target is not a card");
        };
        let revision = state.replace_document(document);
        self.mark_runtime_dirty();
        ctx.request_repaint();
        Outcome::Updated {
            panel_id: panel_id.to_owned(),
            revision,
        }
    }

    fn inspect_card(&self, panel_id: &str, actor: ActorPanel, owner: &str) -> Outcome {
        let panel = match self.card_panel(panel_id, actor.workspace_id) {
            Ok(panel) => panel,
            Err(refusal) => return refusal,
        };
        let Some(state) = panel.block() else {
            return Outcome::failed("not_block_panel", "Target is not a card");
        };
        let agent_present = state.owner_actor.as_deref().is_some_and(|owner_actor| {
            self.board.panels.iter().any(|panel| {
                panel.workspace_id == actor.workspace_id
                    && panel.kind.is_agent()
                    && browser_actor(&panel.local_id) == owner_actor
            })
        });
        Outcome::Inspected {
            panel_id: panel_id.to_owned(),
            title: state.title.clone(),
            revision: state.revision,
            document: state.document.clone(),
            visible: panel.visible,
            agent_present,
            owned_by_caller: state.owner_actor.as_deref() == Some(owner),
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

    fn close_card(&mut self, panel_id: &str, actor: ActorPanel, owner: &str, ctx: &Context) -> Outcome {
        let id = match self.owned_card(panel_id, actor, owner) {
            Ok(id) => id,
            Err(refusal) => return refusal,
        };
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
        }
    }

    /// The cards of one workspace, in board order.
    fn workspace_cards(&self, workspace: WorkspaceId) -> Vec<&Panel> {
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
    fn owned_card(&self, panel_id: &str, actor: ActorPanel, owner: &str) -> Result<PanelId, Outcome> {
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
    /// when the host runs no frame.
    pub(in crate::app) fn block_claims_outstanding(&self) -> bool {
        self.block_claim.outstanding()
    }
}

fn card_owner(panel: &Panel) -> Option<&str> {
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
