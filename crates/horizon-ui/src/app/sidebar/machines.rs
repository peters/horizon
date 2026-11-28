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

mod agents;
mod notice;
mod paint;

use notice::render_sticky_notice;

use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use egui::{Rect, Vec2, pos2, vec2};
use horizon_core::{PanelId, SshConnectionStatus, WorkspaceId};

use crate::theme;

use super::super::HorizonApp;
use super::text;
use super::{SidebarActions, caption, rows};

pub(in crate::app) use agents::MachineWatchRow;

/// The height of one machine row, its inset in the sidebar, its radius and its
/// padding.
pub(super) const ROW_HEIGHT: f32 = 40.0;
const PADDING: f32 = 10.0;
/// The side of the box that marks a machine and its radius.
const BOX_SIDE: f32 = 24.0;
const BOX_RADIUS: u8 = 7;
/// The hover text of the control that sets a machine up.
pub(super) const SET_UP_HOVER: &str = "Set up a machine";
/// How long the reason of a refused machine stays in the section.
pub(super) const NOTICE: Duration = Duration::from_secs(10);

/// The clock of the short notice of the section: the clock of the computer, or the
/// clock of a test, which moves forward instead of waiting for ten seconds.
#[derive(Clone, Copy, Default)]
pub(in crate::app) struct NoticeClock {
    /// How far ahead of the computer this clock reads. A run leaves it at zero, and
    /// a test moves it forward, so the ten seconds of a notice cost no second.
    pub(in crate::app) ahead: Duration,
}

/// How the short notice of the sidebar is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum SidebarNoticeTone {
    /// A reason that the person has to act on.
    Error,
    /// A plain answer to a key press, which names nothing to fix.
    Plain,
}

impl NoticeClock {
    /// What this clock reads now.
    fn now(&self) -> Instant {
        Instant::now() + self.ahead
    }
}
/// The quiet mark of a machine whose last change the server has not decided.
pub(super) const PENDING_MARK: &str = "pending";

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
    /// The second line of the row: the size and the place of a rented machine, or
    /// the repository, folder and provider words of the record when it holds
    /// neither, or nothing when the record names none of them.
    pub(in crate::app) second: Option<String>,
    /// What one hour of a rented machine costs, when the agent that rented it
    /// reported it: the row draws it at its right end as `<price>/h`.
    pub(in crate::app) price: Option<String>,
    pub(in crate::app) state: MachineState,
    /// Whether the server has not decided the last change to this machine yet.
    pub(in crate::app) pending: bool,
    /// The frame of the terminal of this machine, when one is open.
    pub(in crate::app) panel: Option<PanelId>,
    pub(in crate::app) workspace: Option<WorkspaceId>,
    /// The agents of this machine and the state of its link, in the words of a row.
    pub(in crate::app) watch: MachineWatchRow,
}

/// The rows of the machine section, with the list and the terminal states that
/// they were read from.
#[derive(Default)]
pub(in crate::app) struct MachineRows {
    rows: Vec<MachineRow>,
    /// The line of the link to the server, built once for its state, or `None`
    /// when this Horizon keeps its machine list on this computer alone.
    server_line: Option<String>,
    /// Why the last attempt at the link did not open it, for the hover of the
    /// line. It is a fixed sentence of `horizon-sync`, so it carries no address
    /// and no secret.
    server_reason: Option<&'static str>,
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
        let pending = self.pending_machines();
        // One snapshot of the watch for the whole pass: the worker published it, and
        // no row reads a file or a link.
        let snapshot = self.machine_watch.snapshot.clone();
        let now_millis = horizon_core::current_unix_millis();
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
                let refusal = self.node_cards.refusal(&machine.name);
                let watch = MachineWatchRow::of(
                    snapshot.as_ref().and_then(|snapshot| snapshot.machine(&machine.name)),
                    now_millis,
                    machine.cards.then_some(refusal).flatten().as_deref(),
                );
                MachineRow {
                    name: machine.name.clone(),
                    // A lost link says when it was lost in the place of the record.
                    second: watch.lost.clone().or(second_line(machine)),
                    price: machine.price_per_hour.clone(),
                    state: MachineState::of(panel.and_then(horizon_core::Panel::ssh_status)),
                    pending: pending.contains(machine.name.as_str()),
                    panel: panel.map(|panel| panel.id),
                    workspace: panel.map(|panel| panel.workspace_id),
                    watch,
                }
            })
            .collect();
        // The line is built on the frame that the state changes and on no other.
        let (server_line, server_reason) = self.server_sync.as_ref().map_or((None, None), |sync| {
            let line = sync.line();
            (Some(line.text()), line.refusal())
        });
        self.machine_rows = MachineRows {
            rows,
            server_line,
            server_reason,
            read_from: Some(read_from),
        };
    }

    /// Draws the machine section below the work items, when this Horizon keeps a
    /// machine.
    pub(super) fn render_sidebar_machines(&mut self, ui: &mut egui::Ui, actions: &mut SidebarActions) {
        let now = self.notice_clock.now();
        if self.sidebar_notice_until.is_some_and(|until| now >= until) {
            self.sidebar_notice = None;
            self.sidebar_notice_until = None;
        }
        // The rows are borrowed out for the pass, because a press changes the
        // actions and the board; nothing in the pass reads the cache again.
        let rows = std::mem::take(&mut self.machine_rows);
        let count = rows.rows().len();
        let words = self.sidebar_chrome.counts.text(caption::MACHINES, count);
        // A group with no machine says no count, and its plus is the way to get one.
        let counted = (count > 0).then_some(words);
        let controls = [caption::Control::new(caption::Glyph::Plus, SET_UP_HOVER)];
        if let Some(press) = caption::group(ui, caption::MACHINES, counted, &controls) {
            actions.take_caption_press(super::Group::Machines, press);
        }
        // The setting that the server decided for another computer stays under the
        // caption of the section, in the pattern of the line that names the
        // panels that a session left out: its own close control and no timer.
        if let Some(line) = self.settings_lost.clone() {
            ui.add_space(2.0);
            if render_sticky_notice(ui, &line, settings_lost_close_id()) {
                self.dismiss_settings_lost();
            }
            ui.add_space(2.0);
        }
        for row in rows.rows() {
            self.render_sidebar_machine(ui, row, actions);
        }
        self.machine_rows = rows;

        // The control of the machine that the orchestrator rents stands under the
        // rows of the list, and it stands there with no machine as well.
        self.render_rent_machine(ui, actions);

        // The line that stays comes first: it names what an opened session left
        // out, and the short reason of an action belongs under it.
        if let Some(note) = self.sidebar_sticky_notice.clone() {
            ui.add_space(2.0);
            if render_sticky_notice(ui, &note, sticky_close_id()) {
                self.dismiss_sidebar_sticky_notice();
            }
        }
        if let Some(notice) = self.sidebar_notice.clone() {
            // An error tells the person what to fix; an answer to a key press
            // only says that nothing waits, so it stays in the muted text.
            let tone = match self.sidebar_notice_tone {
                SidebarNoticeTone::Error => theme::PALETTE_RED(),
                SidebarNoticeTone::Plain => text::secondary(),
            };
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
                    tone,
                    ui.available_width() - 18.0,
                ));
            });
        }
    }

    /// The line of the link to the server, for the footer: the server line of
    /// today, or the line that says that this Horizon follows no server.
    pub(super) fn server_line(&self) -> &str {
        self.machine_rows
            .server_line
            .as_deref()
            .unwrap_or(super::footer::NO_SERVER)
    }

    /// Why the last attempt at the link to the server did not open it, for the
    /// hover of the footer.
    pub(super) fn server_line_reason(&self) -> Option<&'static str> {
        self.machine_rows.server_reason
    }
}

