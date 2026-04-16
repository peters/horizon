use std::time::Instant;

use egui::{
    Align, Button, Color32, Context, CornerRadius, Id, Layout, Margin, Order, Pos2, Rect, Sense, Stroke, StrokeKind,
    UiBuilder, Vec2,
};

use crate::theme;

const PICKER_WIDTH: f32 = 520.0;
const PICKER_MAX_HEIGHT: f32 = 460.0;
const INPUT_HEIGHT: f32 = 44.0;
const ROW_HEIGHT: f32 = 34.0;
const MAX_VISIBLE_ROWS: usize = 12;
const MAX_RENDERED_ROWS: usize = 36;

pub struct PickerModalState {
    query: String,
    selected: usize,
    opened_at: Instant,
}

pub enum PickerModalAction {
    None,
    Cancelled,
    Submit,
    CompleteSelection,
    ClickedRow(usize),
    FooterAction,
}

pub struct PickerEmptyState<'a> {
    pub message: &'a str,
    pub color: Color32,
}

pub struct PickerModalConfig<'a> {
    pub id_source: &'a str,
    pub heading: &'a str,
    pub hint_text: &'a str,
    pub status_text: Option<&'a str>,
    pub empty_state: PickerEmptyState<'a>,
    pub footer_action_label: Option<&'a str>,
}

struct PickerLayout {
    screen: Rect,
    card: Rect,
    inner: Rect,
    footer_height: f32,
}

impl PickerModalState {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            selected: 0,
            opened_at: Instant::now(),
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn clamp_selected(&mut self, len: usize) {
        if self.selected >= len {
            self.selected = 0;
        }
    }

    pub fn show<Item>(
        &mut self,
        ctx: &Context,
        config: &PickerModalConfig<'_>,
        results: &[Item],
        mut render_row: impl FnMut(&mut egui::Ui, f32, usize, &Item, bool) -> bool,
    ) -> PickerModalAction {
        let layout = picker_layout(ctx.input(egui::InputState::viewport_rect));
        if self.show_backdrop(ctx, layout.screen, config.id_source) {
            return PickerModalAction::Cancelled;
        }

        self.show_modal(ctx, &layout, config, results, &mut render_row)
    }

    fn show_backdrop(&self, ctx: &Context, screen_rect: Rect, id_source: &str) -> bool {
        let mut cancelled = false;
        egui::Area::new(Id::new((id_source, "backdrop")))
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

    fn show_modal<Item>(
        &mut self,
        ctx: &Context,
        layout: &PickerLayout,
        config: &PickerModalConfig<'_>,
        results: &[Item],
        render_row: &mut impl FnMut(&mut egui::Ui, f32, usize, &Item, bool) -> bool,
    ) -> PickerModalAction {
        let mut action = PickerModalAction::None;
        egui::Area::new(Id::new((config.id_source, "modal")))
            .fixed_pos(layout.card.min)
            .constrain(true)
            .order(Order::Debug)
            .show(ctx, |ui| {
                paint_picker_card(ui, layout.card);
                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(layout.inner)
                        .layout(Layout::top_down(Align::Min)),
                    |ui| {
                        action = self.show_modal_contents(ui, ctx, layout, config, results, render_row);
                    },
                );
            });

        action
    }

    fn show_modal_contents<Item>(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &Context,
        layout: &PickerLayout,
        config: &PickerModalConfig<'_>,
        results: &[Item],
        render_row: &mut impl FnMut(&mut egui::Ui, f32, usize, &Item, bool) -> bool,
    ) -> PickerModalAction {
        ui.label(
            egui::RichText::new(config.heading)
                .color(theme::FG())
                .size(15.0)
                .strong(),
        );
        ui.add_space(10.0);

        self.render_query_input(ui, layout.inner, config.hint_text);
        if let Some(action) = self.handle_keyboard(ctx, results.len()) {
            return action;
        }

        ui.allocate_space(Vec2::new(layout.inner.width(), INPUT_HEIGHT));
        ui.add_space(8.0);

        if let Some(status_text) = config.status_text {
            ui.label(egui::RichText::new(status_text).size(11.0).color(theme::FG_DIM()));
            ui.add_space(6.0);
        }

        if let Some(index) = self.render_results(ui, layout, results, render_row) {
            self.selected = index;
            return PickerModalAction::ClickedRow(index);
        }

        render_empty_state(ui, &config.empty_state, results.is_empty());
        if render_footer(ui, config.footer_action_label) {
            return PickerModalAction::FooterAction;
        }

        PickerModalAction::None
    }

    fn render_query_input(&mut self, ui: &mut egui::Ui, inner_rect: Rect, hint_text: &str) {
        let input_rect = Rect::from_min_size(ui.cursor().min, Vec2::new(inner_rect.width(), INPUT_HEIGHT));
        ui.painter()
            .rect_filled(input_rect, CornerRadius::same(12), theme::BG_ELEVATED());
        ui.painter().rect_stroke(
            input_rect,
            CornerRadius::same(12),
            Stroke::new(1.0, theme::alpha(theme::ACCENT(), 70)),
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
                .text_color(theme::FG())
                .frame(false)
                .desired_width(text_rect.width())
                .hint_text(egui::RichText::new(hint_text).color(theme::FG_DIM()).size(13.0))
                .margin(Margin::ZERO),
        );

        if !response.has_focus() && self.opened_at.elapsed().as_millis() < 100 {
            response.request_focus();
        }
    }

