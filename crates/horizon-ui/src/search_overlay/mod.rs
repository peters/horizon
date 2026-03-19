mod render;

use std::time::Instant;

use egui::{
    Align, Color32, Context, CornerRadius, Id, Layout, Margin, Order, Rect, Sense, Stroke, StrokeKind, UiBuilder, Vec2,
};
use horizon_core::{Board, PanelId, SearchOptions, SearchResults, search_board};

use crate::theme;

use render::{
    MatchRowData, SearchLayout, paint_card, paint_empty_results, render_match_row, render_section_header,
    render_status_line, render_toggle_button, search_layout,
};

const SEARCH_WIDTH: f32 = 600.0;
const INPUT_HEIGHT: f32 = 44.0;
const ROW_HEIGHT: f32 = 36.0;
const SECTION_HEADER_HEIGHT: f32 = 28.0;
const MAX_VISIBLE_ROWS: usize = 14;

const LABEL_FONT: egui::FontId = egui::FontId::new(13.0, egui::FontFamily::Proportional);
const DETAIL_FONT: egui::FontId = egui::FontId::new(11.0, egui::FontFamily::Monospace);
const BADGE_FONT: egui::FontId = egui::FontId::new(10.0, egui::FontFamily::Monospace);

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
    opened_at: Instant,
    cached_results: SearchResults,
    display_rows: Vec<DisplayRow>,
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
            opened_at: Instant::now(),
            cached_results: SearchResults::default(),
            display_rows: Vec::new(),
        }
    }

    pub(crate) fn show(&mut self, ctx: &Context, board: &Board) -> SearchAction {
        self.maybe_refresh_results(board);
        let layout = search_layout(ctx.input(egui::InputState::viewport_rect));

        if self.show_backdrop(ctx, layout.screen) {
            return SearchAction::Cancelled;
        }

        self.clamp_selection();
        self.show_modal(ctx, &layout)
    }

    fn maybe_refresh_results(&mut self, board: &Board) {
        let options_changed = self.query != self.last_query;
        if !options_changed {
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

            // Show first match per panel as the primary row, with the panel
            // title visible. Group additional matches underneath without
            // repeating the title, to keep the list scannable.
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

    fn show_backdrop(&self, ctx: &Context, screen_rect: Rect) -> bool {
        let mut cancelled = false;
        egui::Area::new(Id::new("search_backdrop"))
            .fixed_pos(screen_rect.min)
            .constrain(false)
            .order(Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(screen_rect.size(), Sense::click());
                ui.painter_at(rect)
                    .rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(140));
                if response.clicked() && self.opened_at.elapsed().as_millis() > 200 {
                    cancelled = true;
                }
            });
        cancelled
    }

    fn show_modal(&mut self, ctx: &Context, layout: &SearchLayout) -> SearchAction {
        let mut action = SearchAction::None;

        egui::Area::new(Id::new("search_modal"))
            .fixed_pos(layout.card.min)
            .constrain(true)
            .order(Order::Debug)
            .show(ctx, |ui| {
                paint_card(ui, layout.card);

                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(layout.inner)
                        .layout(Layout::top_down(Align::Min)),
                    |ui| {
                        action = self.show_contents(ui, ctx, layout);
                    },
                );
            });

        action
    }

    fn show_contents(&mut self, ui: &mut egui::Ui, ctx: &Context, layout: &SearchLayout) -> SearchAction {
        ui.label(
            egui::RichText::new("Search Terminals")
                .color(theme::FG)
                .size(15.0)
                .strong(),
        );
        ui.add_space(10.0);

        self.render_query_input(ui, layout.inner);
        if let Some(action) = self.handle_keyboard(ctx) {
            return action;
        }

        ui.allocate_space(Vec2::new(layout.inner.width(), INPUT_HEIGHT));
        ui.add_space(4.0);

        self.render_options_row(ui, layout.inner.width());
        ui.add_space(4.0);

        render_status_line(ui, self.cached_results.total_matches, self.cached_results.panels.len());
        ui.add_space(4.0);

        match self.render_results(ui, layout) {
            Some(index) => SearchAction::FocusPanel(self.display_rows[index].panel_id),
            None => SearchAction::None,
        }
    }

    fn render_query_input(&mut self, ui: &mut egui::Ui, inner_rect: Rect) {
        let input_rect = Rect::from_min_size(ui.cursor().min, Vec2::new(inner_rect.width(), INPUT_HEIGHT));
        ui.painter()
            .rect_filled(input_rect, CornerRadius::same(12), theme::BG_ELEVATED);
        ui.painter().rect_stroke(
            input_rect,
            CornerRadius::same(12),
            Stroke::new(1.0, theme::alpha(theme::ACCENT, 70)),
            StrokeKind::Inside,
        );

        let text_rect = input_rect.shrink2(Vec2::new(14.0, 6.0));
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(text_rect)
                .layout(Layout::left_to_right(Align::Center)),
        );

        let response = child.add(
            egui::TextEdit::singleline(&mut self.query)
                .font(egui::FontId::monospace(14.0))
                .text_color(theme::FG)
                .frame(false)
                .desired_width(text_rect.width())
                .hint_text(
                    egui::RichText::new("Search across all terminals...")
                        .color(theme::FG_DIM)
                        .size(13.0),
                )
                .margin(Margin::ZERO),
        );
        if !response.has_focus() && self.opened_at.elapsed().as_millis() < 100 {
            response.request_focus();
        }
        if response.changed() {
            self.selected = 0;
        }
    }

    fn render_options_row(&mut self, ui: &mut egui::Ui, _width: f32) {
        ui.horizontal(|ui| {
            if render_toggle_button(ui, "Aa", self.case_sensitive, "case") {
                self.case_sensitive = !self.case_sensitive;
                // Force a re-query on next frame by invalidating the cache.
                self.last_query = String::from("\x00_invalidate");
            }
            ui.add_space(4.0);
            if render_toggle_button(ui, ".*", self.regex_mode, "regex") {
                self.regex_mode = !self.regex_mode;
                self.last_query = String::from("\x00_invalidate");
            }
        });
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

    fn render_results(&mut self, ui: &mut egui::Ui, layout: &SearchLayout) -> Option<usize> {
        let mut clicked_idx = None;
        let scroll_height = layout.results_height.min(layout.inner.max.y - ui.cursor().min.y - 8.0);

        egui::ScrollArea::vertical()
            .max_height(scroll_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(layout.inner.width());

                if self.query.is_empty() {
                    paint_empty_results(ui, "Type to search across all terminal panels");
                    return;
                }

                if self.display_rows.is_empty() {
                    paint_empty_results(ui, "No matches found");
                    return;
                }

                let mut current_panel: Option<PanelId> = None;
                for (i, row) in self.display_rows.iter().enumerate() {
                    if current_panel != Some(row.panel_id) && !row.panel_title.is_empty() {
                        current_panel = Some(row.panel_id);
                        render_section_header(ui, layout.inner.width(), &row.panel_title);
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

                    if render_match_row(ui, layout.inner.width(), i, &data, self.selected == i) {
                        clicked_idx = Some(i);
                    }
                }
            });

        clicked_idx
    }
}
