//! The machine section of the sidebar: one row for each machine that an agent
//! registered with this Horizon.
//!
//! A row says the state of the terminal that this machine opened, and nothing
//! when no terminal of it is open: Horizon does not poll a machine, so an empty
//! dot is the honest answer. The row text is read again only when the list or a
//! terminal of a machine changes, so an idle frame reads no machine and builds
//! no string.

use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use egui::{CursorIcon, Rect, Sense, Stroke, Vec2, pos2, vec2};
use horizon_core::{PanelId, SshConnectionStatus, WorkspaceId};

use crate::theme;

use super::super::HorizonApp;
use super::text;
use super::{SidebarActions, rows};

/// The height of one machine row.
pub(super) const ROW_HEIGHT: f32 = 38.0;
/// The side of the state dot in front of a machine name.
pub(super) const DOT_SIDE: f32 = 9.0;
/// How long the reason of a refused machine stays in the section.
pub(super) const NOTICE: Duration = Duration::from_secs(10);
/// The circular radius of a machine row behind the pointer.
const CARD_RADIUS: u8 = 10;

/// The state of the terminal of one machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum MachineState {
    /// No terminal of this machine is open.
    #[default]
    Closed,
    Connecting,
    Connected,
    Disconnected,
}

impl MachineState {
    /// The state that a terminal of this machine reports now.
    fn of(status: Option<SshConnectionStatus>) -> Self {
        match status {
            None => Self::Closed,
            Some(SshConnectionStatus::Connecting) => Self::Connecting,
            Some(SshConnectionStatus::Connected) => Self::Connected,
            Some(SshConnectionStatus::Disconnected) => Self::Disconnected,
        }
    }

    /// What a screen reader and a hover say about this state.
    #[must_use]
    pub(in crate::app) const fn spoken(self) -> &'static str {
        match self {
            Self::Closed => "no terminal open",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
        }
    }
}

/// One row of the machine section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct MachineRow {
    pub(in crate::app) name: String,
    /// The provider line as the agent reported it, or nothing.
    pub(in crate::app) provider: Option<String>,
    pub(in crate::app) state: MachineState,
    /// The frame of the terminal of this machine, when one is open.
    pub(in crate::app) panel: Option<PanelId>,
    pub(in crate::app) workspace: Option<WorkspaceId>,
}

/// The rows of the machine section, with the list and the terminal states that
/// they were read from.
#[derive(Default)]
pub(in crate::app) struct MachineRows {
    rows: Vec<MachineRow>,
    read_from: Option<u64>,
}

impl MachineRows {
    /// The rows of the last read.
    pub(in crate::app) fn rows(&self) -> &[MachineRow] {
        &self.rows
    }

    /// What the rows were read from, so a test can see that nothing was read
    /// again.
    #[cfg(test)]
    pub(in crate::app) fn read_from(&self) -> Option<u64> {
        self.read_from
    }
}

impl HorizonApp {
    /// Reads the rows again when the machine list or a terminal of a machine
    /// changed. Without a change this costs one hash of the records and one pass
    /// over the frames that are tied to a machine.
    pub(in crate::app) fn refresh_machine_rows(&mut self) {
        let read_from = machine_fingerprint(self);
        if self.machine_rows.read_from == Some(read_from) {
            return;
        }
        let rows = self
            .machine_list
            .records()
            .iter()
            .map(|machine| {
                let panel = self
                    .board
                    .panels
                    .iter()
                    .find(|panel| panel.machine() == Some(machine.name.as_str()));
                MachineRow {
                    name: machine.name.clone(),
                    provider: machine.provider.clone(),
                    state: MachineState::of(panel.and_then(horizon_core::Panel::ssh_status)),
                    panel: panel.map(|panel| panel.id),
                    workspace: panel.map(|panel| panel.workspace_id),
                }
            })
            .collect();
        self.machine_rows = MachineRows {
            rows,
            read_from: Some(read_from),
        };
    }

    /// Says why a machine action opened nothing, in the machine section.
    pub(in crate::app) fn set_machine_notice(&mut self, line: String) {
        self.machine_notice = Some(line);
        self.machine_notice_until = Some(Instant::now() + NOTICE);
    }

    /// Draws the machine section below the workspaces, when this Horizon keeps a
    /// machine.
    pub(super) fn render_sidebar_machines(&mut self, ui: &mut egui::Ui, actions: &mut SidebarActions) {
        if self.machine_rows.rows().is_empty() {
            self.machine_notice = None;
            return;
        }
        if self.machine_notice_until.is_some_and(|until| Instant::now() >= until) {
            self.machine_notice = None;
            self.machine_notice_until = None;
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            ui.label(text::line(
                ui,
                "Machines",
                13.0,
                text::SEMIBOLD,
                theme::FG_SOFT(),
                120.0,
            ));
        });
        ui.add_space(2.0);

        // The rows are borrowed out for the pass, because a press changes the
        // actions and the board; nothing in the pass reads the cache again.
        let rows = std::mem::take(&mut self.machine_rows);
        for row in rows.rows() {
            self.render_sidebar_machine(ui, row, actions);
        }
        self.machine_rows = rows;

