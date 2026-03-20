mod render;

use std::time::{Duration, Instant};

use egui::{Align, Context, CornerRadius, Id, Layout, Margin, Order, Pos2, Rect, Stroke, StrokeKind, UiBuilder, Vec2};
use horizon_core::{Board, PanelId, SearchOptions, SearchResults, search_board};

use crate::theme;

use render::{
    MatchRowData, paint_dropdown_frame, paint_empty_results, render_match_row, render_section_header,
    render_status_line, render_toggle_button,
};

const DROPDOWN_WIDTH: f32 = 600.0;
const ROW_HEIGHT: f32 = 32.0;
const SECTION_HEADER_HEIGHT: f32 = 24.0;
const MAX_VISIBLE_ROWS: usize = 12;

const LABEL_FONT: egui::FontId = egui::FontId::new(12.0, egui::FontFamily::Proportional);
const DETAIL_FONT: egui::FontId = egui::FontId::new(10.5, egui::FontFamily::Monospace);
const BADGE_FONT: egui::FontId = egui::FontId::new(9.5, egui::FontFamily::Monospace);

/// Flattened result row for display.
struct DisplayRow {
    panel_id: PanelId,
    panel_title: String,
    line_text: String,
    match_count_label: Option<String>,
    /// Zero-based line index of the match within the extracted text snapshot.
    line_index: usize,
    /// Total grid lines at snapshot time (before trailing-empty-line trimming).
    total_lines: usize,
}

#[allow(clippy::struct_excessive_bools)]
pub(crate) struct SearchOverlay {
    query: String,
    last_query: String,
    case_sensitive: bool,
    regex_mode: bool,
    selected: usize,
    cached_results: SearchResults,
    display_rows: Vec<DisplayRow>,
    request_focus: bool,
    options_dirty: bool,
    /// When the query last changed; search fires after the debounce window.
    query_changed_at: Option<Instant>,
}

pub(crate) enum SearchAction {
    None,
    FocusPanel {
        panel_id: PanelId,
        line_index: usize,
        total_lines: usize,
    },
}

impl SearchOverlay {
    pub(crate) fn new() -> Self {
        Self {
            query: String::new(),
            last_query: String::new(),
            case_sensitive: false,
            regex_mode: false,
            selected: 0,
            cached_results: SearchResults::default(),
            display_rows: Vec::new(),
            request_focus: true,
            options_dirty: false,
            query_changed_at: None,
        }
    }

    /// Request focus on the search input next frame.
    pub(crate) fn focus(&mut self) {
        self.request_focus = true;
    }

    /// Clear the query and results.
    pub(crate) fn clear(&mut self) {
        self.query.clear();
        self.last_query.clear();
        self.cached_results = SearchResults::default();
        self.display_rows.clear();
        self.selected = 0;
        self.query_changed_at = None;
    }

    /// Create a search overlay without auto-focusing the input. Used for
    /// the always-present toolbar search bar.
    pub(crate) fn new_inactive() -> Self {
        Self {
            request_focus: false,
            ..Self::new()
        }
    }

    /// Render the search input inline in the toolbar. Returns an action
    /// if the user selects a result or cancels.
    pub(crate) fn show_toolbar_input(&mut self, ui: &mut egui::Ui, board: &Board) -> SearchAction {
        self.maybe_refresh_results(ui.ctx(), board);

        let input_width = ui.available_width();

        let response = ui.add_sized(
            Vec2::new(input_width, 32.0),
            egui::TextEdit::singleline(&mut self.query)
                .font(egui::FontId::monospace(13.0))
                .text_color(theme::FG)
                .hint_text(
                    egui::RichText::new("Search across all terminals...")
                        .color(theme::FG_DIM)
                        .size(12.5),
                )
                .margin(Margin::symmetric(12, 0)),
        );

        // Paint the accent border on top of the default frame so it's
        // always visible regardless of focus state.
        let input_rect = response.rect;
        ui.painter().rect_stroke(
            input_rect,
            CornerRadius::same(6),
            Stroke::new(1.5, theme::alpha(theme::ACCENT, 130)),
            StrokeKind::Inside,
        );

        if self.request_focus {
            response.request_focus();
            self.request_focus = false;
        }

        if response.changed() {
            self.note_query_changed(Instant::now());
            ui.ctx().request_repaint_after(Self::DEBOUNCE);
        }

        // Handle keyboard while the input has focus.
        if response.has_focus()
            && let Some(action) = self.handle_keyboard(ui.ctx())
        {
            return action;
        }

        // Keep the dropdown interactive while the pointer is over it so
        // result-row and toggle clicks are not dropped when the text field
        // loses focus on mouse down.
        let dropdown_rect = self.dropdown_rect(input_rect);
        if self.should_show_dropdown(ui.ctx(), response.has_focus(), dropdown_rect) {
            self.show_results_dropdown(ui.ctx(), dropdown_rect)
        } else {
            SearchAction::None
        }
    }