/// The id of the close control of the sticky line, which a test presses.
pub(in crate::app) fn sticky_close_id() -> egui::Id {
    egui::Id::unique("sidebar-sticky-notice-close")
}

/// The id of the close control of the line about a setting that another computer
/// changed first, which a test presses.
pub(in crate::app) fn settings_lost_close_id() -> egui::Id {
    egui::Id::unique("sidebar-settings-lost-close")
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

/// The box that marks a machine row: 24 wide, 10 in from the left edge of its card
/// and in the middle of the row.
pub(super) fn box_rect(card: Rect) -> Rect {
    Rect::from_min_size(
        pos2(card.min.x + PADDING, card.center().y - BOX_SIDE / 2.0),
        Vec2::splat(BOX_SIDE),
    )
}

/// The centre of the box that marks a machine row, which the guide of the agents of
/// the machine hangs from.
pub(super) fn dot_center(card: Rect) -> egui::Pos2 {
    box_rect(card).center()
}

/// What the right end of a machine row says about one hour of the machine: the
/// price of the record and `/h`, in the words that the design gives for it.
fn price_line(price: &str) -> String {
    format!("{price}/h")
}

/// The second line of a machine row: the size and the place of a rented machine
/// when the record knows at least one of them, and the repository that the record
/// names, the folder that its new terminals start in and the provider words of the
/// agent otherwise. What Horizon knows comes first, so a narrow row cuts the claim
/// of the agent.
fn second_line(machine: &horizon_core::Machine) -> Option<String> {
    if let Some(facts) = horizon_core::MachineFacts::of(machine).size_and_place() {
        return Some(facts);
    }
    let parts: Vec<&str> = [
        machine.repository.as_deref(),
        machine.folder.as_deref(),
        machine.provider.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join(" \u{00b7} "))
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
        machine.repository.hash(&mut hasher);
        machine.folder.hash(&mut hasher);
        machine.size.hash(&mut hasher);
        machine.place.hash(&mut hasher);
        machine.price_per_hour.hash(&mut hasher);
        pending.contains(machine.name.as_str()).hash(&mut hasher);
        app.node_cards.refusal(&machine.name).hash(&mut hasher);
    }
    if let Some(sync) = app.server_sync.as_ref() {
        // One snapshot of the link for the whole fingerprint: the worker of the
        // link published it, and no read of it waits for a disk.
        let link = sync.snapshot();
        let state = link.state();
        state.code().hash(&mut hasher);
        state.pending().hash(&mut hasher);
    }
    for panel in app.board.panels.iter().filter(|panel| panel.machine().is_some()) {
        panel.machine().hash(&mut hasher);
        panel.id.0.hash(&mut hasher);
        panel.workspace_id.0.hash(&mut hasher);
        panel.ssh_status().map(status_code).hash(&mut hasher);
    }
    // The rows of the agents come from the snapshot of the watch, so a frame that
    // did not change it builds no row again.
    app.machine_watch
        .snapshot
        .as_ref()
        .map_or(0, |snapshot| snapshot.version)
        .hash(&mut hasher);
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
