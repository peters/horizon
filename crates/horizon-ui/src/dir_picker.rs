use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use egui::{
    Align, Button, Color32, Context, CornerRadius, Id, Layout, Margin, Order, Pos2, Rect, Sense, Stroke, StrokeKind,
    UiBuilder, Vec2,
};
use horizon_core::dir_search;
use horizon_core::{PresetConfig, WorkspaceId};

use crate::theme;

const PICKER_WIDTH: f32 = 520.0;
const PICKER_MAX_HEIGHT: f32 = 460.0;
const INPUT_HEIGHT: f32 = 44.0;
const ROW_HEIGHT: f32 = 34.0;
const MAX_VISIBLE_ROWS: usize = 12;
const MAX_RENDERED_ROWS: usize = 36;
const DEBOUNCE_MS: u64 = 60;

/// What happens after a directory is chosen.
pub enum DirPickerPurpose {
    /// Create a new workspace + first panel at the given canvas position.
    NewWorkspace { canvas_pos: [f32; 2], preset: PresetConfig },
    /// Add a panel to an existing workspace.
    AddPanel {
        workspace_id: WorkspaceId,
        preset: PresetConfig,
        canvas_pos: Option<[f32; 2]>,
    },
}

/// The directory picker modal state.
pub struct DirPicker {
    query: String,
    results: Vec<PathBuf>,
    selected: usize,
    purpose: Option<DirPickerPurpose>,
    search_rx: Option<mpsc::Receiver<Vec<PathBuf>>>,
    last_query_sent: String,
    last_query_time: Instant,
    opened_at: Instant,
    initial_results_loaded: bool,
}

/// What the caller should do after rendering a frame.
pub enum DirPickerAction {
    /// Picker is still open, nothing to do.
    None,
    /// User selected a directory (or skipped). Second field is the purpose.
    Selected(Option<PathBuf>, DirPickerPurpose),
    /// User cancelled.
    Cancelled,
}

struct PickerLayout {
    screen: Rect,
    card: Rect,
    inner: Rect,
    results_height: f32,
    footer_height: f32,
}

impl DirPicker {
    pub fn new(purpose: DirPickerPurpose) -> Self {
        let rx = dir_search::spawn_dir_search(String::new());
        Self {
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            purpose: Some(purpose),
            search_rx: Some(rx),
            last_query_sent: String::new(),
            last_query_time: Instant::now(),
            opened_at: Instant::now(),
            initial_results_loaded: false,
        }
    }

    /// Poll for search results and fire new searches on query change.
    fn update_search(&mut self) {
        if let Some(rx) = &self.search_rx {
            match rx.try_recv() {
                Ok(results) => {
                    self.results = results;
                    if self.selected >= self.results.len() {
                        self.selected = 0;
                    }
                    self.search_rx = None;
                    self.initial_results_loaded = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.search_rx = None;
                }
            }
        }

        if self.query != self.last_query_sent && self.last_query_time.elapsed().as_millis() >= u128::from(DEBOUNCE_MS) {
            self.last_query_sent.clone_from(&self.query);
            self.last_query_time = Instant::now();
            self.search_rx = Some(dir_search::spawn_dir_search(self.query.clone()));
        }
    }

    fn take_purpose(&mut self) -> Option<DirPickerPurpose> {
        self.purpose.take()
    }

    /// Try to produce a `Selected` action, consuming the purpose.
    fn select(&mut self, path: Option<PathBuf>) -> DirPickerAction {
        match self.take_purpose() {
            Some(purpose) => DirPickerAction::Selected(path, purpose),
            None => DirPickerAction::Cancelled, // purpose already consumed — treat as cancel
        }
    }

    /// Render the picker modal. Returns the action the caller should execute.
    pub fn show(&mut self, ctx: &Context) -> DirPickerAction {
        self.update_search();
        let layout = picker_layout(ctx.input(egui::InputState::viewport_rect));
        if self.show_backdrop(ctx, layout.screen) {
            return DirPickerAction::Cancelled;
        }

        self.show_modal(ctx, &layout)
    }

