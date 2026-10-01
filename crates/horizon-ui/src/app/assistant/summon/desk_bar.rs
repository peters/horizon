//! The command bar as a window of its own (desktop-workspace prototype).
//!
//! In desk mode the root window is only this bar. It stays on every desktop
//! workspace, shows a minimap of the workspaces with what runs in each, takes
//! the prompt, and grows into one of three expanded layouts for the whole
//! conversation. The layouts are prototypes to compare, switched live.

use egui::{
    Align, Align2, Color32, CornerRadius, Frame, Id, Key, Layout, Margin, Order, Rect, RichText, Sense, Shadow, Stroke,
    StrokeKind, Ui, UiBuilder, ViewportCommand, pos2, vec2,
};
use horizon_core::browser::manifest::agent_panels::AgentState;

use super::super::icons;
use super::super::plan;
use super::{Action, HorizonApp, command_bar, summon_divider};
use crate::app::desk::DeskState;
use crate::theme;

mod paint;

use paint::{
    kind_color, paint_click, paint_keycaps, paint_tile_frame, paint_tile_label, paint_tile_panels, scope_row,
    section_label,
};

const MARGIN: f32 = 16.0;
const MARGIN_PX: i8 = 16;
const TILE_HEIGHT: f32 = 92.0;
/// More workspaces than this are drawn as small tiles in two rows.
const COMPACT_ABOVE: usize = 6;
const COMPACT_PREVIEW: f32 = 36.0;
const PROMPT_AND_FOOTER: f32 = 62.0 + 1.0 + 46.0;
const HINT: f32 = 40.0;
const ROW: f32 = 34.0;
pub(super) const MONITOR: [f32; 2] = [1920.0, 1080.0];

/// How the bar looks when it is expanded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum ExpandStyle {
    /// The conversation grows up from the prompt, minimap in a strip above it.
    #[default]
    Sheet,
    /// Conversation on the left, an overview column on the right.
    Split,
    /// The workspaces take the stage; the conversation sits beside the plan.
    Stage,
}

impl ExpandStyle {
    const ALL: [Self; 3] = [Self::Sheet, Self::Split, Self::Stage];

    const fn label(self) -> &'static str {
        match self {
            Self::Sheet => "A  Sheet",
            Self::Split => "B  Split",
            Self::Stage => "C  Stage",
        }
    }

    /// Window width and height when expanded.
    const fn size(self) -> [f32; 2] {
        match self {
            Self::Sheet => [980.0, 800.0],
            Self::Split => [1320.0, 720.0],
            Self::Stage => [1480.0, 820.0],
        }
    }
}

/// One panel drawn small inside a workspace tile.
pub(super) struct TilePanel {
    /// Left, top, right, bottom as fractions of the tile.
    pub(super) at: [f32; 4],
    pub(super) color: Color32,
    pub(super) state: Option<AgentState>,
}

pub(super) struct Tile {
    pub(super) local_id: String,
    pub(super) name: String,
    pub(super) panels: Vec<TilePanel>,
    pub(super) agents: usize,
    pub(super) working: usize,
    pub(super) needs_you: usize,
}

enum TileAction {
    Go(usize),
    Scope(String),
}

impl HorizonApp {
    pub(in crate::app) fn desk_mode(&self) -> bool {
        self.assistant.desk.is_some()
    }

    /// The whole root window in desk mode: the workspace windows are kept alive,
    /// and the command bar is drawn.
    pub(in crate::app) fn render_desk_root(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.run_demo(&ctx);
        self.render_panel_windows(&ctx);
        self.ensure_assistant_panel(&ctx);
        self.assistant.summon.open = true;
        self.close_assistant_if_restarting();

        let desk = self
            .assistant
            .desk
            .as_ref()
            .map(crate::app::desk::Desk::snapshot)
            .unwrap_or_default();
        self.sync_panel_windows(&desk);
        let tiles = self.desk_tiles(&desk);
        let expanded = self.assistant.summon.expanded;
        let style = self.assistant.summon.style;
        let steps = self.assistant.plan.len();

        let mut action = None;
        let mut tile_action = None;
        let background = theme::PANEL_BG();
        Frame::new()
            .fill(background)
            .stroke(Stroke::new(1.0, theme::ACCENT().gamma_multiply(0.45)))
            .corner_radius(CornerRadius::same(22))
            .inner_margin(Margin::same(MARGIN_PX))
            .show(ui, |ui| {
                if expanded {
                    match style {
                        ExpandStyle::Sheet => self.desk_sheet(ui, &tiles, &desk, &mut action, &mut tile_action),
                        ExpandStyle::Split => self.desk_split(ui, &tiles, &desk, &mut action, &mut tile_action),
                        ExpandStyle::Stage => self.desk_stage(ui, &tiles, &desk, &mut action, &mut tile_action),
                    }
                } else {
                    self.desk_collapsed(ui, &tiles, &desk, &mut action, &mut tile_action);
                }
            });

        self.render_scope_popup(&ctx, &tiles);
        self.apply_desk_actions(&ctx, action, tile_action);
        self.fit_desk_window(&ctx, steps, tiles.len());
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
    }

