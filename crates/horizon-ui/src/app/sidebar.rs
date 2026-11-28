pub(in crate::app) mod agents;
pub(in crate::app) mod caption;
mod computers;
mod fold;
mod footer;
mod front_desk_row;
pub(in crate::app) mod inbox;
mod list;
pub(in crate::app) mod machines;
mod menus;
mod new_workspace;
mod panel_rows;
mod place;
mod rent_machine;
mod rich_menu;
pub(in crate::app) mod rows;
mod text;
mod toolbar;

pub(in crate::app) use caption::Counts;
pub(in crate::app) use fold::Folds;
pub(in crate::app) use list::ListCache;
pub(in crate::app) use rent_machine::Dashes;

use std::collections::HashMap;

use egui::{Color32, Context, CornerRadius, CursorIcon, Id, Order, Pos2, Rect, Sense, Stroke, Vec2};
use horizon_core::workspace_list::Row;
use horizon_core::{PanelId, PanelKind, WorkspaceDockSide, WorkspaceId, WorkspaceLayout};

use crate::theme;

use super::util;
use super::workspace::WorkspaceLayoutCapabilities;
use super::{HorizonApp, SIDEBAR_WIDTH, TOOLBAR_HEIGHT};

/// The fill of a row or of a control that the pointer is on, at half strength.
pub(super) const HALF_FILL: u8 = 128;

/// The state of the drawing of the sidebar that a person changes and that no
/// read of the board holds: which work items show their panel rows, the count
/// strings of the captions, and the dashes of the rent control.
#[derive(Default)]
pub(in crate::app) struct SidebarChrome {
    pub(in crate::app) folds: Folds,
    pub(in crate::app) counts: Counts,
    pub(in crate::app) dashes: Dashes,
}

struct WorkspaceSidebarEntry {
    id: WorkspaceId,
    /// The persistent id of the workspace, which its fold is kept by: the id of a
    /// workspace restarts at 1 in a replaced session.
    local_id: String,
    name: String,
    /// The line of facts under the name: the count of the agents that wait and
    /// the machine. It holds no word of a state, because the row shows that word
    /// at its right edge.
    facts: String,
    /// Where the new terminals of the workspace run.
    place: horizon_core::WorkspacePlace,
    /// How many panels of the workspace hold a tmux session of its machine, so
    /// closing them asks before it ends that work.
    machine_panels: usize,
    color: Color32,
    is_active: bool,
    detached: bool,
    capabilities: WorkspaceLayoutCapabilities,
    panels: Vec<SidebarPanelEntry>,
    /// The workspace's status dot and status line in the workspace list.
    row: Row,
}

#[derive(Clone)]
struct SidebarPanelEntry {
    id: PanelId,
    /// The panel's whole title.
    title: String,
    /// What the row says: an agent's task, or the title. Empty while the row
    /// does not show.
    label: String,
    kind: PanelKind,
    is_focused: bool,
    /// The one attention state of the panel: which agent waits, finished or works.
    attention: horizon_core::Attention,
}

/// The group of the sidebar whose caption took a press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Group {
    WorkItems,
    Machines,
}

impl SidebarActions {
    /// Takes the press on an icon control of a caption of `group`: the same action
    /// that the text button `New`, `Fit` or `Set up` asked for.
    pub(super) fn take_caption_press(&mut self, group: Group, press: caption::Press) {
        match (group, press) {
            (Group::WorkItems, caption::Press::Plus) => self.create_workspace = true,
            (Group::WorkItems, caption::Press::Fit) => self.fit_active_workspace = true,
            (Group::Machines, caption::Press::Plus) => self.press_set_up = true,
            // The group `Machines` carries the one control of a machine, and no fit.
            (Group::Machines, caption::Press::Fit) => {}
        }
    }
}

