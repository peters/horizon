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
            SearchAction::FocusPanel {
                panel_id,
                line_index,
                total_lines,
            } => {
                if let Some(overlay) = &mut self.search_overlay {
                    overlay.clear();
                }
                self.board.focus(panel_id);
                scroll_to_search_match(&mut self.board, panel_id, line_index, total_lines);
                if let Some(workspace_id) = self.board.panel(panel_id).map(|panel| panel.workspace_id)
                    && let Some((min, max)) = self.board.workspace_bounds(workspace_id)
                {
                    self.focus_workspace_bounds(ui.ctx(), min, max, true);
                }
            }
        }
    }
}

/// Scroll a panel's terminal so the matched line is visible, roughly centered.
fn scroll_to_search_match(
    board: &mut horizon_core::Board,
    panel_id: horizon_core::PanelId,
    line_index: usize,
    snapshot_total: usize,
) {
    let Some(panel) = board.panel_mut(panel_id) else {
        return;
    };
    let Some(terminal) = panel.terminal() else {
        return;
    };
    let rows = usize::from(terminal.rows());
    let lines_from_bottom = snapshot_total.saturating_sub(1).saturating_sub(line_index);
    if lines_from_bottom < rows {
        panel.set_scrollback(0);
        return;
    }
    let scrollback = lines_from_bottom.saturating_sub(rows / 2);
    panel.set_scrollback(scrollback);
}
