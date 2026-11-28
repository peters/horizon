//! The card operations of this host: create, update, inspect, show, hide and close a
//! card, with the reads that name a card of the workspace of its caller.
//!
//! Every operation reads the caller and the owner of the card before it changes a
//! panel, and each one answers the `Outcome` of its request.

use egui::Context;
use horizon_core::agent_bridge::actor;
use horizon_core::browser::manifest::{
    self,
    block::{self, CardSummary, Outcome},
};
use horizon_core::{BlockPanelState, CardPlace, Panel, PanelId, PanelKind, PanelOptions, WorkspaceId};

use super::super::HorizonApp;
use super::super::browser_requests::actor_panel;
use super::{CardCaller, caller_unavailable, card_document, card_owner};

impl HorizonApp {
    /// The cards of one workspace as `list` reports them.
    pub(super) fn workspace_card_summaries(&mut self, workspace: WorkspaceId, caller: &str) -> Vec<CardSummary> {
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

    pub(super) fn create_card(
        &mut self,
        title: Option<&str>,
        document: &serde_json::Value,
        caller: CardCaller<'_>,
        owner: &str,
        ctx: &Context,
    ) -> Outcome {
        let document = match card_document(document) {
            Ok(document) => document,
            Err(refusal) => return refusal,
        };
        // The first card of a session makes the workspace of its machine.
        let workspace = match caller {
            CardCaller::Panel(panel) => panel.workspace_id,
            // A card of an orchestrator lands in the workspace of its actor, as a
            // card of a machine lands in the workspace of that machine.
            CardCaller::Orchestrator { workspace } => workspace,
            CardCaller::Session { machine } => self.machine_workspace(machine, ctx),
            // The front desk creates no card, and its own path answers before this.
            CardCaller::FrontDesk => return caller_unavailable(),
        };
        let card_count = self
            .workspace_cards(workspace)
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
            answered_callout: None,
            // The card of an agent panel stands in the column of that panel.
            place: caller.panel().map_or(CardPlace::Tile, |_| CardPlace::Column),
            fold: None,
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
        let beside = caller.panel().map(|panel| panel.panel_id);
        let Ok(id) = self.create_agent_child_panel(options, workspace, beside) else {
            return Outcome::failed("panel_unavailable", "Horizon could not create a card in this workspace");
        };
        // Creating a card must not steal keyboard input from the caller.
        if let Some(focused) = focused {
            self.board.focus(focused);
        }
        self.settle_card_fold(ctx, id, false);
        self.note_card_row(id);
        self.show_card_of_shown_conversation(ctx, caller, id);
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

    /// Adds or moves the reference row of the card `id` in the conversation of
    /// its workspace, when the orchestrator of that workspace owns the card. The
    /// dock draws one row for the card, which brings it into view.
    fn note_card_row(&mut self, id: PanelId) {
        let Some(panel) = self.board.panel(id) else {
            return;
        };
        let Some(heading) = panel.block().map(|block| block.heading().to_owned()) else {
            return;
        };
        let (local_id, workspace) = (panel.local_id.clone(), panel.workspace_id);
        let Some(owner_local_id) = card_owner(panel).and_then(actor::workspace_of).map(str::to_owned) else {
            return;
        };
        // A program of the dock reads and closes only the cards of the workspace
        // that its actor names, so the row of a card of another workspace is no
        // reference to this conversation.
        if self.board.workspace_id_by_local_id(&owner_local_id) != Some(workspace) {
            return;
        }
        self.orchestrator_dock.card_row(&owner_local_id, &local_id, &heading);
    }

    pub(super) fn update_card(
        &mut self,
        panel_id: &str,
        document: &serde_json::Value,
        owner: &str,
        ctx: &Context,
    ) -> Outcome {
        let id = match self.owned_card(panel_id, owner) {
            Ok(id) => id,
            Err(refusal) => return refusal,
        };
        let document = match card_document(document) {
            Ok(document) => document,
            Err(refusal) => return refusal,
        };
        // The card below holds a borrow of the board for the rest of the update.
        let Some(state) = self.board.panel_mut(id).and_then(Panel::block_mut) else {
            return Outcome::failed("not_block_panel", "Target is not a card");
        };
        let revision = state.replace_document(document);
        state.updated_at_millis = manifest::now_millis();
        // The person keeps a selection only while its row is still drawn.
        if let Some(card) = self.panel_render_caches.card_runtime.get_mut(&id) {
            card.retain_selection(&state.document);
        }
        // The card takes the width that its document asks for in the pass that
        // fits it, after the body measured the content of this revision.
        self.settle_card_fold(ctx, id, false);
        self.note_card_row(id);
        self.mark_runtime_dirty();
        ctx.request_repaint();
        Outcome::Updated {
            panel_id: panel_id.to_owned(),
            revision,
        }
    }

    pub(super) fn inspect_card(&mut self, panel_id: &str, owner: &str) -> Outcome {
        let panel = match self.owner_card(panel_id, owner) {
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
        let agent_present = self.card_owner_present(panel);
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

    pub(super) fn change_card(&mut self, panel_id: &str, visible: bool, owner: &str, ctx: &Context) -> Outcome {
        let id = match self.owned_card(panel_id, owner) {
            Ok(id) => id,
            Err(refusal) => return refusal,
        };
        self.board.set_panel_visible(id, visible);
        if !visible && self.fullscreen_panel == Some(id) {
            self.fullscreen_panel = None;
        }
        if !visible
            && self.board.focused.is_none()
            && let Some(panel) = actor_panel(&self.board, owner)
        {
            self.board.focus(panel.panel_id);
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
    pub(super) fn close_card(&mut self, panel_id: &str, owner: &str, ctx: &Context) -> Outcome {
        let panel = match self.owner_card(panel_id, owner) {
            Ok(panel) => panel,
            Err(refusal) => return refusal,
        };
        let by_owner = card_owner(panel) == Some(owner);
        if !by_owner && self.card_owner_present(panel) {
            return Outcome::failed(
                "not_owner",
                "Only the agent that created this card may close it: its owner is still in this workspace",
            );
        }
        let id = panel.id;
        // Only a panel of this board takes the keyboard back: the session of a
        // machine is no panel here.
        let closing = actor_panel(&self.board, owner).map(|closer| closer.panel_id);
        if self.fullscreen_panel == Some(id) {
            self.fullscreen_panel = None;
        }
        let focused = self.board.focused == Some(id);
        let _ = self.close_panel_returning_teardown(id);
        if focused && let Some(closer) = closing {
            self.board.focus(closer);
        }
        self.mark_runtime_dirty();
        ctx.request_repaint();
        Outcome::Closed {
            panel_id: panel_id.to_owned(),
            by_owner,
            closed_by: closing
                .and_then(|closer| self.board.panel(closer))
                .map_or_else(String::new, |panel| panel.local_id.clone()),
        }
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

    /// A card of the workspace of `owner`, or the refusal of a caller that is gone.
    pub(in crate::app) fn owner_card(&self, panel_id: &str, owner: &str) -> Result<&Panel, Outcome> {
        let workspace = self.owner_workspace(owner).ok_or_else(caller_unavailable)?;
        self.card_panel(panel_id, workspace)
    }

    /// The panel of a card that this caller owns; every other agent reads it.
    pub(in crate::app) fn owned_card(&self, panel_id: &str, owner: &str) -> Result<PanelId, Outcome> {
        let panel = self.owner_card(panel_id, owner)?;
        if card_owner(panel) != Some(owner) {
            return Err(Outcome::failed(
                "not_owner",
                "Only the agent that created this card may change it",
            ));
        }
        Ok(panel.id)
    }
}
