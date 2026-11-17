//! The column of cards that stands at the right of an agent panel.
//!
//! The cards that an agent panel draws stand in one column, the newest at the
//! top, level with the top of that panel. Horizon lays a column out when a card
//! or a panel of its workspace is added, closed, shown, hidden, resized or moved,
//! and never on a frame that changes none of them.

use crate::CardPlace;
use crate::layout::TILE_GAP;
use crate::panel::{Panel, PanelId, browser_actor};
use crate::workspace::WorkspaceId;

use super::super::{Board, vec2_eq};
use super::{panel_rect, rects_overlap};

impl Board {
    /// Whether `id` is a card that is no tile of its workspace: a preset does not
    /// arrange it, and it pushes no panel when it grows.
    pub(crate) fn card_stands_free(&self, id: PanelId) -> bool {
        self.panel(id)
            .and_then(Panel::block)
            .is_some_and(|card| card.place != CardPlace::Tile)
    }

    /// Lays out the column of every agent panel of `workspace`. The card `held` is
    /// under the pointer of the person, so it holds no place yet.
    pub(crate) fn tidy_card_columns(&mut self, workspace: WorkspaceId, held: Option<PanelId>) {
        let previous_frame = self.workspace_frame_rect(workspace);
        let agents: Vec<PanelId> = self
            .panels
            .iter()
            .filter(|panel| panel.workspace_id == workspace && panel.kind.is_agent())
            .map(|panel| panel.id)
            .collect();
        let mut moved = false;
        for agent in agents {
            moved |= self.tidy_card_column(agent, held);
        }
        if moved {
            self.resolve_workspace_collisions_after_frame_growth(workspace, previous_frame);
        }
    }

    /// Lays out the column of the agent panel `agent`. Returns whether a card moved.
    ///
    /// The column stands at the first place at the right of the agent panel where
    /// it covers no other panel of the workspace: a panel that holds that place
    /// puts the column at its own right.
    pub(super) fn tidy_card_column(&mut self, agent: PanelId, held: Option<PanelId>) -> bool {
        let Some(anchor) = self.panel(agent).filter(|panel| panel.visible) else {
            return false;
        };
        let (workspace, actor) = (anchor.workspace_id, browser_actor(&anchor.local_id));
        let [_, top, right, _] = panel_rect(anchor.layout.position, anchor.layout.size);
        let in_column = |panel: &Panel| {
            panel.workspace_id == workspace
                && panel.visible
                && panel.block().is_some_and(|card| card.stands_in_column_of(&actor))
        };
        let cards: Vec<(PanelId, [f32; 2])> = self
            .panels
            .iter()
            .rev()
            .filter(|panel| in_column(panel))
            .map(|panel| (panel.id, panel.layout.size))
            .collect();
        if cards.is_empty() {
            return false;
        }
        let width = cards.iter().fold(0.0, |width: f32, (_, size)| width.max(size[0]));
        let height = cards
            .iter()
            .fold(-TILE_GAP, |height, (_, size)| height + size[1] + TILE_GAP);
        let others: Vec<[f32; 4]> = self
            .panels
            .iter()
            .filter(|panel| panel.workspace_id == workspace && panel.visible && !in_column(panel))
            .filter(|panel| panel.id != agent && Some(panel.id) != held)
            .map(|panel| panel_rect(panel.layout.position, panel.layout.size))
            .collect();
        let mut left = right + TILE_GAP;
        // Each pass leaves one more panel at the left of the column, so the walk
        // is as long as the panels at most, also for a coordinate that is no number.
        for _ in 0..others.len() {
            let column = [left, top, left + width, top + height];
            let Some(holder) = others.iter().find(|other| rects_overlap(column, **other)) else {
                break;
            };
            left = holder[2] + TILE_GAP;
        }
        let mut place = [left, top];
        let mut moved = false;
        for (id, size) in cards {
            if let Some(card) = self.panel_mut(id).filter(|card| !vec2_eq(card.layout.position, place)) {
                card.move_to(place);
                moved = true;
            }
            place[1] += size[1] + TILE_GAP;
        }
        moved
    }
}
