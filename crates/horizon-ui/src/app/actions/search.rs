use egui::{Pos2, Vec2};

use crate::app::{HorizonApp, WS_BG_PAD, WS_TITLE_HEIGHT};
use crate::search_overlay::{SearchAction, SearchOverlay};

impl HorizonApp {
    /// Render the inline search input in the toolbar. When active, a
    /// dropdown with results appears below the input field.
    pub(in crate::app) fn render_toolbar_search(&mut self, ui: &mut egui::Ui) {
        let overlay = self.search_overlay.get_or_insert_with(SearchOverlay::new_inactive);

        let action = overlay.show_toolbar_input(ui, &self.board);
        match action {
            SearchAction::None => {}
            SearchAction::Cancelled => {
                self.search_overlay = None;
            }
            SearchAction::FocusPanel(panel_id) => {
                self.search_overlay = None;
                self.board.focus(panel_id);
                if let Some(workspace_id) = self.board.panel(panel_id).map(|panel| panel.workspace_id)
                    && let Some((min, max)) = self.board.workspace_bounds(workspace_id)
                {
                    let pos = Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT);
                    let size = Vec2::new(
                        max[0] - min[0] + 2.0 * WS_BG_PAD,
                        max[1] - min[1] + 2.0 * WS_BG_PAD + WS_TITLE_HEIGHT,
                    );
                    self.pan_to_canvas_pos_aligned(ui.ctx(), pos, size, true);
                }
            }
        }
    }
}
