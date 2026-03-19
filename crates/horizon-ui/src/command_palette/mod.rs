mod render;

use std::time::Instant;

use egui::{
    Align, Color32, Context, CornerRadius, Id, Layout, Margin, Order, Rect, Sense, Stroke, StrokeKind, UiBuilder, Vec2,
};
use horizon_core::{PanelId, WorkspaceId};

use crate::command_registry::{Category, CommandEntry, CommandId};
use crate::theme;

use render::{
    PaletteLayout, paint_card, paint_empty_results, palette_layout, render_result_row, render_section_header,
};

const PALETTE_WIDTH: f32 = 500.0;
const INPUT_HEIGHT: f32 = 44.0;
const ROW_HEIGHT: f32 = 36.0;
const SECTION_HEADER_HEIGHT: f32 = 28.0;
const MAX_VISIBLE_ROWS: usize = 12;

// ── Public data fed in by the app each frame ────────────────────────

pub struct WorkspaceEntry {
    pub id: WorkspaceId,
    pub name: String,
    pub color: Color32,
    pub panel_count: usize,
    pub is_active: bool,
}

pub struct PanelEntry {
    pub id: PanelId,
    pub title: String,
    pub workspace_name: String,
    pub cwd: Option<String>,
}

// ── Palette state ───────────────────────────────────────────────────

pub struct CommandPalette {
    query: String,
    selected: usize,
    opened_at: Instant,
}

pub enum PaletteAction {
    None,
    Execute(CommandId),
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaletteMode {
    All,
    ActionsOnly,
    PanelsOnly,
}

// ── Filtered result item (uniform across categories) ────────────────

struct ResultItem {
    id: CommandId,
    label: String,
    detail: String,
    shortcut: Option<String>,
    category: Category,
    accent: Option<Color32>,
}

// ── Implementation ──────────────────────────────────────────────────

impl CommandPalette {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            selected: 0,
            opened_at: Instant::now(),
        }
    }

    pub fn show(
        &mut self,
        ctx: &Context,
        workspaces: &[WorkspaceEntry],
        panels: &[PanelEntry],
        actions: &[CommandEntry],
    ) -> PaletteAction {
        let (mode, search_query) = parse_mode(&self.query);
        let items = build_results(mode, search_query, workspaces, panels, actions);
        let layout = palette_layout(ctx.input(egui::InputState::viewport_rect));

        if self.show_backdrop(ctx, layout.screen) {
            return PaletteAction::Cancelled;
        }

        self.clamp_selection(items.len());
        self.show_modal(ctx, &items, &layout, mode)
    }

    fn clamp_selection(&mut self, count: usize) {
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }

    fn show_backdrop(&self, ctx: &Context, screen_rect: Rect) -> bool {
        let mut cancelled = false;
        egui::Area::new(Id::new("palette_backdrop"))
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

    fn show_modal(
        &mut self,
        ctx: &Context,
        items: &[ResultItem],
        layout: &PaletteLayout,
        mode: PaletteMode,
    ) -> PaletteAction {
        let mut action = PaletteAction::None;

        egui::Area::new(Id::new("palette_modal"))
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
                        action = self.show_contents(ui, ctx, items, layout, mode);
                    },
                );
            });

        action
    }

    fn show_contents(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &Context,
        items: &[ResultItem],
        layout: &PaletteLayout,
        mode: PaletteMode,
    ) -> PaletteAction {
        let title = match mode {
            PaletteMode::All => "Command Palette",
            PaletteMode::ActionsOnly => "Actions",
            PaletteMode::PanelsOnly => "Panels",
        };
        ui.label(egui::RichText::new(title).color(theme::FG).size(15.0).strong());
        ui.add_space(10.0);

        self.render_query_input(ui, layout.inner);
        if let Some(action) = self.handle_keyboard(ctx, items) {
            return action;
        }

        ui.allocate_space(Vec2::new(layout.inner.width(), INPUT_HEIGHT));
        ui.add_space(4.0);

        self.render_hint(ui, layout.inner.width());
        ui.add_space(4.0);

        match self.render_results(ui, items, layout) {
            Some(index) => PaletteAction::Execute(items[index].id.clone()),
            None => PaletteAction::None,
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
                .font(egui::FontId::proportional(14.0))
                .text_color(theme::FG)
                .frame(false)
                .desired_width(text_rect.width())
                .hint_text(egui::RichText::new("Type to search...").color(theme::FG_DIM).size(13.0))
                .margin(Margin::ZERO),
        );
        if !response.has_focus() && self.opened_at.elapsed().as_millis() < 100 {
            response.request_focus();
        }
        if response.changed() {
            self.selected = 0;
        }
    }

    fn render_hint(&self, ui: &mut egui::Ui, width: f32) {
        let (mode, _) = parse_mode(&self.query);
        if mode != PaletteMode::All {
            return;
        }

        let hint = "  >  actions    @  panels";
        let rect = Rect::from_min_size(ui.cursor().min, Vec2::new(width, 16.0));
        ui.painter().text(
            rect.left_center() + Vec2::new(4.0, 0.0),
            egui::Align2::LEFT_CENTER,
            hint,
            egui::FontId::monospace(10.0),
            theme::FG_DIM,
        );
        ui.allocate_space(Vec2::new(width, 16.0));
    }

    fn handle_keyboard(&mut self, ctx: &Context, items: &[ResultItem]) -> Option<PaletteAction> {
        let (up, down, enter, escape) = ctx.input(|input| {
            (
                input.key_pressed(egui::Key::ArrowUp),
                input.key_pressed(egui::Key::ArrowDown),
                input.key_pressed(egui::Key::Enter),
                input.key_pressed(egui::Key::Escape),
            )
        });

        if escape {
            return Some(PaletteAction::Cancelled);
        }
        if up && self.selected > 0 {
            self.selected -= 1;
        }
        if down && !items.is_empty() && self.selected < items.len() - 1 {
            self.selected += 1;
        }
        if enter && !items.is_empty() {
            return Some(PaletteAction::Execute(items[self.selected].id.clone()));
        }

        None
    }

    fn render_results(&mut self, ui: &mut egui::Ui, items: &[ResultItem], layout: &PaletteLayout) -> Option<usize> {
        let mut clicked_idx = None;
        let scroll_height = layout.results_height.min(layout.inner.max.y - ui.cursor().min.y - 8.0);

        egui::ScrollArea::vertical()
            .max_height(scroll_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(layout.inner.width());

                if items.is_empty() {
                    paint_empty_results(ui, "No matching results");
                    return;
                }

                let mut current_category: Option<Category> = None;
                for (i, item) in items.iter().enumerate() {
                    if current_category != Some(item.category) {
                        current_category = Some(item.category);
                        render_section_header(ui, layout.inner.width(), item.category.label());
                    }
                    if render_result_row(ui, layout.inner.width(), i, item, self.selected == i) {
                        clicked_idx = Some(i);
                    }
                }
            });

        clicked_idx
    }
}