#[derive(Clone, Default)]
struct SidebarActions {
    create_workspace: bool,
    fit_active_workspace: bool,
    /// The Set up control of the machine section: rent a machine, add the
    /// Horizon server or a browser that stays signed in.
    press_set_up: bool,
    workspace_drop: Option<SidebarWorkspaceDropAction>,
    focus_panel: Option<PanelId>,
    pan_to_panel: Option<PanelId>,
    pan_to_workspace: Option<WorkspaceId>,
    detach_workspace: Option<WorkspaceId>,
    reattach_workspace: Option<WorkspaceId>,
    close_panel: Option<PanelId>,
    close_all_in_workspace: Option<WorkspaceId>,
    /// A click on the row of this machine: focus its terminal, or open one.
    connect_machine: Option<String>,
    /// A click on the row of an agent of a machine: go to its terminal, or open one.
    open_machine_agent: Option<horizon_core::machine_watch::MachineAgent>,
    /// A click on the row of a terminal of another computer: the computer and
    /// the panel that the follow panel shows.
    open_shared_terminal: Option<(String, String)>,
    /// A confirmed "Forget this machine" on the row of this machine.
    forget_machine: Option<String>,
    /// A press on `+ Rent a machine`, which sends the message of the button to
    /// the orchestrator of the active workspace.
    rent_machine: bool,
    remove_empty_workspace: Option<WorkspaceId>,
    /// Put a workspace on this computer, or on a machine of the list.
    set_workspace_place: Option<(WorkspaceId, horizon_core::WorkspacePlace)>,
    /// Put the active workspace on the machine whose row a person chose.
    put_active_workspace_here: Option<String>,
    clear_layout: Option<WorkspaceId>,
    arrange_layout: Option<(WorkspaceId, WorkspaceLayout)>,
}

#[derive(Clone, Copy)]
enum SidebarWorkspaceInsert {
    Before,
    After,
}

#[derive(Clone, Copy)]
struct SidebarWorkspaceDropAction {
    dragged_workspace_id: WorkspaceId,
    target_workspace_id: WorkspaceId,
    insert: SidebarWorkspaceInsert,
}

#[derive(Default)]
struct SidebarWorkspaceDragState {
    active_this_frame: bool,
    drop_requested: bool,
    drop_action: Option<SidebarWorkspaceDropAction>,
}

impl HorizonApp {
    fn has_attached_workspace(&self) -> bool {
        self.board
            .workspaces
            .iter()
            .any(|workspace| !self.workspace_is_detached(workspace.id))
    }

    pub(in crate::app) fn render_sidebar(&mut self, ctx: &Context) {
        if !self.sidebar_visible {
            return;
        }

        let viewport = util::viewport_local_rect(ctx);
        let sidebar_origin = Pos2::new(viewport.min.x, viewport.min.y + TOOLBAR_HEIGHT);
        let sidebar_width = SIDEBAR_WIDTH;
        let sidebar_size = Vec2::new(sidebar_width, viewport.height() - TOOLBAR_HEIGHT);
        self.refresh_sidebar_rows(std::time::Instant::now());
        self.refresh_machine_rows();
        let workspace_data = self.sidebar_workspace_data();
        let mut actions = SidebarActions::default();

        egui::Area::new(Id::unique("sidebar"))
            .fixed_pos(sidebar_origin)
            .constrain(false)
            .order(Order::Tooltip)
            .show(ctx, |ui| {
                Self::paint_sidebar_frame(ui, sidebar_origin, sidebar_size, sidebar_width);
                self.render_sidebar_contents(ui, &workspace_data, &mut actions);
            });

        self.apply_sidebar_actions(ctx, &actions);
    }

