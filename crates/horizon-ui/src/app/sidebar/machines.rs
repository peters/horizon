//! The machine section of the sidebar: one row for each machine that an agent
//! registered with this Horizon.
//!
//! A row says the state of the terminal that this machine opened, and nothing
//! when no terminal of it is open: Horizon does not poll a machine, so an empty
//! dot is the honest answer. A machine that follows a server carries a quiet
//! `pending` mark while the server has not decided its last change, and the
//! section says the state of that link above its rows. The row text is read again
//! only when the list, a terminal of a machine, the queue or the link changes, so
//! an idle frame reads no machine and builds no string.

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
/// The side of the close control of the line that stays until a person closes it.
const CLOSE_SIDE: f32 = 18.0;
/// The corner radius of that control.
const CLOSE_RADIUS: u8 = 5;

/// The clock of the short notice of the section: the clock of the computer, or the
/// clock of a test, which moves forward instead of waiting for ten seconds.
#[derive(Clone, Copy)]
pub(in crate::app) struct NoticeClock {
    /// How far ahead of the computer this clock reads, which only a test sets.
    #[cfg(test)]
    pub(in crate::app) ahead: Duration,
}

impl Default for NoticeClock {
    fn default() -> Self {
        Self {
            #[cfg(test)]
            ahead: Duration::ZERO,
        }
    }
}

impl NoticeClock {
    /// What this clock reads now.
    fn now(self) -> Instant {
        #[cfg(test)]
        {
            Instant::now() + self.ahead
        }
        #[cfg(not(test))]
        {
            Instant::now()
        }
    }
}
/// The quiet mark of a machine whose last change the server has not decided.
pub(super) const PENDING_MARK: &str = "pending";
/// The circular radius of a machine row behind the pointer.
const CARD_RADIUS: u8 = 10;

/// The state of the terminal of one machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum MachineState {
    /// No terminal of this machine is open.
    #[default]
    Closed,
    Connecting,
    /// The link was lost, and Horizon waits to reach the machine again.
    Reconnecting,
    Connected,
    Disconnected,
}

