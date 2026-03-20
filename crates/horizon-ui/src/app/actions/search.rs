use crate::app::HorizonApp;
use crate::search_overlay::SearchAction;
use crate::search_overlay::SearchOverlay;

impl HorizonApp {
    /// Render the inline search input in the toolbar. When active, a
    /// dropdown with results appears below the input field.
    pub(in crate::app) fn render_toolbar_search(&mut self, ui: &mut egui::Ui) {
        let overlay = self.search_overlay.get_or_insert_with(SearchOverlay::new_inactive);

        let action = overlay.show_toolbar_input(ui, &self.board);
        match action {
            SearchAction::None => {}
            SearchAction::FocusPanel(panel_id) => {
                if let Some(overlay) = &mut self.search_overlay {
                    overlay.clear();
                }
                self.board.focus(panel_id);
                if let Some(workspace_id) = self.board.panel(panel_id).map(|panel| panel.workspace_id)
                    && let Some((min, max)) = self.board.workspace_bounds(workspace_id)
                {
                    self.focus_workspace_bounds(ui.ctx(), min, max, true);
                }
            }
        }
    }
}
