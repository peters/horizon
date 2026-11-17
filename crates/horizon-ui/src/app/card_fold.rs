//! The fold of a card: one line that holds its heading and its badge.
//!
//! A card whose job is finished is folded. The person folds and unfolds any
//! card with the control of its title bar, and a press on a folded line opens it.

use egui::{Context, Rect, Sense, Stroke, Ui, Vec2, WidgetInfo};
use horizon_core::{BlockPanelState, CardPlace, Panel, PanelId};

use super::{HorizonApp, PANEL_TITLEBAR_HEIGHT};
use crate::{block_widget::default_card_size, theme};

impl HorizonApp {
    /// Shows the fold control of the card `id` in `rect`, an arrow. Returns whether
    /// the person pressed it, or pressed the folded line, which `on_title` says.
    pub(super) fn show_card_fold(&self, ui: &Ui, id: PanelId, rect: Rect, live: bool, on_title: bool) -> bool {
        let Some(folded) = self.board.panel(id).and_then(Panel::block).map(BlockPanelState::folded) else {
            return false;
        };
        let words = ["Fold the card to one line", "Unfold the card"][usize::from(folded)];
        let sense = if live { Sense::click() } else { Sense::hover() };
        let id = ui.make_persistent_id(("panel_fold", id.0));
        let response = ui.interact(rect.expand(4.0), id, sense).on_hover_text(words);
        response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, live, words));
        let down = [Vec2::new(-3.5, -1.75), Vec2::new(0.0, 1.75), Vec2::new(3.5, -1.75)];
        let points = down.map(|point| rect.center() + if folded { point.yx() } else { point });
        let colour = [theme::FG_DIM(), theme::FG()][usize::from(response.hovered())];
        ui.painter().line(points.to_vec(), Stroke::new(1.5, colour));
        response.clicked() || folded && on_title
    }

    /// Gives the card `id` the height of its fold when the fold changed, after the
    /// press of the person that `pressed` says.
    pub(super) fn settle_card_fold(&mut self, ctx: &Context, id: PanelId, pressed: bool) {
        let Some(panel) = self.board.panel_mut(id) else {
            return;
        };
        let size = panel.layout.size;
        let Some(card) = panel.block_mut() else {
            return;
        };
        if pressed {
            card.fold = Some(!card.folded());
        }
        let folded = card.folded();
        if folded == (size[1] <= PANEL_TITLEBAR_HEIGHT) {
            return;
        }
        // No tile of a preset is one line high, so a card that folds leaves the tiles.
        if card.place == CardPlace::Tile {
            card.place = CardPlace::Free;
        }
        let open = default_card_size(&card.document, 0.0)[1];
        self.board
            .resize_panel(id, [size[0], if folded { PANEL_TITLEBAR_HEIGHT } else { open }]);
        // The body of a card that opens measures its content before the next fit.
        if let Some(runtime) = self.panel_render_caches.card_runtime.get_mut(&id) {
            runtime.forget_content_fit();
        }
        self.mark_runtime_dirty();
        ctx.request_repaint();
    }
}