// ── Mode parsing ────────────────────────────────────────────────────

fn parse_mode(query: &str) -> (PaletteMode, &str) {
    let trimmed = query.trim_start();
    if let Some(rest) = trimmed.strip_prefix('>') {
        (PaletteMode::ActionsOnly, rest.trim_start())
    } else if let Some(rest) = trimmed.strip_prefix('@') {
        (PaletteMode::PanelsOnly, rest.trim_start())
    } else {
        (PaletteMode::All, trimmed)
    }
}

// ── Result building ─────────────────────────────────────────────────

fn build_results(
    mode: PaletteMode,
    query: &str,
    workspaces: &[WorkspaceEntry],
    panels: &[PanelEntry],
    actions: &[CommandEntry],
) -> Vec<ResultItem> {
    let mut items = Vec::new();
    let query_lower = query.to_ascii_lowercase();

    if mode == PaletteMode::All {
        for ws in workspaces {
            if matches_query(&query_lower, &ws.name, &[]) {
                let detail = if ws.is_active {
                    format!("{} panels  active", ws.panel_count)
                } else {
                    format!("{} panels", ws.panel_count)
                };
                items.push(ResultItem {
                    id: CommandId::SwitchWorkspace(ws.id),
                    label: ws.name.clone(),
                    detail,
                    shortcut: None,
                    category: Category::Workspace,
                    accent: Some(ws.color),
                });
            }
        }
    }

    if mode == PaletteMode::All || mode == PaletteMode::PanelsOnly {
        for panel in panels {
            let extra: Vec<String> = panel
                .cwd
                .iter()
                .cloned()
                .chain(std::iter::once(panel.workspace_name.clone()))
                .collect();
            let extra_refs: Vec<&str> = extra.iter().map(String::as_str).collect();
            if matches_query(&query_lower, &panel.title, &extra_refs) {
                items.push(ResultItem {
                    id: CommandId::FocusPanel(panel.id),
                    label: panel.title.clone(),
                    detail: compact_detail(panel.cwd.as_ref(), &panel.workspace_name),
                    shortcut: None,
                    category: Category::Panel,
                    accent: None,
                });
            }
        }
    }

    if mode == PaletteMode::All || mode == PaletteMode::ActionsOnly {
        for action in actions {
            let keyword_refs: Vec<&str> = action.keywords.iter().map(String::as_str).collect();
            if matches_query(&query_lower, &action.label, &keyword_refs) {
                items.push(ResultItem {
                    id: action.id.clone(),
                    label: action.label.clone(),
                    detail: String::new(),
                    shortcut: action.shortcut.clone(),
                    category: Category::Action,
                    accent: None,
                });
            }
        }
    }

    items
}

