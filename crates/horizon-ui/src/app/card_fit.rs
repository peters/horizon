//! Fits a card panel to the height its content needs.
//!
//! The body measures its content in the pass that draws it, because only that
//! pass has the font atlas. After the pass, the host gives the card the height
//! the measurement asks for, once per document revision and set of
//! Horizon-owned frames: the card opens and changes at the size of its content,
//! a card that the person resized by hand keeps the person's size, and a card
//! shorter than its content scrolls its body above a footer that stays put. One
//! repaint settles a new height, so no frame loop starts.
//!
//! The frames that Horizon adds above the body, the one that explains a card
//! whose agent left and the one that reports a refused click, take room like a
//! component does. A card therefore fits again when that set changes, not only
//! when its document changes.
//!
//! A workspace that the person arranged with a preset owns the size of every
//! panel in it, so a card there keeps the preset's size.

use egui::Context;

use horizon_core::{Panel, PanelId, PanelKind};

use super::HorizonApp;
use crate::block_widget::{CARD_HEIGHT_LIMITS, card_height_limits, fitted_card_height};

impl HorizonApp {
    /// Gives every card panel the height that its content of the current
    /// revision and frames needs, and marks that pair as fitted.
    ///
    /// Runs after the pass that measured the content. A card whose document and
    /// frames did not change since its last fit is left alone, so a person's
    /// resize and a settled card never fight the measurement.
    pub(super) fn fit_block_cards(&mut self, ctx: &Context) {
        let cards: Vec<PanelId> = self
            .board
            .panels
            .iter()
            .filter(|panel| panel.kind == PanelKind::Block)
            .map(|panel| panel.id)
            .collect();
        let mut resized = false;
        for id in cards {
            let Some(revision) = self
                .board
                .panel(id)
                .and_then(|panel| panel.block())
                .map(|bl| bl.revision)
            else {
                continue;
            };
            let Some(card) = self.panel_render_caches.card_runtime.get(&id) else {
                continue;
            };
            if !card.needs_content_fit(revision) {
                continue;
            }
            let Some(measured) = card.measured_content_height() else {
                continue;
            };
            let hand_resized = card.hand_resized();
            // The bounds of the height come from the document: a wide card may
            // grow taller than the narrow card of today.
            let limits = self
                .board
                .panel(id)
                .and_then(Panel::block)
                .map_or(CARD_HEIGHT_LIMITS, |state| card_height_limits(&state.document));
            let Some((size, workspace_id)) = self
                .board
                .panel(id)
                .map(|panel| (panel.layout.size, panel.workspace_id))
            else {
                continue;
            };
            // An arranged workspace gives every panel its preset size.
            if self
                .board
                .workspace(workspace_id)
                .is_some_and(|workspace| workspace.layout.is_some())
            {
                continue;
            }
            let fitted = fitted_card_height(measured, hand_resized, size[1], limits);
            if (fitted - size[1]).abs() > f32::EPSILON {
                resized |= self.board.resize_panel(id, [size[0], fitted]);
            }
            if let Some(card) = self.panel_render_caches.card_runtime.get_mut(&id) {
                card.mark_content_fit(revision);
            }
        }
        if resized {
            self.mark_runtime_dirty();
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests;