    fn desk_tiles(&self, desk: &DeskState) -> Vec<Tile> {
        let have_windows = desk
            .windows
            .iter()
            .any(|window| window.app_id.starts_with(crate::app::desk::PANEL_APP_PREFIX));
        self.board
            .workspaces
            .iter()
            .enumerate()
            .map(|(index, workspace)| {
                // What is on this desktop right now, as the shell reports it (with the real
                // window rectangle); before the windows exist, the layout of the workspace.
                let shown: Vec<(&horizon_core::Panel, [f32; 4])> = if have_windows {
                    desk.windows
                        .iter()
                        .filter(|window| usize::try_from(window.workspace).is_ok_and(|ws| ws == index))
                        .filter_map(|window| {
                            let panel = self
                                .board
                                .panels
                                .iter()
                                .find(|panel| crate::app::desk::panel_app_id(&panel.local_id) == window.app_id)?;
                            Some((panel, monitor_fraction(window.rect)))
                        })
                        .collect()
                } else {
                    layout_fractions(
                        workspace
                            .panels
                            .iter()
                            .filter_map(|id| self.board.panel(*id))
                            .filter(|panel| panel.visible && !panel.is_assistant())
                            .collect(),
                    )
                };
                let (mut agents, mut working, mut needs_you) = (0, 0, 0);
                let drawn = shown
                    .iter()
                    .map(|(panel, at)| {
                        let state = panel
                            .kind
                            .is_agent()
                            .then(|| self.board.agent_state(panel.id))
                            .flatten();
                        if let Some(state) = state {
                            agents += 1;
                            working += usize::from(state == AgentState::Working);
                            needs_you += usize::from(state == AgentState::NeedsInput);
                        }
                        TilePanel {
                            at: *at,
                            color: kind_color(panel.kind),
                            state,
                        }
                    })
                    .collect();
                Tile {
                    local_id: workspace.local_id.clone(),
                    name: workspace.name.clone(),
                    panels: drawn,
                    agents,
                    working,
                    needs_you,
                }
            })
            .collect()
    }

    // ---- layouts -------------------------------------------------------

    fn desk_collapsed(
        &mut self,
        ui: &mut Ui,
        tiles: &[Tile],
        desk: &DeskState,
        action: &mut Option<Action>,
        tile_action: &mut Option<TileAction>,
    ) {
        self.minimap_strip(ui, tiles, desk, tile_action);
        ui.add_space(10.0);
        let steps = self.assistant.plan.clone();
        if !steps.is_empty() {
            Frame::new().inner_margin(Margin::symmetric(6, 0)).show(ui, |ui| {
                plan::draw_steps(ui, &steps);
            });
            ui.add_space(6.0);
        }
        self.desk_prompt(ui, action);
        summon_divider(ui);
        self.desk_footer(ui, action, false);
        self.desk_hint(ui);
    }

    /// A: the conversation above the prompt, the minimap as a strip on top.
    fn desk_sheet(
        &mut self,
        ui: &mut Ui,
        tiles: &[Tile],
        desk: &DeskState,
        action: &mut Option<Action>,
        tile_action: &mut Option<TileAction>,
    ) {
        self.desk_header(ui, action);
        ui.add_space(8.0);
        self.minimap_strip(ui, tiles, desk, tile_action);
        ui.add_space(10.0);
        let steps = self.assistant.plan.clone();
        let body = (ui.available_height() - PROMPT_AND_FOOTER - HINT - plan_height(steps.len()) - 8.0).max(120.0);
        self.conversation(ui, body);
        if !steps.is_empty() {
            ui.add_space(6.0);
            Frame::new()
                .inner_margin(Margin::symmetric(6, 0))
                .show(ui, |ui| plan::draw_steps(ui, &steps));
        }
        ui.add_space(8.0);
        self.desk_prompt(ui, action);
        summon_divider(ui);
        self.desk_footer(ui, action, true);
        self.desk_hint(ui);
    }