    fn show_results_dropdown(&mut self, ctx: &Context, dropdown_rect: Rect) -> SearchAction {
        self.clamp_selection();

        let mut action = SearchAction::None;

        egui::Area::new(Id::new("search_dropdown"))
            .fixed_pos(dropdown_rect.min)
            .constrain(true)
            .order(Order::Tooltip)
            .show(ctx, |ui| {
                paint_dropdown_frame(ui, dropdown_rect);

                let inner = dropdown_rect.shrink2(Vec2::new(12.0, 10.0));
                ui.scope_builder(
                    UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)),
                    |ui| {
                        ui.horizontal(|ui| {
                            if render_toggle_button(ui, "Aa", self.case_sensitive, "case") {
                                self.case_sensitive = !self.case_sensitive;
                                self.options_dirty = true;
                            }
                            ui.add_space(2.0);
                            if render_toggle_button(ui, ".*", self.regex_mode, "regex") {
                                self.regex_mode = !self.regex_mode;
                                self.options_dirty = true;
                            }
                            ui.add_space(8.0);
                            render_status_line(ui, self.cached_results.total_matches, self.cached_results.panels.len());
                        });
                        ui.add_space(4.0);

                        if let Some(idx) = self.render_results(ui, inner.width()) {
                            let row = &self.display_rows[idx];
                            action = SearchAction::FocusPanel {
                                panel_id: row.panel_id,
                                line_index: row.line_index,
                                total_lines: row.total_lines,
                            };
                        }
                    },
                );
            });

        action
    }

    fn dropdown_rect(&self, anchor_rect: Rect) -> Rect {
        let dropdown_x = (anchor_rect.max.x - DROPDOWN_WIDTH).max(anchor_rect.min.x);
        let dropdown_top = anchor_rect.max.y + 6.0;
        let max_results_height = self.dropdown_content_height();
        let dropdown_height = max_results_height + 56.0;

        Rect::from_min_size(
            Pos2::new(dropdown_x, dropdown_top),
            Vec2::new(DROPDOWN_WIDTH, dropdown_height),
        )
    }

    fn should_show_dropdown(&self, ctx: &Context, input_has_focus: bool, dropdown_rect: Rect) -> bool {
        !self.query.is_empty()
            && (input_has_focus
                || ctx.pointer_hover_pos().is_some_and(|pos| dropdown_rect.contains(pos))
                || ctx
                    .pointer_interact_pos()
                    .is_some_and(|pos| dropdown_rect.contains(pos)))
    }

    const DEBOUNCE: Duration = Duration::from_millis(150);

    fn maybe_refresh_results(&mut self, ctx: &Context, board: &Board) {
        let terminal_output_changed = self.should_refresh_for_terminal_output(board);
        self.maybe_refresh_results_at(ctx, board, terminal_output_changed, Instant::now());
    }

    fn should_refresh_for_terminal_output(&self, board: &Board) -> bool {
        !self.query.is_empty() && board.panels.iter().any(horizon_core::Panel::had_recent_output)
    }

    fn note_query_changed(&mut self, now: Instant) {
        self.selected = 0;
        self.query_changed_at = Some(now);
    }

    fn maybe_refresh_results_at(&mut self, ctx: &Context, board: &Board, terminal_output_changed: bool, now: Instant) {
        let query_changed = self.query != self.last_query;
        let options_changed = self.options_dirty;
        let query_or_options_changed = query_changed || options_changed;

        if !query_changed && !options_changed && !terminal_output_changed {
            self.query_changed_at = None;
            return;
        }

        if options_changed {
            // Option toggles fire immediately (no debounce needed).
            self.options_dirty = false;
            self.query_changed_at = None;
        } else if query_changed {
            // Query text changed -- debounce to avoid searching on every
            // keystroke when many terminals are open.
            let changed_at = *self.query_changed_at.get_or_insert(now);
            let remaining = Self::DEBOUNCE.saturating_sub(now.duration_since(changed_at));
            if !remaining.is_zero() {
                ctx.request_repaint_after(remaining);
                return;
            }
            self.query_changed_at = None;
        }

        self.last_query.clone_from(&self.query);
        let options = SearchOptions {
            case_sensitive: self.case_sensitive,
            regex: self.regex_mode,
        };
        self.cached_results = search_board(board, &self.query, &options);
        self.rebuild_display_rows();
        if query_or_options_changed {
            self.selected = 0;
        } else {
            self.clamp_selection();
        }
    }

    fn rebuild_display_rows(&mut self) {
        self.display_rows.clear();
        for panel_result in &self.cached_results.panels {
            let count_label = if panel_result.matches.len() > 1 {
                Some(format!("{}", panel_result.matches.len()))
            } else {
                None
            };

            for (i, m) in panel_result.matches.iter().enumerate() {
                let line_text = panel_result.lines.get(m.line_index).cloned().unwrap_or_default();
                self.display_rows.push(DisplayRow {
                    panel_id: panel_result.panel_id,
                    panel_title: if i == 0 {
                        panel_result.panel_title.clone()
                    } else {
                        String::new()
                    },
                    line_text,
                    match_count_label: if i == 0 { count_label.clone() } else { None },
                    line_index: m.line_index,
                    total_lines: panel_result.total_lines,
                });
            }
        }
    }

    fn clamp_selection(&mut self) {
        if self.display_rows.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.display_rows.len() {
            self.selected = self.display_rows.len() - 1;
        }
    }

    fn dropdown_content_height(&self) -> f32 {
        use crate::app::util::usize_to_f32;
        let visible = self.display_rows.len().min(MAX_VISIBLE_ROWS);
        usize_to_f32(visible) * ROW_HEIGHT + 2.0 * SECTION_HEADER_HEIGHT + 8.0
    }

    fn handle_keyboard(&mut self, ctx: &Context) -> Option<SearchAction> {
        let (up, down, enter, escape) = ctx.input(|input| {
            (
                input.key_pressed(egui::Key::ArrowUp),
                input.key_pressed(egui::Key::ArrowDown),
                input.key_pressed(egui::Key::Enter),
                input.key_pressed(egui::Key::Escape),
            )
        });

        if escape {
            self.clear();
            return Some(SearchAction::None);
        }
        if up && self.selected > 0 {
            self.selected -= 1;
        }
        if down && !self.display_rows.is_empty() && self.selected < self.display_rows.len() - 1 {
            self.selected += 1;
        }
        if enter && !self.display_rows.is_empty() {
            let row = &self.display_rows[self.selected];
            return Some(SearchAction::FocusPanel {
                panel_id: row.panel_id,
                line_index: row.line_index,
                total_lines: row.total_lines,
            });
        }

        None
    }

    fn render_results(&mut self, ui: &mut egui::Ui, width: f32) -> Option<usize> {
        let mut clicked_idx = None;
        let max_height = self.dropdown_content_height();

        egui::ScrollArea::vertical()
            .max_height(max_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(width);

                if self.display_rows.is_empty() {
                    paint_empty_results(ui, "No matches found");
                    return;
                }

                let mut current_panel: Option<PanelId> = None;
                for (i, row) in self.display_rows.iter().enumerate() {
                    if current_panel != Some(row.panel_id) && !row.panel_title.is_empty() {
                        current_panel = Some(row.panel_id);
                        render_section_header(ui, width, &row.panel_title);
                    }

                    let data = MatchRowData {
                        panel_title: if row.panel_title.is_empty() {
                            ""
                        } else {
                            &row.panel_title
                        },
                        line_text: &row.line_text,
                        match_count_label: row.match_count_label.as_deref(),
                    };

                    if render_match_row(ui, width, i, &data, self.selected == i) {
                        clicked_idx = Some(i);
                    }
                }
            });

        clicked_idx
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use egui::{Context, Pos2, Rect, Vec2};
    use horizon_core::{Board, PanelId, PanelSearchResult, SearchMatch, SearchResults};

    use super::{DisplayRow, SearchOverlay};

    #[test]
    fn note_query_changed_resets_debounce_anchor() {
        let mut overlay = SearchOverlay::new();
        let first_change = Instant::now();
        let second_change = first_change + Duration::from_millis(100);

        overlay.note_query_changed(first_change);
        assert_eq!(overlay.query_changed_at, Some(first_change));

        overlay.note_query_changed(second_change);
        assert_eq!(overlay.query_changed_at, Some(second_change));
    }

    #[test]
    fn clear_resets_pending_debounce() {
        let mut overlay = SearchOverlay::new();
        overlay.note_query_changed(Instant::now());

        overlay.clear();

        assert!(overlay.query_changed_at.is_none());
        assert!(overlay.query.is_empty());
        assert!(overlay.last_query.is_empty());
    }

    #[test]
    fn dropdown_stays_open_while_pointer_is_over_results() {
        let overlay = SearchOverlay {
            query: "workspace-beta-only".to_string(),
            ..SearchOverlay::new()
        };
        let ctx = Context::default();
        let dropdown_rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(120.0, 80.0));

        let pointer = dropdown_rect.center();
        let mut input = egui::RawInput {
            events: vec![egui::Event::PointerMoved(pointer)],
            ..egui::RawInput::default()
        };
        input.viewport_id = egui::ViewportId::ROOT;
        ctx.begin_pass(input);
        let show_dropdown = overlay.should_show_dropdown(&ctx, false, dropdown_rect);
        let _ = ctx.end_pass();

        assert!(show_dropdown);
    }

    #[test]
    fn dropdown_hides_after_outside_click_when_input_loses_focus() {
        let overlay = SearchOverlay {
            query: "workspace-beta-only".to_string(),
            ..SearchOverlay::new()
        };
        let ctx = Context::default();
        let dropdown_rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(120.0, 80.0));

        let outside = Pos2::new(dropdown_rect.max.x + 20.0, dropdown_rect.max.y + 20.0);
        let mut input = egui::RawInput {
            events: vec![egui::Event::PointerMoved(outside)],
            ..egui::RawInput::default()
        };
        input.viewport_id = egui::ViewportId::ROOT;
        ctx.begin_pass(input);
        let show_dropdown = overlay.should_show_dropdown(&ctx, false, dropdown_rect);
        let _ = ctx.end_pass();

        assert!(!show_dropdown);
    }

    #[test]
    fn terminal_output_refreshes_cached_results_without_query_change() {
        let mut overlay = SearchOverlay::new_inactive();
        overlay.query = "error".to_string();
        overlay.last_query = overlay.query.clone();
        overlay.selected = 1;
        overlay.cached_results = SearchResults {
            panels: vec![PanelSearchResult {
                panel_id: PanelId(7),
                panel_title: "build".to_string(),
                lines: vec!["error: first failure".to_string()],
                matches: vec![SearchMatch {
                    line_index: 0,
                    byte_offset: 0,
                    byte_len: 5,
                }],
                total_lines: 1,
            }],
            total_matches: 1,
        };
        overlay.display_rows = vec![DisplayRow {
            panel_id: PanelId(7),
            panel_title: "build".to_string(),
            line_text: "error: first failure".to_string(),
            match_count_label: None,
            line_index: 0,
            total_lines: 1,
        }];

        overlay.maybe_refresh_results_at(&Context::default(), &Board::new(), true, Instant::now());

        assert_eq!(overlay.cached_results.total_matches, 0);
        assert!(overlay.cached_results.panels.is_empty());
        assert!(overlay.display_rows.is_empty());
    }

    #[test]
    fn debounce_fires_after_window_expires() {
        let mut overlay = SearchOverlay::new_inactive();
        let ctx = Context::default();
        let board = Board::new();
        let edit_time = Instant::now();

        overlay.query = "error".to_string();
        overlay.note_query_changed(edit_time);

        // Before debounce expires: should not search.
        overlay.maybe_refresh_results_at(&ctx, &board, false, edit_time + Duration::from_millis(50));
        assert!(overlay.last_query.is_empty());

        // After debounce expires: should search.
        overlay.maybe_refresh_results_at(&ctx, &board, false, edit_time + SearchOverlay::DEBOUNCE);
        assert_eq!(overlay.last_query, overlay.query);
    }
}
