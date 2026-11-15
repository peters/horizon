//! Fits a card panel to the size its content needs.
//!
//! The body measures its content in the pass that draws it, because only that
//! pass has the font atlas. After the pass, the host gives the card the size the
//! measurement asks for, once per document revision and set of Horizon-owned
//! frames: the card opens, widens and changes at the size of its content, a card
//! that the person resized by hand keeps the person's size, and a card shorter
//! than its content scrolls its body above a footer that stays put. One repaint
//! settles a new size, so no frame loop starts.
//!
//! The height of a card is bounded by the ceiling of its document and by the room
//! that the canvas view shows at the zoom of the moment: a card that fits in the
//! window of the person takes the height of its content, so nothing of it hides
//! under its footer, and a card that is taller than that room scrolls as before.
//!
//! The width follows the document too: a wide document takes the width that its
//! board asks for at every update, and a document that is narrow again takes the
//! narrow card. The left edge of the panel stays where it is, so the card grows
//! and shrinks to the right.
//!
//! The frames that Horizon adds above the body, the one that explains a card
//! whose agent left and the one that reports a refused click, take room like a
//! component does. A card therefore fits again when that set changes, not only
//! when its document changes.
//!
//! A workspace that the person arranged with a preset owns the size of every
//! panel in it, so a card there keeps the preset's size.

use egui::Context;

use horizon_core::{PanelId, PanelKind};

use super::HorizonApp;
use crate::block_widget::{card_height_limits, card_width, fitted_card_height};

impl HorizonApp {
    /// Gives every card panel the size that its content of the current revision
    /// and frames needs, and marks that pair as fitted.
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
        // The room of the canvas decides how wide a wide card may be, and how tall
        // a card may grow: the visible height of the view at the zoom of this moment.
        let canvas = self.canvas_rect(ctx);
        let canvas_width = canvas.width();
        let canvas_height = canvas.height() / self.canvas_view.zoom;
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
            // The bounds of the height and the width come from the document: a
            // wide card may grow taller and wider than the narrow card of today.
            let Some((mut limits, width, current, workspace_id)) = self.board.panel(id).and_then(|panel| {
                let document = &panel.block()?.document;
                Some((
                    card_height_limits(document),
                    card_width(document, canvas_width),
                    panel.layout.size,
                    panel.workspace_id,
                ))
            }) else {
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
            // A card of a person's window may take the room that the view shows, so
            // a card that fits there needs no scroll. A card that a short window
            // cannot show keeps the ceiling of its document and scrolls as before.
            limits[1] = limits[1].max(canvas_height);
            let fitted = fitted_card_height(measured, hand_resized, current[1], limits);
            let fitted_width = if hand_resized { current[0] } else { width };
            let wider = (fitted_width - current[0]).abs() > f32::EPSILON;
            if wider || (fitted - current[1]).abs() > f32::EPSILON {
                resized |= self.board.resize_panel(id, [fitted_width, fitted]);
            }
            // A card that took another width measures its content again: the
            // height of a document depends on the room that its body has.
            if let Some(card) = self.panel_render_caches.card_runtime.get_mut(&id)
                && !wider
            {
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
