use std::time::Instant;

use egui::{
    Align, Color32, Context, CornerRadius, Id, Layout, Margin, Order, Pos2, Rect, Sense, Stroke, StrokeKind, UiBuilder,
    Vec2,
};
use horizon_core::WorkspaceId;

use crate::theme;

const NAV_WIDTH: f32 = 440.0;
const INPUT_HEIGHT: f32 = 44.0;
const ROW_HEIGHT: f32 = 40.0;
const MAX_VISIBLE_ROWS: usize = 8;

pub struct WorkspaceEntry {
    pub id: WorkspaceId,
    pub name: String,
    pub color: Color32,
    pub panel_count: usize,
    pub is_active: bool,
}

pub struct QuickNav {
    query: String,
    selected: usize,
    opened_at: Instant,
}

pub enum QuickNavAction {
    None,
    Selected(WorkspaceId),
    Cancelled,
}

impl QuickNav {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            selected: 0,
            opened_at: Instant::now(),
        }
    }

    pub fn show(&mut self, ctx: &Context, workspaces: &[WorkspaceEntry]) -> QuickNavAction {
        let filtered = self.filtered_workspaces(workspaces);
        let mut action = QuickNavAction::None;

        // Backdrop
        let screen_rect = ctx.input(egui::InputState::viewport_rect);
        egui::Area::new(Id::new("quick_nav_backdrop"))
            .fixed_pos(screen_rect.min)
            .constrain(false)
            .order(Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(screen_rect.size(), Sense::click());
                ui.painter_at(rect)
                    .rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(140));
                if response.clicked() && self.opened_at.elapsed().as_millis() > 200 {
                    action = QuickNavAction::Cancelled;
                }
            });

        if matches!(action, QuickNavAction::Cancelled) {
            return action;
        }

        // Card dimensions — fixed height based on max rows.
        let results_height = usize_to_f32(MAX_VISIBLE_ROWS) * ROW_HEIGHT;
        let card_height = INPUT_HEIGHT + 16.0 + results_height + 24.0;
        let card_pos = Pos2::new(
            (screen_rect.width() - NAV_WIDTH) * 0.5,
            (screen_rect.height() - card_height) * 0.3,
        );

        egui::Area::new(Id::new("quick_nav_modal"))
            .fixed_pos(card_pos)
            .constrain(true)
            .order(Order::Debug)
            .show(ctx, |ui| {
                let card_rect = Rect::from_min_size(card_pos, Vec2::new(NAV_WIDTH, card_height));

                let painter = ui.painter();
                painter.rect_filled(card_rect, CornerRadius::same(20), theme::PANEL_BG);
                painter.rect_stroke(
                    card_rect,
                    CornerRadius::same(20),
                    Stroke::new(1.5, theme::alpha(theme::ACCENT, 80)),
                    StrokeKind::Outside,
                );
                painter.rect_stroke(
                    card_rect.expand(2.0),
                    CornerRadius::same(22),
                    Stroke::new(2.0, theme::alpha(theme::ACCENT, 25)),
                    StrokeKind::Outside,
                );

                let inner_rect = card_rect.shrink2(Vec2::new(20.0, 16.0));
                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(inner_rect)
                        .layout(Layout::top_down(Align::Min)),
                    |ui| {
                        // Title
                        ui.label(
                            egui::RichText::new("Go to workspace")
                                .color(theme::FG)
                                .size(15.0)
                                .strong(),
                        );
                        ui.add_space(10.0);

                        // Search input
                        let input_rect =
                            Rect::from_min_size(ui.cursor().min, Vec2::new(inner_rect.width(), INPUT_HEIGHT));
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
                                .font(egui::FontId::proportional(14.0))
                                .text_color(theme::FG)
                                .frame(false)
                                .desired_width(text_rect.width())
                                .hint_text(
                                    egui::RichText::new("Search workspaces...")
                                        .color(theme::FG_DIM)
                                        .size(13.0),
                                )
                                .margin(Margin::ZERO),
                        );
                        if !response.has_focus() && self.opened_at.elapsed().as_millis() < 100 {
                            response.request_focus();
                        }

                        // Keyboard
                        let (up, down, enter, escape) = ctx.input(|input| {
                            (
                                input.key_pressed(egui::Key::ArrowUp),
                                input.key_pressed(egui::Key::ArrowDown),
                                input.key_pressed(egui::Key::Enter),
                                input.key_pressed(egui::Key::Escape),
                            )
                        });

                        if escape {
                            action = QuickNavAction::Cancelled;
                        }
                        if up && self.selected > 0 {
                            self.selected -= 1;
                        }
                        if down && !filtered.is_empty() && self.selected < filtered.len() - 1 {
                            self.selected += 1;
                        }
                        if enter && !filtered.is_empty() && !matches!(action, QuickNavAction::Cancelled) {
                            action = QuickNavAction::Selected(filtered[self.selected].id);
                        }

                        // Advance past input
                        ui.allocate_space(Vec2::new(inner_rect.width(), INPUT_HEIGHT));
                        ui.add_space(8.0);

                        // Results
                        let mut clicked_idx: Option<usize> = None;
                        let scroll_height = results_height.min(inner_rect.max.y - ui.cursor().min.y - 8.0);
                        egui::ScrollArea::vertical()
                            .max_height(scroll_height)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_min_width(inner_rect.width());
                                if filtered.is_empty() {
                                    ui.add_space(16.0);
                                    ui.vertical_centered(|ui| {
                                        ui.label(
                                            egui::RichText::new("No matching workspaces")
                                                .color(theme::FG_DIM)
                                                .size(12.0),
                                        );
                                    });
                                } else {
                                    for (i, entry) in filtered.iter().enumerate().take(MAX_VISIBLE_ROWS) {
                                        if render_workspace_row(ui, inner_rect.width(), i, entry, self.selected == i) {
                                            clicked_idx = Some(i);
                                        }
                                    }
                                }
                            });

                        if let Some(i) = clicked_idx {
                            action = QuickNavAction::Selected(filtered[i].id);
                        }
                    },
                );
            });

        // Clamp selected to filtered range (query may have changed).
        if self.selected >= filtered.len().max(1) {
            self.selected = filtered.len().saturating_sub(1);
        }

        action
    }

    fn filtered_workspaces<'a>(&self, workspaces: &'a [WorkspaceEntry]) -> Vec<&'a WorkspaceEntry> {
        let query = self.query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return workspaces.iter().collect();
        }
        workspaces
            .iter()
            .filter(|ws| ws.name.to_ascii_lowercase().contains(&query))
            .collect()
    }
}