    fn sidebar_workspace_data(&self) -> Vec<WorkspaceSidebarEntry> {
        // One pass over the panels: rows read their panel from here, never by a
        // lookup on the board.
        let panels_by_id = self
            .board
            .panels
            .iter()
            .map(|panel| (panel.id, panel))
            .collect::<HashMap<_, _>>();
        // The dock of a visible front desk names the conversation of no workspace,
        // so no row of this list is the active one and no row opens its panels.
        // This is a rule of the drawing: the active workspace of the board and the
        // focus of the canvas stay as they are.
        let front_desk = self.orchestrator_dock.visible() && self.orchestrator_dock.front_desk().shows(&self.board);

        self.board
            .workspaces
            .iter()
            .map(|workspace| {
                let is_active = !front_desk && self.board.active_workspace == Some(workspace.id);
                // A work item shows its panel rows only while its fold is open.
                let shows_panels = self.sidebar_chrome.folds.is_open(&workspace.local_id);
                let panels = workspace
                    .panels
                    .iter()
                    .filter_map(|panel_id| panels_by_id.get(panel_id))
                    .map(|panel| self.sidebar_panel_entry(panel, shows_panels))
                    .collect::<Vec<_>>();

                let row = self.sidebar_row(workspace.id);
                WorkspaceSidebarEntry {
                    id: workspace.id,
                    local_id: workspace.local_id.clone(),
                    name: workspace.name.clone(),
                    facts: row.facts.clone(),
                    place: workspace.place.clone(),
                    machine_panels: workspace
                        .panels
                        .iter()
                        .filter_map(|panel_id| panels_by_id.get(panel_id))
                        .filter(|panel| panel.machine_session().is_some())
                        .count(),
                    color: theme::workspace_accent(workspace.color_idx),
                    is_active,
                    detached: self.workspace_is_detached(workspace.id),
                    capabilities: self.workspace_layout_capabilities(workspace.id),
                    panels,
                    row,
                }
            })
            .collect()
    }

    /// The sidebar entry of `panel`, with its row label when the row `shows`.
    fn sidebar_panel_entry(&self, panel: &horizon_core::Panel, shows: bool) -> SidebarPanelEntry {
        let title = panel.display_title().into_owned();
        let label = if shows {
            self.sidebar_panel_label(panel, &title)
        } else {
            String::new()
        };
        SidebarPanelEntry {
            id: panel.id,
            title,
            label,
            kind: panel.kind,
            is_focused: self.board.focused == Some(panel.id),
            attention: self.panel_attention_of(panel),
        }
    }

    fn paint_sidebar_frame(ui: &mut egui::Ui, sidebar_origin: Pos2, sidebar_size: Vec2, sidebar_width: f32) {
        ui.set_min_size(sidebar_size);
        ui.set_max_size(sidebar_size);
        ui.painter().rect_filled(
            Rect::from_min_size(sidebar_origin, sidebar_size),
            CornerRadius::ZERO,
            theme::BG_ELEVATED(),
        );
        ui.painter().line_segment(
            [
                Pos2::new(sidebar_origin.x + sidebar_width, sidebar_origin.y),
                Pos2::new(sidebar_origin.x + sidebar_width, sidebar_origin.y + sidebar_size.y),
            ],
            Stroke::new(1.0_f32, theme::BORDER_SUBTLE()),
        );
    }

    fn render_sidebar_contents(
        &mut self,
        ui: &mut egui::Ui,
        workspace_data: &[WorkspaceSidebarEntry],
        actions: &mut SidebarActions,
    ) {
        // The footer keeps its place at the bottom of the sidebar, so the groups
        // above it scroll in the room that is left.
        let scroll_height = (ui.available_height() - footer::HEIGHT).max(0.0);
        let mut drag_state = SidebarWorkspaceDragState::default();
        egui::ScrollArea::vertical()
            .max_height(scroll_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                // Rows, captions and chips set their own gaps.
                ui.spacing_mut().item_spacing.y = 0.0;

                self.render_front_desk_row(ui);
                self.render_sidebar_inbox(ui, actions);
                self.render_sidebar_workspaces(ui, workspace_data, actions, &mut drag_state);
                self.render_sidebar_machines(ui, actions);
                self.render_sidebar_agents(ui);
                self.render_sidebar_computers(ui, actions);
            });