fn matches_query(query: &str, label: &str, extras: &[&str]) -> bool {
    if query.is_empty() {
        return true;
    }
    let label_lower = label.to_ascii_lowercase();
    if fuzzy_contains(&label_lower, query) {
        return true;
    }
    extras
        .iter()
        .any(|extra| fuzzy_contains(&extra.to_ascii_lowercase(), query))
}

/// Simple subsequence match: every character of `needle` appears in `haystack`
/// in order.
fn fuzzy_contains(haystack: &str, needle: &str) -> bool {
    let mut haystack_chars = haystack.chars();
    for needle_char in needle.chars() {
        loop {
            match haystack_chars.next() {
                Some(h) if h == needle_char => break,
                Some(_) => {}
                None => return false,
            }
        }
    }
    true
}

fn compact_detail(cwd: Option<&String>, workspace_name: &str) -> String {
    match cwd {
        Some(path) => {
            let short_path = shorten_path(path);
            format!("{short_path}  {workspace_name}")
        }
        None => workspace_name.to_string(),
    }
}

fn shorten_path(path: &str) -> &str {
    match path.rfind('/') {
        Some(last) => match path[..last].rfind('/') {
            Some(prev) => &path[prev + 1..],
            None => &path[last + 1..],
        },
        None => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mode_default_is_all() {
        let (mode, query) = parse_mode("hello");
        assert_eq!(mode, PaletteMode::All);
        assert_eq!(query, "hello");
    }

    #[test]
    fn parse_mode_actions_prefix() {
        let (mode, query) = parse_mode("> toggle");
        assert_eq!(mode, PaletteMode::ActionsOnly);
        assert_eq!(query, "toggle");
    }

    #[test]
    fn parse_mode_panels_prefix() {
        let (mode, query) = parse_mode("@ zsh");
        assert_eq!(mode, PaletteMode::PanelsOnly);
        assert_eq!(query, "zsh");
    }

    #[test]
    fn fuzzy_contains_basic() {
        assert!(fuzzy_contains("toggle sidebar", "tgsb"));
        assert!(fuzzy_contains("toggle sidebar", "toggle"));
        assert!(!fuzzy_contains("toggle", "xyz"));
    }

    #[test]
    fn fuzzy_contains_empty_needle() {
        assert!(fuzzy_contains("anything", ""));
    }

    #[test]
    fn shorten_path_two_components() {
        assert_eq!(shorten_path("/home/user/projects/horizon"), "projects/horizon");
    }

    #[test]
    fn shorten_path_single_component() {
        assert_eq!(shorten_path("horizon"), "horizon");
    }

    #[test]
    fn build_results_filters_by_mode() {
        let workspaces = vec![WorkspaceEntry {
            id: WorkspaceId(1),
            name: "dev".into(),
            color: Color32::WHITE,
            panel_count: 2,
            is_active: true,
        }];
        let panels = vec![PanelEntry {
            id: PanelId(1),
            title: "zsh".into(),
            workspace_name: "dev".into(),
            cwd: Some("/home".into()),
        }];
        let actions = crate::command_registry::action_commands("Ctrl");

        let results = build_results(PaletteMode::ActionsOnly, "", &workspaces, &panels, &actions);
        assert!(results.iter().all(|r| r.category == Category::Action));

        let results = build_results(PaletteMode::PanelsOnly, "", &workspaces, &panels, &actions);
        assert!(results.iter().all(|r| r.category == Category::Panel));
    }
}