        if let Some(notice) = self.machine_notice.clone() {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                ui.label(text::line(
                    ui,
                    &notice,
                    11.0,
                    text::REGULAR,
                    theme::PALETTE_RED(),
                    ui.available_width() - 18.0,
                ));
            });
        }
        ui.add_space(6.0);
    }

    /// Draws one machine row and turns a press on it into an action.
    fn render_sidebar_machine(&mut self, ui: &mut egui::Ui, row: &MachineRow, actions: &mut SidebarActions) {
        let rect = ui.allocate_space(Vec2::new(ui.available_width(), ROW_HEIGHT)).1;
        let card = self::card(rect);
        let hovered = ui.rect_contains_pointer(rect);
        if hovered {
            ui.painter()
                .rect_filled(card, CARD_RADIUS, theme::alpha(theme::PANEL_BG_ALT(), 170));
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        let dot = dot_center(card);
        paint_machine_dot(ui.painter(), dot, row.state);

        let text_left = dot.x + DOT_SIDE / 2.0 + 10.0;
        let text_right = card.max.x - 8.0;
        let name = text::line(ui, &row.name, 13.0, text::MEDIUM, theme::FG(), text_right - text_left);
        let name_rect = Rect::from_min_size(pos2(text_left, card.min.y + 6.0), name.size());
        let elided = name.elided;
        ui.painter().galley(name_rect.min, name, theme::FG());
        if let Some(provider) = row.provider.as_deref() {
            let galley = text::line(
                ui,
                provider,
                11.0,
                text::REGULAR,
                text::secondary(),
                text_right - text_left,
            );
            ui.painter()
                .galley(pos2(text_left, card.min.y + 21.0), galley, text::secondary());
        }

        let response = ui.interact(
            rect,
            ui.make_persistent_id(("sidebar_machine", row.name.as_str())),
            Sense::click(),
        );
        let spoken = format!("{}, {}", row.name, row.state.spoken());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &spoken));
        let response = if elided {
            response.on_hover_text(format!("{}\n{spoken}", row.name))
        } else {
            response.on_hover_text(spoken)
        };
        if response.clicked() {
            machine_click(row, actions);
        }
        self.show_machine_context_menu(&response, row, actions);
    }
}

/// What a press on a machine row asks for: the terminal of a machine that is
/// already open, focused and in its own workspace, or a terminal that Horizon
/// opens in the active workspace.
pub(super) fn machine_click(row: &MachineRow, actions: &mut SidebarActions) {
    match row.panel {
        Some(panel) => {
            actions.focus_panel = Some(panel);
            actions.pan_to_panel = Some(panel);
        }
        None => actions.connect_machine = Some(row.name.clone()),
    }
}

/// The card of a machine row: the row without the sidebar inset.
pub(super) fn card(row: Rect) -> Rect {
    row.shrink2(vec2(rows::ROW_INSET, 0.0))
}

/// The center of the state dot of a machine row.
pub(super) fn dot_center(card: Rect) -> egui::Pos2 {
    pos2(card.min.x + rows::TILE_INSET + DOT_SIDE / 2.0, card.min.y + 12.0)
}

/// Paints the state dot: hollow when no terminal of the machine is open, and a
/// filled green, red or soft dot for the state of the terminal that is open.
pub(super) fn paint_machine_dot(painter: &egui::Painter, center: egui::Pos2, state: MachineState) {
    let radius = DOT_SIDE / 2.0 - 0.5;
    match state {
        MachineState::Closed => {
            painter.circle_stroke(center, radius, Stroke::new(1.0_f32, theme::FG_DIM()));
        }
        MachineState::Connecting => {
            painter.circle_stroke(center, radius, Stroke::new(1.0_f32, theme::FG_SOFT()));
        }
        MachineState::Connected => {
            painter.circle_filled(center, radius, theme::PALETTE_GREEN());
        }
        MachineState::Disconnected => {
            painter.circle_filled(center, radius, theme::PALETTE_RED());
        }
    }
}

/// The fields of the list and of the terminals that the rows read from, so a
/// frame knows without building a row whether it must read them again.
fn machine_fingerprint(app: &HorizonApp) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for machine in app.machine_list.records() {
        machine.name.hash(&mut hasher);
        machine.host.hash(&mut hasher);
        machine.user.hash(&mut hasher);
        machine.port.hash(&mut hasher);
        machine.key.hash(&mut hasher);
        machine.session.hash(&mut hasher);
        machine.provider.hash(&mut hasher);
    }
    for panel in app.board.panels.iter().filter(|panel| panel.machine().is_some()) {
        panel.machine().hash(&mut hasher);
        panel.id.0.hash(&mut hasher);
        panel.workspace_id.0.hash(&mut hasher);
        panel.ssh_status().map(status_code).hash(&mut hasher);
    }
    hasher.finish()
}

/// The state of a terminal as one number, for the fingerprint.
const fn status_code(status: SshConnectionStatus) -> u8 {
    match status {
        SshConnectionStatus::Connecting => 1,
        SshConnectionStatus::Connected => 2,
        SshConnectionStatus::Disconnected => 3,
    }
}

#[cfg(test)]
mod tests;