fn render_workspace_row(
    ui: &mut egui::Ui,
    width: f32,
    index: usize,
    entry: &WorkspaceEntry,
    is_selected: bool,
) -> bool {
    let row_rect = ui.allocate_space(Vec2::new(width, ROW_HEIGHT)).1;
    let mut clicked = false;

    if is_selected {
        ui.painter_at(row_rect).rect_filled(
            row_rect,
            CornerRadius::same(8),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT, theme::ACCENT, 0.28), 200),
        );
    } else {
        let hover = ui
            .interact(row_rect, ui.make_persistent_id(("qn_hover", index)), Sense::hover())
            .hovered();
        if hover {
            ui.painter_at(row_rect).rect_filled(
                row_rect,
                CornerRadius::same(8),
                theme::alpha(theme::PANEL_BG_ALT, 160),
            );
        }
    }

    let click = ui.interact(row_rect, ui.make_persistent_id(("qn_click", index)), Sense::click());
    if click.clicked() {
        clicked = true;
    }

    let text_y = row_rect.center().y;

    // Workspace color dot
    ui.painter_at(row_rect)
        .circle_filled(Pos2::new(row_rect.min.x + 16.0, text_y), 5.0, entry.color);

    // Workspace name
    ui.painter_at(row_rect).text(
        Pos2::new(row_rect.min.x + 32.0, text_y),
        egui::Align2::LEFT_CENTER,
        &entry.name,
        egui::FontId::proportional(14.0),
        if is_selected { theme::FG } else { theme::FG_SOFT },
    );

    // Panel count badge
    let count_text = format!("{} panels", entry.panel_count);
    ui.painter_at(row_rect).text(
        Pos2::new(row_rect.max.x - 12.0, text_y),
        egui::Align2::RIGHT_CENTER,
        &count_text,
        egui::FontId::proportional(11.0),
        theme::FG_DIM,
    );

    // Active indicator
    if entry.is_active {
        ui.painter_at(row_rect).text(
            Pos2::new(row_rect.max.x - 100.0, text_y),
            egui::Align2::RIGHT_CENTER,
            "active",
            egui::FontId::proportional(10.0),
            theme::alpha(theme::ACCENT, 180),
        );
    }

    clicked
}

fn usize_to_f32(v: usize) -> f32 {
    let clamped = u16::try_from(v).unwrap_or(u16::MAX);
    f32::from(clamped)
}
