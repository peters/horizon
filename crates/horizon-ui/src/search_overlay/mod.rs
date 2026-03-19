mod render;

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
}

pub(crate) struct SearchOverlay {
    query: String,
    last_query: String,
    case_sensitive: bool,
    regex_mode: bool,
    selected: usize,
    cached_results: SearchResults,
    display_rows: Vec<DisplayRow>,
    request_focus: bool,
}

pub(crate) enum SearchAction {
    None,
    Cancelled,
    FocusPanel(PanelId),
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
        }
    }

    /// Request focus on the search input next frame.
    pub(crate) fn focus(&mut self) {
        self.request_focus = true;
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
        self.maybe_refresh_results(board);

        let input_width = ui.available_width();

        // Paint a visible accent border so the search bar is always discoverable.
        let input_rect = Rect::from_min_size(ui.cursor().min, Vec2::new(input_width, 32.0));
        ui.painter().rect_filled(
            input_rect,
            CornerRadius::same(10),
            theme::alpha(theme::BG_ELEVATED, 220),
        );
        ui.painter().rect_stroke(
            input_rect,
            CornerRadius::same(10),
            Stroke::new(1.5, theme::alpha(theme::ACCENT, 130)),
            StrokeKind::Inside,
        );

        let response = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .font(egui::FontId::monospace(13.0))
                .text_color(theme::FG)
                .desired_width(input_width - 28.0)
                .hint_text(
                    egui::RichText::new("Search across all terminals...")
                        .color(theme::FG_DIM)
                        .size(12.5),
                )
                .margin(Margin::symmetric(14, 6)),
        );

        let input_rect = response.rect;

        if self.request_focus {
            response.request_focus();
            self.request_focus = false;
        }

        if response.changed() {
            self.selected = 0;
        }

        // Handle keyboard while the input has focus.
        let action = if response.has_focus() {
            self.handle_keyboard(ui.ctx())
        } else if self.query.is_empty() {
            Some(SearchAction::Cancelled)
        } else {
            None
        };

        if let Some(action) = action {
            return action;
        }

        // Show dropdown results below the input if we have a query.
        if self.query.is_empty() {
            SearchAction::None
        } else {
            self.show_results_dropdown(ui.ctx(), input_rect)
        }
    }

    fn show_results_dropdown(&mut self, ctx: &Context, anchor_rect: Rect) -> SearchAction {
        self.clamp_selection();

        let dropdown_x = (anchor_rect.max.x - DROPDOWN_WIDTH).max(anchor_rect.min.x);
        let dropdown_top = anchor_rect.max.y + 6.0;
        let max_results_height = self.dropdown_content_height();
        let dropdown_height = max_results_height + 56.0;

        let dropdown_rect = Rect::from_min_size(
            Pos2::new(dropdown_x, dropdown_top),
            Vec2::new(DROPDOWN_WIDTH, dropdown_height),
        );

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
                                self.last_query = String::from("\x00_invalidate");
                            }
                            ui.add_space(2.0);
                            if render_toggle_button(ui, ".*", self.regex_mode, "regex") {
                                self.regex_mode = !self.regex_mode;
                                self.last_query = String::from("\x00_invalidate");
                            }
                            ui.add_space(8.0);
                            render_status_line(ui, self.cached_results.total_matches, self.cached_results.panels.len());
                        });
                        ui.add_space(4.0);

                        if let Some(idx) = self.render_results(ui, inner.width()) {
                            action = SearchAction::FocusPanel(self.display_rows[idx].panel_id);
                        }
                    },
                );
            });

        action
    }

    fn maybe_refresh_results(&mut self, board: &Board) {
        if self.query == self.last_query {
            return;
        }

        self.last_query.clone_from(&self.query);
        let options = SearchOptions {
            case_sensitive: self.case_sensitive,
            regex: self.regex_mode,
        };
        self.cached_results = search_board(board, &self.query, &options);
        self.rebuild_display_rows();
        self.selected = 0;
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
                self.display_rows.push(DisplayRow {
                    panel_id: panel_result.panel_id,
                    panel_title: if i == 0 {
                        panel_result.panel_title.clone()
                    } else {
                        String::new()
                    },
                    line_text: m.line_text.clone(),
                    match_count_label: if i == 0 { count_label.clone() } else { None },
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
            return Some(SearchAction::Cancelled);
        }
        if up && self.selected > 0 {
            self.selected -= 1;
        }
        if down && !self.display_rows.is_empty() && self.selected < self.display_rows.len() - 1 {
            self.selected += 1;
        }
        if enter && !self.display_rows.is_empty() {
            return Some(SearchAction::FocusPanel(self.display_rows[self.selected].panel_id));
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
                        match_count_label: row.match_count_label.clone(),
                    };

                    if render_match_row(ui, width, i, &data, self.selected == i) {
                        clicked_idx = Some(i);
                    }
                }
            });

        clicked_idx
    }
}