    /// B: conversation and prompt on the left, the overview on the right.
    fn desk_split(
        &mut self,
        ui: &mut Ui,
        tiles: &[Tile],
        desk: &DeskState,
        action: &mut Option<Action>,
        tile_action: &mut Option<TileAction>,
    ) {
        self.desk_header(ui, action);
        ui.add_space(8.0);
        let total = ui.available_size();
        let right_width = 400.0;
        let left_width = total.x - right_width - 16.0;
        let top = ui.cursor().min;
        let left = Rect::from_min_size(top, vec2(left_width, total.y));
        let right = Rect::from_min_size(top + vec2(left_width + 16.0, 0.0), vec2(right_width, total.y));

        let mut left_ui = ui.new_child(UiBuilder::new().max_rect(left));
        let body = (left.height() - PROMPT_AND_FOOTER - HINT - 8.0).max(120.0);
        self.conversation(&mut left_ui, body);
        left_ui.add_space(8.0);
        self.desk_prompt(&mut left_ui, action);
        summon_divider(&mut left_ui);
        self.desk_footer(&mut left_ui, action, true);
        self.desk_hint(&mut left_ui);

        let mut right_ui = ui.new_child(UiBuilder::new().max_rect(right));
        section_label(&mut right_ui, "Workspaces");
        right_ui.add_space(6.0);
        let columns = if tiles.len() > COMPACT_ABOVE { 5 } else { 2 };
        self.minimap_grid(&mut right_ui, tiles, desk, tile_action, columns);
        right_ui.add_space(14.0);
        section_label(&mut right_ui, "Plan");
        right_ui.add_space(4.0);
        let steps = self.assistant.plan.clone();
        if steps.is_empty() {
            right_ui.label(
                RichText::new("No plan yet. Ask for something.")
                    .size(12.0)
                    .color(theme::FG_DIM()),
            );
        } else {
            plan::draw_steps(&mut right_ui, &steps);
        }
        right_ui.add_space(12.0);
        self.render_reach_strip(&mut right_ui);
        self.render_cards_tray(&mut right_ui);
        ui.allocate_rect(left.union(right), Sense::hover());
    }

    /// C: the workspaces on a wide stage, conversation and plan below them.
    fn desk_stage(
        &mut self,
        ui: &mut Ui,
        tiles: &[Tile],
        desk: &DeskState,
        action: &mut Option<Action>,
        tile_action: &mut Option<TileAction>,
    ) {
        self.desk_header(ui, action);
        ui.add_space(10.0);
        // Many workspaces become two rows of small tiles, leaving room for the conversation.
        let columns = if tiles.len() > COMPACT_ABOVE {
            tiles.len().div_ceil(2)
        } else {
            tiles.len().clamp(1, 4)
        };
        self.minimap_grid(ui, tiles, desk, tile_action, columns);
        ui.add_space(12.0);
        let total = ui.available_size();
        let plan_width = 420.0;
        let top = ui.cursor().min;
        let height = (total.y - PROMPT_AND_FOOTER - HINT - 10.0).max(140.0);
        let left = Rect::from_min_size(top, vec2(plan_width, height));
        let right = Rect::from_min_size(
            top + vec2(plan_width + 16.0, 0.0),
            vec2(total.x - plan_width - 16.0, height),
        );

        let mut left_ui = ui.new_child(UiBuilder::new().max_rect(left));
        section_label(&mut left_ui, "Plan");
        left_ui.add_space(4.0);
        let steps = self.assistant.plan.clone();
        if steps.is_empty() {
            left_ui.label(
                RichText::new("No plan yet. Ask for something.")
                    .size(12.0)
                    .color(theme::FG_DIM()),
            );
        } else {
            plan::draw_steps(&mut left_ui, &steps);
        }
        left_ui.add_space(10.0);
        self.render_cards_tray(&mut left_ui);

        let mut right_ui = ui.new_child(UiBuilder::new().max_rect(right));
        self.conversation(&mut right_ui, right.height());
        ui.allocate_rect(left.union(right), Sense::hover());
        ui.add_space(10.0);
        self.desk_prompt(ui, action);
        summon_divider(ui);
        self.desk_footer(ui, action, true);
        self.desk_hint(ui);
    }