    fn show_backdrop(&self, ctx: &Context, screen_rect: Rect) -> bool {
        let mut cancelled = false;
        egui::Area::new(Id::new("dir_picker_backdrop"))
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

    fn show_modal(&mut self, ctx: &Context, layout: &PickerLayout) -> DirPickerAction {
        let mut action = DirPickerAction::None;
        egui::Area::new(Id::new("dir_picker_modal"))
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
                        action = self.show_modal_contents(ui, ctx, layout);
                    },
                );
            });

        action
    }

    fn show_modal_contents(&mut self, ui: &mut egui::Ui, ctx: &Context, layout: &PickerLayout) -> DirPickerAction {
        ui.label(egui::RichText::new(self.heading()).color(theme::FG).size(15.0).strong());
        ui.add_space(10.0);

        self.render_query_input(ui, layout.inner);
        if let Some(action) = self.handle_keyboard(ctx) {
            return action;
        }

        ui.allocate_space(Vec2::new(layout.inner.width(), INPUT_HEIGHT));
        ui.add_space(8.0);

        if let Some(index) = self.render_results(ui, layout) {
            self.selected = index;
            return self.select(Some(self.results[index].clone()));
        }

        self.render_empty_state(ui);
        if Self::render_footer(ui) {
            return self.select(None);
        }

        DirPickerAction::None
    }

    fn heading(&self) -> &'static str {
        match &self.purpose {
            Some(DirPickerPurpose::NewWorkspace { .. }) => "Select workspace directory",
            Some(DirPickerPurpose::AddPanel { .. }) | None => "Select terminal directory",
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
        child.label(egui::RichText::new("~").monospace().size(16.0).color(theme::ACCENT));
        child.add_space(4.0);

        let response = child.add(
            egui::TextEdit::singleline(&mut self.query)
                .font(egui::FontId::monospace(14.0))
                .text_color(theme::FG)
                .frame(false)
                .desired_width(text_rect.width() - 30.0)
                .hint_text(
                    egui::RichText::new("Type a path or search...")
                        .color(theme::FG_DIM)
                        .size(13.0),
                )
                .margin(Margin::ZERO),
        );

        if response.changed() {
            self.last_query_time = Instant::now();
        }
        if !response.has_focus() && self.opened_at.elapsed().as_millis() < 100 {
            response.request_focus();
        }
    }

    fn handle_keyboard(&mut self, ctx: &Context) -> Option<DirPickerAction> {
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
            return Some(DirPickerAction::Cancelled);
        }
        if up && self.selected > 0 {
            self.selected -= 1;
        }
        if down && !self.results.is_empty() && self.selected < self.results.len() - 1 {
            self.selected += 1;
        }
        if tab && !self.results.is_empty() {
            let path = &self.results[self.selected];
            self.query = format!("{}/", dir_search::abbreviate_home(path));
            self.last_query_time = Instant::now();
        }
        if enter {
            return Some(self.confirm_selection());
        }

        None
    }

    fn confirm_selection(&mut self) -> DirPickerAction {
        if let Some(path) = self.results.get(self.selected).cloned() {
            return self.select(Some(path));
        }
        if self.query.is_empty() {
            return self.select(None);
        }

        let expanded = expand_tilde_simple(&self.query);
        if expanded.is_dir() {
            self.select(Some(expanded))
        } else {
            DirPickerAction::None
        }
    }

    fn render_results(&mut self, ui: &mut egui::Ui, layout: &PickerLayout) -> Option<usize> {
        if self.results.is_empty() {
            return None;
        }

        let mut clicked_row = None;
        let scroll_height = layout
            .results_height
            .min(layout.inner.max.y - ui.cursor().min.y - layout.footer_height - 8.0);

        egui::ScrollArea::vertical()
            .max_height(scroll_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(layout.inner.width());
                for (index, path) in self.results.iter().enumerate().take(MAX_RENDERED_ROWS) {
                    if render_result_row(ui, layout.inner.width(), index, path, self.selected == index) {
                        clicked_row = Some(index);
                    }
                }
            });

        clicked_row
    }

    fn render_empty_state(&self, ui: &mut egui::Ui) {
        if !self.results.is_empty() {
            return;
        }

        let message = if self.initial_results_loaded {
            "No directories found"
        } else {
            "Searching..."
        };
        ui.add_space(16.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(message).color(theme::FG_DIM).size(12.0));
        });
    }

    fn render_footer(ui: &mut egui::Ui) -> bool {
        let mut skip = false;
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.horizontal(|ui| {
                keyboard_hint(ui, "enter", "select");
                keyboard_hint(ui, "tab", "complete");
                keyboard_hint(ui, "esc", "cancel");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(
                            Button::new(
                                egui::RichText::new("Skip (use default)")
                                    .size(11.5)
                                    .color(theme::FG_DIM),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        skip = true;
                    }
                });
            });
        });
        skip
    }
}