        if drag_state.drop_requested {
            actions.workspace_drop = drag_state.drop_action;
            self.sidebar_drag_workspace = None;
        } else if !drag_state.active_this_frame && !ui.ctx().input(|input| input.pointer.primary_down()) {
            self.sidebar_drag_workspace = None;
        }

        let room = ui.available_height() - footer::HEIGHT;
        if room > 0.0 {
            ui.add_space(room);
        }
        let width = ui.available_width();
        let line = self.server_line();
        let reason = self.server_line_reason();
        footer::render(ui, width, line, reason, actions);
    }

    fn render_sidebar_workspace(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &WorkspaceSidebarEntry,
        actions: &mut SidebarActions,
        drag_state: &mut SidebarWorkspaceDragState,
    ) {
        let row_rect = ui.allocate_space(Vec2::new(ui.available_width(), rows::ROW_HEIGHT)).1;
        let hovered = ui.rect_contains_pointer(row_rect);
        let open = self.sidebar_chrome.folds.is_open(&workspace.local_id);
        let mark = fold::Mark::of(open, hovered, !workspace.panels.is_empty());
        let word = rows::word_of(&workspace.row);
        let parts = rows::paint(
            ui,
            row_rect,
            workspace,
            word,
            mark,
            hovered,
            self.sidebar_drag_workspace == Some(workspace.id),
        );
        // The control of the fold is a press target of its own, so the row takes no
        // press under it and a press there opens or folds the panel rows alone.
        if let Some(rect) = parts.fold
            && fold::render(ui, rect, &workspace.local_id, mark)
        {
            let accordion = self.template_config.features.sidebar_accordion;
            self.sidebar_chrome.folds.toggle(&workspace.local_id, accordion);
        }

        // The row's own press stops at the fold control.
        let click_right = parts.fold.map_or(row_rect.max.x, |rect| rect.min.x);
        let click_rect = Rect::from_min_max(row_rect.min, Pos2::new(click_right, row_rect.max.y));
        let row_response = ui.interact(
            click_rect,
            ui.make_persistent_id(("sidebar_ws_click", workspace.id.0)),
            Sense::click_and_drag(),
        );
        row_response.widget_info(|| {
            let spoken = text::workspace_spoken(&workspace.name, None, workspace.detached);
            egui::WidgetInfo::labeled(egui::Role::Button, true, spoken)
        });
        // A name or a line that the row cuts shows its whole text in a hover. The
        // hover goes after the row: egui hovers a widget that only senses hover
        // when it lies above the row's click target.
        if parts.name_elided || parts.line_elided {
            let text = if parts.name_elided {
                &workspace.name
            } else {
                &workspace.facts
            };
            ui.interact(
                parts.text,
                ui.make_persistent_id(("sidebar_ws_text", workspace.id.0)),
                Sense::hover(),
            )
            .on_hover_text(text);
        }
        let row_clicked = row_response.clicked();
        self.handle_sidebar_workspace_drag(ui, workspace, row_rect, &row_response, drag_state, hovered);

        if row_clicked {
            // A press on a workspace row shows that workspace in the dock, also
            // while the row is already the active one: the person asks for the
            // conversation of that workspace.
            self.orchestrator_dock.front_desk_mut().show_workspace();
            // With the accordion the panel rows are collapsed, so selecting a
            // workspace reveals its selected panel instead of panning to the
            // whole workspace. Single-panel workspaces behave the same in
            // both modes; flat multi-panel rows keep panning to bounds.
            let accordion = self.template_config.features.sidebar_accordion;
            match self.workspace_row_reveal(accordion, workspace.id, &workspace.panels) {
                Some(panel_id) => {
                    actions.focus_panel = Some(panel_id);
                    actions.pan_to_panel = Some(panel_id);
                }
                None => actions.pan_to_workspace = Some(workspace.id),
            }
        }
        self.show_workspace_context_menu(&row_response, workspace, actions);

        if open {
            let count = workspace.panels.len();
            for (index, panel) in workspace.panels.iter().enumerate() {
                self.render_sidebar_panel(ui, workspace, panel, index + 1 == count, actions);
            }
        }
        ui.add_space(2.0);
    }

    /// The panel a workspace-row click reveals. Accordion rows (where panel
    /// rows are collapsed) and single-panel workspaces reveal a panel;
    /// flat multi-panel rows return `None` so the click keeps its original
    /// pan-to-workspace-bounds behavior.
    fn workspace_row_reveal(
        &self,
        accordion: bool,
        workspace_id: WorkspaceId,
        panels: &[SidebarPanelEntry],
    ) -> Option<PanelId> {
        if !accordion && panels.len() > 1 {
            return None;
        }
        self.workspace_reveal_panel(workspace_id, panels)
    }

    /// The panel a workspace-row click reveals: the focused panel when it
    /// belongs to the workspace, otherwise the workspace's first panel.
    fn workspace_reveal_panel(&self, workspace_id: WorkspaceId, panels: &[SidebarPanelEntry]) -> Option<PanelId> {
        self.board
            .focused
            .filter(|panel_id| self.board.panel_workspace_id(*panel_id) == Some(workspace_id))
            .or_else(|| panels.first().map(|panel| panel.id))
    }

    fn handle_sidebar_workspace_drag(
        &mut self,
        ui: &egui::Ui,
        workspace: &WorkspaceSidebarEntry,
        row_rect: Rect,
        row_response: &egui::Response,
        drag_state: &mut SidebarWorkspaceDragState,
        click_target_hovered: bool,
    ) {
        if row_response.drag_started() || row_response.dragged() {
            self.sidebar_drag_workspace = Some(workspace.id);
            drag_state.active_this_frame = true;
        }
        if self.sidebar_drag_workspace == Some(workspace.id) && row_response.drag_stopped() {
            drag_state.active_this_frame = true;
            drag_state.drop_requested = true;
        }

        if let Some(dragged_workspace_id) = self.sidebar_drag_workspace.filter(|id| *id != workspace.id)
            && list::accepts_drop(workspace)
            && let Some(pointer_pos) = ui.ctx().pointer_interact_pos()
            && row_rect.expand2(Vec2::new(0.0, 4.0)).contains(pointer_pos)
        {
            let insert = if pointer_pos.y <= row_rect.center().y {
                SidebarWorkspaceInsert::Before
            } else {
                SidebarWorkspaceInsert::After
            };
            drag_state.drop_action = Some(SidebarWorkspaceDropAction {
                dragged_workspace_id,
                target_workspace_id: workspace.id,
                insert,
            });
            rows::paint_workspace_drop_indicator(ui, row_rect, insert, workspace.color);
        }

        if self.sidebar_drag_workspace == Some(workspace.id) && row_response.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        } else if click_target_hovered {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }
    }

    fn render_sidebar_panel(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &WorkspaceSidebarEntry,
        panel: &SidebarPanelEntry,
        last: bool,
        actions: &mut SidebarActions,
    ) {
        let row_rect = ui
            .allocate_space(Vec2::new(ui.available_width(), panel_rows::PANEL_ROW_HEIGHT))
            .1;
        let label = &panel.label;
        let hovered = ui.rect_contains_pointer(row_rect);
        let close_rect = panel_rows::close_rect(row_rect);
        // The close button always takes clicks and tells a screen reader its name,
        // but shows only on the hovered or focused row, or while the row or the
        // button has keyboard focus.
        let close = ui
            .interact(
                close_rect,
                ui.make_persistent_id(("sidebar_panel_close", panel.id.0)),
                Sense::click(),
            )
            .on_hover_text("Close panel");
        close.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Close panel"));
        let row_id = sidebar_panel_row_id(panel.id);
        let row_focused = ui.memory(|memory| memory.has_focus(row_id));
        let close_shows = hovered || panel.is_focused || row_focused || close.hovered() || close.has_focus();
        // A pointer press closes only a button that the person could see before the
        // press began: a tap on the hidden button of another row focuses that row
        // instead. The decision is kept from pointer-down until the release.
        let shown_id = ui.make_persistent_id(("sidebar_panel_close_shown", panel.id.0));
        let armed_id = ui.make_persistent_id(("sidebar_panel_close_armed", panel.id.0));
        let shown_before = ui.ctx().data(|data| data.get_temp::<bool>(shown_id)).unwrap_or(false);
        // A press and its release can land in one frame: a quick click of a person is
        // inside one frame of 55 ms, and the pointer clears its press origin on the
        // release. The press of this frame is read from its events as well, so a
        // press on the mark arms it at any frame rate.
        let (pressed, press_origin) = ui.input(|input| {
            let origin = input.pointer.press_origin().or_else(|| {
                input.events.iter().find_map(|event| match event {
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        ..
                    } => Some(*pos),
                    _ => None,
                })
            });
            (input.pointer.any_pressed(), origin)
        });
        if pressed && press_origin.is_some_and(|origin| close_rect.contains(origin)) {
            ui.ctx().data_mut(|data| data.insert_temp(armed_id, shown_before));
        }
        ui.ctx().data_mut(|data| data.insert_temp(shown_id, close_shows));
        let armed = ui.ctx().data(|data| data.get_temp::<bool>(armed_id)).unwrap_or(false);
        let by_pointer = ui.input(|input| input.pointer.any_click());
        let close_clicked = close.clicked() && (armed || close.has_focus() || !by_pointer);
        let blind_tap = close.clicked() && !close_clicked;
        let look = panel_rows::PanelRowLook {
            guide_x: panel_rows::guide_x(rows::card(row_rect)),
            accent: workspace.color,
            kind: panel.kind,
            focused: panel.is_focused,
            label,
            attention: panel.attention,
        };
        let title_elided = panel_rows::paint_panel_row(
            ui,
            row_rect,
            &look,
            panel_rows::PanelRowState {
                hovered,
                close: close_shows.then(|| close.hovered() || close.has_focus()),
                last,
            },
        );

        let row_click_rect = Rect::from_min_max(row_rect.min, Pos2::new(close_rect.min.x, row_rect.max.y));
        let row_response = ui.interact(row_click_rect, row_id, Sense::click());
        row_response.widget_info(|| {
            let spoken = text::panel_spoken(panel.kind, label, panel.attention);
            egui::WidgetInfo::labeled(egui::Role::Button, true, spoken)
        });
        // The whole title, when the row cuts it or shows only the agent's task,
        // and the sentence of the state while the pointer is on the row.
        let state = (hovered && panel.attention.word().is_some()).then(|| panel.attention.sentence());
        let row_response = match (title_elided || *label != panel.title, state) {
            (true, Some(sentence)) => row_response.on_hover_text(format!("{}\n{sentence}", panel.title)),
            (true, None) => row_response.on_hover_text(&panel.title),
            (false, Some(sentence)) => row_response.on_hover_text(sentence),
            (false, None) => row_response,
        };
        if hovered {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }

        if close_clicked {
            actions.close_panel = Some(panel.id);
        } else if row_response.clicked() || blind_tap {
            // A click on the row is a look at the panel, so its state clears.
            self.note_panel_attention_look(panel.id);
            actions.focus_panel = Some(panel.id);
            actions.pan_to_panel = Some(panel.id);
        }

        self.show_sidebar_panel_context_menu(&row_response, workspace, panel.id, panel.kind, actions);
        ui.add_space(1.0);
    }

    /// The presses of the machine section: Set up, and the button that rents a
    /// machine for the active workspace.
    fn apply_machine_section_presses(&mut self, ctx: &Context, actions: &SidebarActions) {
        if actions.press_set_up {
            self.press_set_up(ctx, horizon_core::SetupGoal::Choose);
        }
        if actions.rent_machine {
            self.rent_a_machine(ctx);
        }
    }

    /// The actions of a press on a machine or on an agent of a machine.
    fn apply_machine_actions(&mut self, ctx: &Context, actions: &SidebarActions) {
        if let Some(name) = actions.forget_machine.clone() {
            self.forget_machine(&name, ctx);
        }
        if let Some(name) = actions.connect_machine.clone() {
            self.connect_machine(&name, ctx);
        }
        if let Some(agent) = actions.open_machine_agent.clone() {
            self.open_machine_agent(&agent, ctx);
        }
        if let Some((device, panel)) = &actions.open_shared_terminal {
            self.open_shared_terminal(device, panel, ctx);
        }
    }

    /// What the row of `panel` says: an agent's task, or `display_title`.
    pub(in crate::app) fn sidebar_panel_label(&self, panel: &horizon_core::Panel, display_title: &str) -> String {
        let own_name = (!text::name_labels_agent(
            panel.kind,
            &panel.title,
            panel.name_is_custom(),
            &self.template_config.presets,
        ))
        .then_some(panel.title.as_str());
        text::panel_label(panel.kind, own_name, display_title, &panel.terminal_title)
    }

    /// Puts a workspace in the place that a menu chose, which its new terminals
    /// follow and its open panels do not.
    fn apply_workspace_place_actions(&mut self, actions: &SidebarActions) {
        if let Some((workspace_id, place)) = actions.set_workspace_place.clone()
            && self.board.set_workspace_place(workspace_id, place)
        {
            self.mark_runtime_dirty();
        }
        if let Some(name) = actions.put_active_workspace_here.clone()
            && let Some(workspace_id) = self.board.active_workspace
            && self
                .board
                .set_workspace_place(workspace_id, horizon_core::WorkspacePlace::Machine(name))
        {
            self.mark_runtime_dirty();
        }
    }

    fn apply_sidebar_actions(&mut self, ctx: &Context, actions: &SidebarActions) {
        if actions.create_workspace {
            self.create_new_workspace(ctx, None);
        }
        if actions.fit_active_workspace {
            let _ = self.fit_active_workspace(ctx);
        }
        self.apply_machine_section_presses(ctx, actions);
        let workspace_collision_ids = self.workspace_collision_scope(None);
        let workspace_drop_target = actions.workspace_drop.map(|drop| {
            let moved = if sidebar_workspace_drop_should_dock(self.workspace_is_detached(drop.target_workspace_id)) {
                self.board.move_workspace_beside_in_scope(
                    drop.dragged_workspace_id,
                    drop.target_workspace_id,
                    sidebar_workspace_insert_dock_side(drop.insert),
                    &workspace_collision_ids,
                )
            } else {
                false
            };
            let reordered = match drop.insert {
                SidebarWorkspaceInsert::Before => self
                    .board
                    .move_workspace_before(drop.dragged_workspace_id, drop.target_workspace_id),
                SidebarWorkspaceInsert::After => self
                    .board
                    .move_workspace_after(drop.dragged_workspace_id, drop.target_workspace_id),
            };
            if moved || reordered {
                self.mark_runtime_dirty();
            }
            drop.dragged_workspace_id
        });
        self.apply_workspace_place_actions(actions);
        self.apply_machine_actions(ctx, actions);
        if let Some(panel_id) = actions.focus_panel {
            // A click on a row of an open machine or of a panel is the person's,
            // so the terminal of that panel takes the next key.
            self.give_the_keyboard_to(panel_id);
        }

        let pan_to_panel_workspace = actions
            .pan_to_panel
            .and_then(|panel_id| self.board.panel(panel_id).map(|panel| panel.workspace_id));
        let pan_workspace_id = if workspace_drop_target.is_some() {
            workspace_drop_target
        } else if pan_to_panel_workspace.is_some() {
            pan_to_panel_workspace
        } else {
            actions.pan_to_workspace
        };
        if let Some(workspace_id) = pan_workspace_id {
            if actions.pan_to_panel.is_none() {
                self.board.focus_workspace(workspace_id);
            }
            if self.focus_workspace_window(ctx, workspace_id) {
                if let Some(panel_id) = actions.focus_panel {
                    self.board.focus(panel_id);
                }
            } else if let Some(panel_id) = actions.pan_to_panel {
                // Attached canvas: reveal the clicked panel (zooming out when
                // needed) instead of panning to the whole workspace bounds.
                self.reveal_panel_visible(ctx, panel_id);
            } else if let Some((pos, size)) = self.workspace_focus_frame(workspace_id) {
                self.pan_to_canvas_pos_aligned(ctx, pos, size, true);
            }
        }

        if let Some(workspace_id) = actions.detach_workspace {
            self.detach_workspace(workspace_id);
        }
        if let Some(workspace_id) = actions.reattach_workspace {
            self.reattach_workspace(ctx, workspace_id);
        }

        if let Some(panel_id) = actions.close_panel {
            self.request_close_panel(panel_id);
        }
        if let Some(workspace_id) = actions.close_all_in_workspace {
            self.close_workspace_panels(workspace_id);
        }
        if let Some(workspace_id) = actions.remove_empty_workspace {
            let local_id = self
                .board
                .workspace(workspace_id)
                .map(|workspace| workspace.local_id.clone());
            if self.board.remove_empty_workspace(workspace_id) {
                self.mark_runtime_dirty();
                // A workspace that goes takes its conversation and its draft with
                // it: a later row with the same name is another workspace.
                if let Some(local_id) = local_id {
                    self.sidebar_chrome.folds.close(&local_id);
                    self.forget_orchestrator_conversation(&local_id);
                }
            }
        }
        if let Some(workspace_id) = actions.clear_layout
            && self.workspace_can_arrange_panels(workspace_id)
            && self.board.clear_workspace_layout(workspace_id)
        {
            self.mark_runtime_dirty();
        }
        if let Some((workspace_id, layout)) = actions.arrange_layout
            && self.workspace_can_arrange_panels(workspace_id)
        {
            self.board.arrange_workspace(workspace_id, layout);
            self.mark_runtime_dirty();
        }
    }
}