    // ---- pieces --------------------------------------------------------

    fn desk_header(&mut self, ui: &mut Ui, action: &mut Option<Action>) {
        ui.horizontal(|ui| {
            let (mark, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
            icons::paint_mark(ui.painter(), mark);
            ui.vertical(|ui| {
                ui.add_space(2.0);
                ui.label(RichText::new("Assistant").size(14.5).strong().color(theme::FG()));
                ui.label(RichText::new(self.scope_label()).size(11.5).color(theme::FG_DIM()));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if super::ghost_button(ui, "Collapse").clicked() {
                    *action = Some(Action::Close);
                }
                ui.add_space(6.0);
                for style in ExpandStyle::ALL.iter().rev() {
                    let selected = self.assistant.summon.style == *style;
                    let label = RichText::new(style.label()).size(11.5).color(if selected {
                        theme::FG()
                    } else {
                        theme::FG_DIM()
                    });
                    let button = egui::Button::new(label)
                        .fill(if selected {
                            theme::ACCENT().gamma_multiply(0.22)
                        } else {
                            theme::PANEL_BG_ALT()
                        })
                        .stroke(Stroke::new(
                            1.0,
                            if selected {
                                theme::ACCENT().gamma_multiply(0.6)
                            } else {
                                theme::BORDER_SUBTLE()
                            },
                        ))
                        .corner_radius(CornerRadius::same(8))
                        .min_size(vec2(0.0, 26.0));
                    if ui.add(button).clicked() {
                        self.assistant.summon.style = *style;
                    }
                }
            });
        });
    }

    /// The assistant's terminal, `height` tall, on a rounded dark panel.
    fn conversation(&mut self, ui: &mut Ui, height: f32) {
        let Some(panel) = self.board.assistant_panel() else {
            ui.label(RichText::new("Starting the assistant...").color(theme::FG_DIM()));
            return;
        };
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(12), theme::BG());
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(12),
            Stroke::new(1.0, theme::BORDER_SUBTLE()),
            StrokeKind::Inside,
        );
        let mut body = ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(12.0, 10.0))));
        if self.show_assistant_terminal(&mut body, panel) {
            self.focus_assistant();
            let id = Id::new(super::INPUT_ID);
            ui.memory_mut(|memory| memory.surrender_focus(id));
        }
    }

    fn desk_prompt(&mut self, ui: &mut Ui, action: &mut Option<Action>) {
        let id = Id::new(super::INPUT_ID);
        let agent = self.assistant.settings.agent;
        let listed = command_bar::suggestions(&self.assistant.summon.text, agent);
        if let Some(keys) = self.summon_keys(ui.ctx(), &listed, id) {
            *action = Some(keys);
        }
        let mic = self.summon_mic_state();
        let mut text = std::mem::take(&mut self.assistant.summon.text);
        let focus = std::mem::take(&mut self.assistant.summon.focus_requested);
        self.summon_prompt_row(ui, &mut text, id, mic, focus, action);
        self.assistant.summon.text = text;
        if !listed.is_empty() {
            summon_divider(ui);
            let selected = self.assistant.summon.selected;
            for (index, command) in listed.iter().enumerate() {
                if super::command_row(ui, command, index == selected, agent).clicked() {
                    *action = Some(Action::Run(super::entry_for(*command)));
                }
            }
        }
    }

    fn desk_footer(&mut self, ui: &mut Ui, action: &mut Option<Action>, expanded: bool) {
        let agent = self.assistant.settings.agent;
        let ask = self.assistant.settings.ask_before_send;
        let steps = self.assistant.plan.clone();
        let anchor = self.summon_footer_with_scope(ui, agent, ask, &steps, action, expanded);
        self.assistant.scope_anchor = Some(anchor);
    }

    fn desk_hint(&self, ui: &mut Ui) {
        ui.add_space(8.0);
        if let Some((right, progress)) = self.assistant.demo.as_ref().and_then(super::demo::Demo::key_progress) {
            paint_keycaps(ui, right, progress);
            return;
        }
        let hint = self.assistant.command.feedback_text().unwrap_or_else(|| {
            "Up arrow for history     Tab to expand     Esc to collapse     Click a workspace to go there".to_string()
        });
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(hint).size(11.5).color(theme::FG_DIM()));
        });
    }

    fn apply_desk_actions(&mut self, ctx: &egui::Context, action: Option<Action>, tile: Option<TileAction>) {
        match tile {
            Some(TileAction::Go(index)) => {
                if let Some(desk) = self.assistant.desk.as_ref() {
                    desk.switch(index);
                }
            }
            Some(TileAction::Scope(local_id)) => {
                let every = self.workspace_local_ids();
                self.assistant.scope.toggle(&local_id, &every);
            }
            None => {}
        }
        match action {
            Some(Action::Run(entry)) => self.submit_summon(&entry),
            Some(Action::Complete(name)) => self.assistant.summon.text = format!("/{name}"),
            Some(Action::OpenAsChat) => self.assistant.summon.expanded = !self.assistant.summon.expanded,
            Some(Action::Close) => {
                if self.assistant.summon.expanded {
                    self.assistant.summon.expanded = false;
                }
            }
            Some(Action::ToggleAsk) => self.run_local_command(command_bar::LocalCommand::ToggleAsk),
            Some(Action::ToggleDictation) => self.toggle_assistant_dictation(ctx),
            Some(Action::Scope(anchor)) => {
                self.assistant.scope_open = !self.assistant.scope_open;
                self.assistant.scope_anchor = Some(anchor);
            }
            None => {}
        }
    }

    /// Sizes the native window for the current layout and asks the shell to put it
    /// at the bottom centre of the monitor.
    fn fit_desk_window(&mut self, ctx: &egui::Context, plan_steps: usize, tiles: usize) {
        let monitor = ctx
            .input(|input| input.viewport().monitor_size)
            .map_or(MONITOR, |size| [size.x, size.y]);
        let size = if self.assistant.summon.expanded {
            self.assistant.summon.style.size()
        } else {
            [980.0, collapsed_height(plan_steps, tiles)]
        };
        let size = [size[0].min(monitor[0] - 32.0), size[1].min(monitor[1] - 80.0)];
        if self.assistant.summon.window_size != Some(size) {
            self.assistant.summon.window_size = Some(size);
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(size[0], size[1])));
            // Force the shell to re-place it for the new size.
            if let Some(desk) = self.assistant.desk.as_mut() {
                desk.invalidate();
            }
        }
        let x = ((monitor[0] - size[0]) / 2.0).round();
        let y = (monitor[1] - size[1] - 26.0).round();
        let workspaces = self.board.workspaces.len();
        if let Some(desk) = self.assistant.desk.as_mut() {
            #[allow(clippy::cast_possible_truncation)]
            desk.place_bar(workspaces, [x as i32, y as i32, size[0] as i32, size[1] as i32]);
        }
    }

    // ---- minimap -------------------------------------------------------

    /// One row of workspace tiles across the bar.
    fn minimap_strip(&self, ui: &mut Ui, tiles: &[Tile], desk: &DeskState, action: &mut Option<TileAction>) {
        if tiles.is_empty() {
            return;
        }
        if tiles.len() > COMPACT_ABOVE {
            self.minimap_compact(ui, tiles, desk, action);
            return;
        }
        let gap = 10.0;
        let count = super::super::num::count(tiles.len());
        let width = ((ui.available_width() - gap * (count - 1.0)) / count).max(80.0);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), TILE_HEIGHT + 22.0), Sense::hover());
        for (index, tile) in tiles.iter().enumerate() {
            let left = row.left() + super::super::num::count(index) * (width + gap);
            let rect = Rect::from_min_size(pos2(left, row.top()), vec2(width, TILE_HEIGHT + 22.0));
            self.workspace_tile(ui, rect, index, tile, desk, action);
        }
    }

    /// Many workspaces: small tiles in one row, each with its number and state; hover for the name.
    fn minimap_compact(&self, ui: &mut Ui, tiles: &[Tile], desk: &DeskState, action: &mut Option<TileAction>) {
        let gap = 6.0;
        let columns = tiles.len().max(1);
        let width = (ui.available_width() - gap * (super::super::num::count(columns) - 1.0))
            / super::super::num::count(columns);
        let height = COMPACT_PREVIEW + 22.0;
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        for (index, tile) in tiles.iter().enumerate() {
            let rect = Rect::from_min_size(
                pos2(
                    area.left() + super::super::num::count(index) * (width + gap),
                    area.top(),
                ),
                vec2(width, height),
            );
            self.workspace_tile(ui, rect, index, tile, desk, action);
        }
    }

    /// Workspace tiles in a grid, as large as the width allows.
    fn minimap_grid(
        &self,
        ui: &mut Ui,
        tiles: &[Tile],
        desk: &DeskState,
        action: &mut Option<TileAction>,
        columns: usize,
    ) {
        let gap = 10.0;
        let columns = columns.max(1);
        let width = (ui.available_width() - gap * (super::super::num::count(columns) - 1.0))
            / super::super::num::count(columns);
        let height = (width * 0.58).clamp(40.0, 150.0) + 22.0;
        let rows = tiles.len().div_ceil(columns);
        let (area, _) = ui.allocate_exact_size(
            vec2(
                ui.available_width(),
                super::super::num::count(rows) * (height + gap) - gap,
            ),
            Sense::hover(),
        );
        for (index, tile) in tiles.iter().enumerate() {
            let (row, column) = (index / columns, index % columns);
            let rect = Rect::from_min_size(
                pos2(
                    area.left() + super::super::num::count(column) * (width + gap),
                    area.top() + super::super::num::count(row) * (height + gap),
                ),
                vec2(width, height),
            );
            self.workspace_tile(ui, rect, index, tile, desk, action);
        }
    }

    fn workspace_tile(
        &self,
        ui: &mut Ui,
        rect: Rect,
        index: usize,
        tile: &Tile,
        desk: &DeskState,
        action: &mut Option<TileAction>,
    ) {
        let active = desk.active == index;
        let in_scope = self.assistant.scope.includes(&tile.local_id);
        let narrowed = in_scope && !self.assistant.scope.is_all();
        let response = ui.interact(rect, Id::new(("desk_tile", index)), Sense::click());
        let preview = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y - 22.0));
        let accent = theme::ACCENT();

        paint_tile_frame(ui, preview, active, response.hovered());
        paint_tile_panels(ui, preview, tile);
        if narrowed {
            ui.painter().rect_stroke(
                preview.expand(2.0),
                CornerRadius::same(12),
                Stroke::new(1.0, accent.gamma_multiply(0.5)),
                StrokeKind::Outside,
            );
        }
        paint_tile_label(ui, rect, index, tile, active);
        if let Some((clicked, progress)) = self.assistant.demo.as_ref().and_then(super::demo::Demo::click_progress)
            && clicked == index
        {
            paint_click(ui, preview.center(), progress);
        }

        // The corner dot narrows the assistant to this workspace; the rest of the tile goes there.
        let dot_rect = Rect::from_center_size(preview.right_bottom() - vec2(12.0, 12.0), vec2(18.0, 18.0));
        // Narrow tiles (many workspaces) leave the scope to the chip's picker.
        let has_dot = rect.width() >= 90.0;
        let dot = has_dot.then(|| ui.interact(dot_rect, Id::new(("desk_tile_scope", index)), Sense::click()));
        if has_dot {
            let dot_color = if in_scope { accent } else { theme::BORDER_STRONG() };
            ui.painter()
                .circle_stroke(dot_rect.center(), 5.0, Stroke::new(1.3, dot_color));
            if narrowed {
                ui.painter().circle_filled(dot_rect.center(), 3.0, accent);
            }
        } else if narrowed {
            ui.painter()
                .circle_filled(preview.right_bottom() - vec2(7.0, 7.0), 2.5, accent);
        }
        let dot_clicked = dot
            .map(|dot| dot.on_hover_text("Ask the assistant about this workspace only"))
            .is_some_and(|dot| dot.clicked());
        if dot_clicked {
            *action = Some(TileAction::Scope(tile.local_id.clone()));
        } else if response.clicked() {
            *action = Some(TileAction::Go(index));
        }
        response.on_hover_text(format!("{} - click to go there", tile.name));
    }

    // ---- scope ---------------------------------------------------------

    fn render_scope_popup(&mut self, ctx: &egui::Context, tiles: &[Tile]) {
        if !self.assistant.scope_open {
            return;
        }
        let Some(anchor) = self.assistant.scope_anchor else {
            return;
        };
        let mut toggle = None;
        let mut all = false;
        let area = egui::Area::new(Id::new("assistant_scope_popup"))
            .order(Order::Tooltip)
            .pivot(Align2::LEFT_BOTTOM)
            .fixed_pos(anchor.left_top() - vec2(0.0, 8.0))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(theme::BG_ELEVATED())
                    .stroke(Stroke::new(1.0, theme::BORDER_STRONG()))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::same(10))
                    .shadow(Shadow {
                        offset: [0, -8],
                        blur: 28,
                        spread: 1,
                        color: Color32::from_black_alpha(120),
                    })
                    .show(ui, |ui| {
                        ui.set_width(260.0);
                        section_label(ui, "The assistant looks at");
                        ui.add_space(6.0);
                        let everything = self.assistant.scope.is_all();
                        if scope_row(ui, "All workspaces", everything).clicked() {
                            all = true;
                        }
                        for tile in tiles {
                            let on = !everything && self.assistant.scope.includes(&tile.local_id);
                            if scope_row(ui, &tile.name, on).clicked() {
                                toggle = Some(tile.local_id.clone());
                            }
                        }
                    });
            });
        if all {
            self.assistant.scope.set_all();
        }
        if let Some(local_id) = toggle {
            let every = self.workspace_local_ids();
            self.assistant.scope.toggle(&local_id, &every);
        }
        let outside = ctx.input(|input| {
            input.pointer.any_pressed()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|at| !area.response.rect.contains(at) && !anchor.contains(at))
        });
        if outside || ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.assistant.scope_open = false;
        }
    }
}

