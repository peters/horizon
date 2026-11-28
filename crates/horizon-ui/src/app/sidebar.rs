pub(in crate::app) mod agents;
mod apply;
pub(in crate::app) mod caption;
mod computers;
mod fold;
mod footer;
mod front_desk_row;
pub(in crate::app) mod inbox;
mod list;
pub(in crate::app) mod machines;
pub(in crate::app) mod menus;
mod mirror_rows;
mod new_workspace;
mod panel_row;
mod panel_rows;
mod place;
mod rent_machine;
mod rich_menu;
pub(in crate::app) mod rows;
mod text;
mod toolbar;
mod workspace_row;

pub(in crate::app) use caption::Counts;
pub(in crate::app) use fold::Folds;
pub(in crate::app) use list::ListCache;
pub(in crate::app) use rent_machine::Dashes;

use std::collections::HashMap;

use egui::{Color32, Context, CornerRadius, Id, Order, Pos2, Rect, Stroke, Vec2};
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
    /// A press on the row of a mirrored session of another computer: the session
    /// that the dock shows.
    mirror_session: Option<crate::app::session_mirror::MirrorKey>,
    /// A press on the row `and <N> more` of a publishing computer.
    mirror_more: Option<String>,
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