fn picker_layout(screen_rect: Rect) -> PickerLayout {
    let results_height = usize_to_f32(MAX_VISIBLE_ROWS) * ROW_HEIGHT;
    let footer_height = 36.0;
    let card_height = (INPUT_HEIGHT + 16.0 + results_height + footer_height + 44.0).min(PICKER_MAX_HEIGHT);
    let card_min = Pos2::new(
        (screen_rect.width() - PICKER_WIDTH) * 0.5,
        (screen_rect.height() - card_height) * 0.35,
    );
    let card = Rect::from_min_size(card_min, Vec2::new(PICKER_WIDTH, card_height));

    PickerLayout {
        screen: screen_rect,
        inner: card.shrink2(Vec2::new(20.0, 16.0)),
        card,
        results_height,
        footer_height,
    }
}

fn paint_picker_card(ui: &egui::Ui, card_rect: Rect) {
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
}

/// Render a single result row. Returns `true` if clicked.
fn render_result_row(ui: &mut egui::Ui, width: f32, index: usize, path: &std::path::Path, is_selected: bool) -> bool {
    let display = dir_search::abbreviate_home(path);
    let is_project =
        path.join(".git").exists() || path.join("Cargo.toml").exists() || path.join("package.json").exists();

    let row_rect = ui.allocate_space(Vec2::new(width, ROW_HEIGHT)).1;
    let mut clicked = false;

    // Background
    if is_selected {
        ui.painter_at(row_rect).rect_filled(
            row_rect,
            CornerRadius::same(8),
            theme::alpha(theme::blend(theme::PANEL_BG_ALT, theme::ACCENT, 0.28), 200),
        );
    } else {
        let hover = ui
            .interact(row_rect, ui.make_persistent_id(("dir_hover", index)), Sense::hover())
            .hovered();
        if hover {
            ui.painter_at(row_rect).rect_filled(
                row_rect,
                CornerRadius::same(8),
                theme::alpha(theme::PANEL_BG_ALT, 160),
            );
        }
    }

    // Click
    let click = ui.interact(row_rect, ui.make_persistent_id(("dir_click", index)), Sense::click());
    if click.clicked() {
        clicked = true;
    }

    // Content
    let text_y = row_rect.center().y;

    // Project indicator dot
    if is_project {
        ui.painter_at(row_rect)
            .circle_filled(Pos2::new(row_rect.min.x + 14.0, text_y), 3.0, theme::ACCENT);
    }

    // Path text: dim parent + bright leaf
    let (dir_part, name_part) = split_path_display(&display);
    let text_x = row_rect.min.x + 26.0;

    if dir_part.is_empty() {
        ui.painter_at(row_rect).text(
            Pos2::new(text_x, text_y),
            egui::Align2::LEFT_CENTER,
            &display,
            egui::FontId::monospace(12.5),
            if is_selected { theme::FG } else { theme::FG_SOFT },
        );
    } else {
        let dir_end = ui.painter_at(row_rect).text(
            Pos2::new(text_x, text_y),
            egui::Align2::LEFT_CENTER,
            &dir_part,
            egui::FontId::monospace(12.5),
            theme::FG_DIM,
        );
        ui.painter_at(row_rect).text(
            Pos2::new(dir_end.max.x, text_y),
            egui::Align2::LEFT_CENTER,
            &name_part,
            egui::FontId::monospace(12.5),
            if is_selected { theme::FG } else { theme::FG_SOFT },
        );
    }

    clicked
}

/// Render a keyboard shortcut hint.
fn keyboard_hint(ui: &mut egui::Ui, key: &str, desc: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        egui::Frame::default()
            .fill(theme::BG_ELEVATED)
            .corner_radius(4)
            .inner_margin(Margin::symmetric(5, 2))
            .stroke(Stroke::new(1.0, theme::alpha(theme::BORDER_SUBTLE, 160)))
            .show(ui, |ui| {
                ui.label(egui::RichText::new(key).monospace().size(10.0).color(theme::FG));
            });
        ui.label(egui::RichText::new(desc).size(10.5).color(theme::FG_DIM));
        ui.add_space(8.0);
    });
}

/// Split "~/foo/bar/baz" into ("~/foo/bar/", "baz").
fn split_path_display(display: &str) -> (String, String) {
    if let Some(last_slash) = display.rfind('/') {
        (
            display[..=last_slash].to_string(),
            display[last_slash + 1..].to_string(),
        )
    } else {
        (String::new(), display.to_string())
    }
}

fn expand_tilde_simple(input: &str) -> PathBuf {
    if let Some(rest) = input.strip_prefix('~') {
        let home: PathBuf = std::env::var("HOME").map_or_else(|_| PathBuf::from("/"), PathBuf::from);
        if rest.is_empty() {
            home
        } else {
            home.join(rest.strip_prefix('/').unwrap_or(rest))
        }
    } else {
        PathBuf::from(input)
    }
}

fn usize_to_f32(v: usize) -> f32 {
    let clamped = u16::try_from(v).unwrap_or(u16::MAX);
    f32::from(clamped)
}