/// A window's rectangle as fractions of the monitor below the shell's top bar.
fn monitor_fraction(rect: [i32; 4]) -> [f32; 4] {
    let [left, top, width, height] = rect.map(|value| f32::from(i16::try_from(value).unwrap_or(0)));
    let (w, h) = (MONITOR[0], MONITOR[1] - 32.0);
    let clamp = |value: f32| value.clamp(0.0, 1.0);
    [
        clamp(left / w),
        clamp((top - 32.0) / h),
        clamp((left + width) / w),
        clamp((top + height - 32.0) / h),
    ]
}

/// The layout of panels fitted into the tile, for workspaces with no windows yet.
fn layout_fractions(panels: Vec<&horizon_core::Panel>) -> Vec<(&horizon_core::Panel, [f32; 4])> {
    let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
    for panel in &panels {
        let (position, size) = (panel.layout.position, panel.layout.size);
        min = [min[0].min(position[0]), min[1].min(position[1])];
        max = [max[0].max(position[0] + size[0]), max[1].max(position[1] + size[1])];
    }
    let span = [(max[0] - min[0]).max(1.0), (max[1] - min[1]).max(1.0)];
    panels
        .into_iter()
        .map(|panel| {
            let (position, size) = (panel.layout.position, panel.layout.size);
            let at = [
                (position[0] - min[0]) / span[0],
                (position[1] - min[1]) / span[1],
                (position[0] + size[0] - min[0]) / span[0],
                (position[1] + size[1] - min[1]) / span[1],
            ];
            (panel, at)
        })
        .collect()
}

/// Height of the row (or two rows, with many workspaces) of tiles in the bar.
fn strip_height(tiles: usize) -> f32 {
    if tiles > COMPACT_ABOVE {
        COMPACT_PREVIEW + 22.0
    } else {
        TILE_HEIGHT + 22.0
    }
}

fn plan_height(steps: usize) -> f32 {
    if steps == 0 {
        0.0
    } else {
        super::super::num::count(steps) * ROW + 8.0
    }
}

fn collapsed_height(steps: usize, tiles: usize) -> f32 {
    MARGIN * 2.0 + strip_height(tiles) + 10.0 + plan_height(steps) + 6.0 + PROMPT_AND_FOOTER + HINT + 4.0
}