/// The rows that the sidebar draws, which a test reads: the `local_id` of each
/// workspace that it draws as active, and the number of panel rows that it opens.
#[cfg(test)]
impl HorizonApp {
    pub(in crate::app) fn sidebar_drawn_rows(&self) -> (Vec<String>, usize) {
        let data = self.sidebar_workspace_data();
        let active = data
            .iter()
            .filter(|entry| entry.is_active)
            .filter_map(|entry| self.board.workspace(entry.id))
            .map(|workspace| workspace.local_id.clone())
            .collect();
        let panels = data
            .iter()
            .map(|entry| entry.panels.iter().filter(|panel| !panel.label.is_empty()).count())
            .sum();
        (active, panels)
    }
}

/// The widget id of a panel row: global, so that the close button can read the
/// row's keyboard focus before the row is registered.
fn sidebar_panel_row_id(panel: PanelId) -> Id {
    Id::unique(("sidebar_panel_click", panel.0))
}

fn sidebar_workspace_insert_dock_side(insert: SidebarWorkspaceInsert) -> WorkspaceDockSide {
    match insert {
        SidebarWorkspaceInsert::Before => WorkspaceDockSide::Left,
        SidebarWorkspaceInsert::After => WorkspaceDockSide::Right,
    }
}

fn sidebar_workspace_drop_should_dock(target_detached: bool) -> bool {
    !target_detached
}

#[cfg(test)]
mod tests;