impl MachineState {
    /// The state that a terminal of this machine reports now.
    fn of(status: Option<SshConnectionStatus>) -> Self {
        match status {
            None => Self::Closed,
            Some(SshConnectionStatus::Connecting) => Self::Connecting,
            Some(SshConnectionStatus::Reconnecting) => Self::Reconnecting,
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
            Self::Reconnecting => "reconnecting",
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
    /// Whether the server has not decided the last change to this machine yet.
    pub(in crate::app) pending: bool,
    /// The frame of the terminal of this machine, when one is open.
    pub(in crate::app) panel: Option<PanelId>,
    pub(in crate::app) workspace: Option<WorkspaceId>,
}

/// The rows of the machine section, with the list and the terminal states that
/// they were read from.
#[derive(Default)]
pub(in crate::app) struct MachineRows {
    rows: Vec<MachineRow>,
    /// The line of the link to the server, built once for its state, or `None`
    /// when this Horizon keeps its machine list on this computer alone.
    server_line: Option<String>,
    read_from: Option<u64>,
}

impl MachineRows {
    /// The rows of the last read.
    pub(in crate::app) fn rows(&self) -> &[MachineRow] {
        &self.rows
    }

    /// The state line of the link to the server, when the settings name one.
    pub(in crate::app) fn server_line(&self) -> Option<&str> {
        self.server_line.as_deref()
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
        let pending = self.pending_machines();
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
                    pending: pending.contains(machine.name.as_str()),
                    panel: panel.map(|panel| panel.id),
                    workspace: panel.map(|panel| panel.workspace_id),
                }
            })
            .collect();
        // The line is built on the frame that the state changes and on no other.
        let server_line = self.server_sync.as_ref().map(|sync| sync.line().text());
        self.machine_rows = MachineRows {
            rows,
            server_line,
            read_from: Some(read_from),
        };
    }

    /// Says why an action of the person opened nothing, in the machine section.
    ///
    /// It is the one short notice of this window: a machine action, a panel
    /// that cannot start and a browser engine that ends at once all say their
    /// reason here, in the danger tone, for [`NOTICE`].
    pub(in crate::app) fn set_sidebar_notice(&mut self, line: String) {
        self.sidebar_notice = Some(line);
        self.sidebar_notice_until = Some(self.notice_clock.now() + NOTICE);
    }

    /// Says one line that stays until the person closes it, in the machine section.
    ///
    /// It has its own close control and no timer, because it names something that
    /// is gone for good: the panels that an opened session left out. A short notice
    /// of an action beside it neither replaces it nor ends with it.
    pub(in crate::app) fn set_sidebar_sticky_notice(&mut self, line: String) {
        self.sidebar_sticky_notice = Some(line);
    }

    /// Forgets the line that stays until the person closes it, which is what its
    /// close control asks for.
    pub(in crate::app) fn dismiss_sidebar_sticky_notice(&mut self) {
        self.sidebar_sticky_notice = None;
    }

    /// Draws the machine section below the workspaces, when this Horizon keeps a
    /// machine.
    pub(super) fn render_sidebar_machines(&mut self, ui: &mut egui::Ui, actions: &mut SidebarActions) {
        let now = self.notice_clock.now();
        if self.sidebar_notice_until.is_some_and(|until| now >= until) {
            self.sidebar_notice = None;
            self.sidebar_notice_until = None;
        }
        // A reason that an action of the person opened nothing stays visible
        // after the row is forgotten, because it is the reason the person has to
        // act on. The section appears with its first machine, or with the line of
        // a server that this Horizon follows: the line says that the list comes
        // from there before the first machine arrives.
        if self.machine_rows.rows().is_empty()
            && self.machine_rows.server_line().is_none()
            && self.sidebar_notice.is_none()
            && self.sidebar_sticky_notice.is_none()
        {
            return;
        }
        // The rows are borrowed out for the pass, because a press changes the
        // actions and the board; nothing in the pass reads the cache again.
        let rows = std::mem::take(&mut self.machine_rows);
        if !rows.rows().is_empty() || rows.server_line().is_some() {
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
        }
        if let Some(line) = rows.server_line() {
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                ui.label(text::line(
                    ui,
                    line,
                    11.0,
                    text::REGULAR,
                    text::secondary(),
                    ui.available_width() - 18.0,
                ));
            });
            ui.add_space(2.0);
        }
        for row in rows.rows() {
            self.render_sidebar_machine(ui, row, actions);
        }
        self.machine_rows = rows;

        // The line that stays comes first: it names what an opened session left
        // out, and the short reason of an action belongs under it.
        if let Some(note) = self.sidebar_sticky_notice.clone() {
            ui.add_space(2.0);
            if render_sticky_notice(ui, &note) {
                self.dismiss_sidebar_sticky_notice();
            }
        }
        if let Some(notice) = self.sidebar_notice.clone() {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                // The notice names a machine or a reason, and the sidebar is narrow
                // by design, so it wraps instead of cutting its last word away.
                ui.label(text::paragraph(
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
        let mark = row.pending.then(|| {
            text::line(
                ui,
                PENDING_MARK,
                10.0,
                text::REGULAR,
                text::secondary(),
                (text_right - text_left).max(1.0),
            )
        });
        let mark_width = mark.as_ref().map_or(0.0, |mark| mark.size().x + 6.0);
        let name = text::line(
            ui,
            &row.name,
            13.0,
            text::MEDIUM,
            theme::FG(),
            text_right - text_left - mark_width,
        );
        let name_rect = Rect::from_min_size(pos2(text_left, card.min.y + 6.0), name.size());
        let elided = name.elided;
        ui.painter().galley(name_rect.min, name, theme::FG());
        if let Some(mark) = mark {
            let left = text_right - mark.size().x;
            ui.painter()
                .galley(pos2(left, card.min.y + 9.0), mark, text::secondary());
        }
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
        let spoken = spoken(row);
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

/// The line that stays until the person closes it, and its close control. Answers
/// whether the person closed it.
///
/// The control is a press target of its own, as a machine row is, so a keyboard
/// reaches it with Tab and ends it with Space or Enter, and the ring of the focus
/// says where the keyboard is.
fn render_sticky_notice(ui: &mut egui::Ui, note: &str) -> bool {
    let mut closed = false;
    ui.horizontal_top(|ui| {
        ui.add_space(18.0);
        let width = (ui.available_width() - CLOSE_SIDE - 4.0).max(40.0);
        ui.label(text::paragraph(
            ui,
            note,
            11.0,
            text::REGULAR,
            theme::PALETTE_YELLOW(),
            width,
        ));
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(CLOSE_SIDE), Sense::hover());
        let response = ui.interact(rect, sticky_close_id(), Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, CLOSE_SPOKEN));
        if response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            ui.painter()
                .rect_filled(rect, CLOSE_RADIUS, theme::alpha(theme::PANEL_BG_ALT(), 170));
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect,
                CLOSE_RADIUS,
                Stroke::new(1.0_f32, theme::ACCENT()),
                egui::StrokeKind::Outside,
            );
        }
        let color = if response.hovered() || response.has_focus() {
            theme::FG()
        } else {
            text::secondary()
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            CLOSE_MARK,
            egui::FontId::proportional(11.0),
            color,
        );
        if response.clicked() {
            closed = true;
        }
    });
    closed
}

/// The mark of the close control, and what a screen reader calls it.
const CLOSE_MARK: &str = "\u{00d7}";
const CLOSE_SPOKEN: &str = "Close this line";

/// The id of the close control of the sticky line, which a test presses.
pub(in crate::app) fn sticky_close_id() -> egui::Id {
    egui::Id::unique("sidebar-sticky-notice-close")
}

/// What a screen reader and a hover say about one row.
pub(super) fn spoken(row: &MachineRow) -> String {
    if row.pending {
        format!("{}, {}, {PENDING_MARK}", row.name, row.state.spoken())
    } else {
        format!("{}, {}", row.name, row.state.spoken())
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
        MachineState::Reconnecting => {
            painter.circle_filled(center, radius, theme::PALETTE_YELLOW());
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
    // A queue and a link are the work of another thread, so they are read once
    // for the whole fingerprint and not for each machine.
    let pending = app.pending_machines();
    for machine in app.machine_list.records() {
        machine.name.hash(&mut hasher);
        machine.host.hash(&mut hasher);
        machine.user.hash(&mut hasher);
        machine.port.hash(&mut hasher);
        app.machine_list.key(&machine.name).hash(&mut hasher);
        machine.session.hash(&mut hasher);
        machine.provider.hash(&mut hasher);
        pending.contains(machine.name.as_str()).hash(&mut hasher);
    }
    if let Some(sync) = app.server_sync.as_ref() {
        let state = sync.handle().state();
        state.code().hash(&mut hasher);
        state.pending().hash(&mut hasher);
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
        SshConnectionStatus::Reconnecting => 4,
        SshConnectionStatus::Connected => 2,
        SshConnectionStatus::Disconnected => 3,
    }
}

#[cfg(test)]
mod tests;