    fn handle_keyboard(&mut self, ctx: &Context, result_count: usize) -> Option<PickerModalAction> {
        let (up, down, enter, escape, tab) = ctx.input(|input| {
            (
                input.key_pressed(egui::Key::ArrowUp),
                input.key_pressed(egui::Key::ArrowDown),
                input.key_pressed(egui::Key::Enter),
                input.key_pressed(egui::Key::Escape),
                input.key_pressed(egui::Key::Tab),
            )
        });

        if escape {
            return Some(PickerModalAction::Cancelled);
        }
        if up && self.selected > 0 {
            self.selected -= 1;
        }
        if down && result_count > 0 && self.selected < result_count - 1 {
            self.selected += 1;
        }
        if tab && result_count > 0 {
            return Some(PickerModalAction::CompleteSelection);
        }
        if enter {
            return Some(PickerModalAction::Submit);
        }

        None
    }

    fn render_results<Item>(
        &mut self,
        ui: &mut egui::Ui,
        layout: &PickerLayout,
        results: &[Item],
        render_row: &mut impl FnMut(&mut egui::Ui, f32, usize, &Item, bool) -> bool,
    ) -> Option<usize> {
        if results.is_empty() {
            return None;
        }

        let mut clicked_row = None;
        let max_results_height = usize_to_f32(MAX_VISIBLE_ROWS) * ROW_HEIGHT;
        let scroll_height = max_results_height.min(layout.inner.max.y - ui.cursor().min.y - layout.footer_height - 8.0);

        egui::ScrollArea::vertical()
            .max_height(scroll_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(layout.inner.width());
                for (index, item) in results.iter().enumerate().take(MAX_RENDERED_ROWS) {
                    if render_row(ui, layout.inner.width(), index, item, self.selected == index) {
                        clicked_row = Some(index);
                    }
                }
            });

        clicked_row
    }
}

fn render_empty_state(ui: &mut egui::Ui, empty_state: &PickerEmptyState<'_>, results_empty: bool) {
    if !results_empty {
        return;
    }

    ui.add_space(16.0);
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new(empty_state.message)
                .color(empty_state.color)
                .size(12.0),
        );
    });
}

fn render_footer(ui: &mut egui::Ui, footer_action_label: Option<&str>) -> bool {
    let mut footer_action = false;
    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
        ui.horizontal(|ui| {
            keyboard_hint(ui, "enter", "select");
            keyboard_hint(ui, "tab", "complete");
            keyboard_hint(ui, "esc", "cancel");
            if let Some(label) = footer_action_label {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(Button::new(egui::RichText::new(label).size(11.5).color(theme::FG_DIM())).frame(false))
                        .clicked()
                    {
                        footer_action = true;
                    }
                });
            }
        });
    });
    footer_action
}

fn picker_layout(screen_rect: Rect) -> PickerLayout {
    let footer_height = 36.0;
    let max_results_height = usize_to_f32(MAX_VISIBLE_ROWS) * ROW_HEIGHT;
    let card_height = (INPUT_HEIGHT + 16.0 + max_results_height + footer_height + 44.0).min(PICKER_MAX_HEIGHT);
    let card_min = Pos2::new(
        (screen_rect.width() - PICKER_WIDTH) * 0.5,
        (screen_rect.height() - card_height) * 0.35,
    );
    let card = Rect::from_min_size(card_min, Vec2::new(PICKER_WIDTH, card_height));

    PickerLayout {
        screen: screen_rect,
        inner: card.shrink2(Vec2::new(20.0, 16.0)),
        card,
        footer_height,
    }
}

fn paint_picker_card(ui: &egui::Ui, card_rect: Rect) {
    let painter = ui.painter();
    painter.rect_filled(card_rect, CornerRadius::same(20), theme::PANEL_BG());
    painter.rect_stroke(
        card_rect,
        CornerRadius::same(20),
        Stroke::new(1.5, theme::alpha(theme::ACCENT(), 80)),
        StrokeKind::Outside,
    );
    painter.rect_stroke(
        card_rect.expand(2.0),
        CornerRadius::same(22),
        Stroke::new(2.0, theme::alpha(theme::ACCENT(), 25)),
        StrokeKind::Outside,
    );
}

fn keyboard_hint(ui: &mut egui::Ui, key: &str, desc: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        egui::Frame::default()
            .fill(theme::BG_ELEVATED())
            .corner_radius(4)
            .inner_margin(Margin::symmetric(5, 2))
            .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE(), 160)))
            .show(ui, |ui| {
                ui.label(egui::RichText::new(key).monospace().size(10.0).color(theme::FG()));
            });
        ui.label(egui::RichText::new(desc).size(10.5).color(theme::FG_DIM()));
        ui.add_space(8.0);
    });
}

pub fn split_path_display(display: &str) -> (String, String) {
    if let Some(last_slash) = display.rfind('/') {
        (
            display[..=last_slash].to_string(),
            display[last_slash + 1..].to_string(),
        )
    } else {
        (String::new(), display.to_string())
    }
}

fn usize_to_f32(v: usize) -> f32 {
    let clamped = u16::try_from(v).unwrap_or(u16::MAX);
    f32::from(clamped)
}
