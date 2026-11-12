//! One desktop notification for a change of an agent panel that the person did
//! not see, because no window of Horizon had the focus of the desktop.
//!
//! A change while a window of Horizon is in front sends nothing: the sidebar
//! shows it, and the person is already there. A panel that the person marked
//! unread by hand sends nothing either, because that path never reaches this
//! module.
//!
//! A frame only queues a change and sends a notice that the bounds let go: the
//! program of the system is looked up and run on the worker of
//! [`horizon_core::notify`]. Nothing here reads a file, starts a program or waits.

use std::time::Instant;

use horizon_core::notify::{Bounds, Change, ChangeKind, DesktopSink, NotificationSetting, Sink, System};
use horizon_core::{Attention, PanelId};

use super::HorizonApp;

/// The notifications of this run: the window that shows them and the changes that
/// wait for it.
#[derive(Default)]
pub(in crate::app) struct Notifications {
    /// The program of this computer, or no sink in a test that records instead.
    sink: Option<Box<dyn Sink>>,
    bounds: Bounds,
    /// Whether a window of Horizon had the focus of the desktop at the last
    /// decision, so the person who comes back clears the changes that waited.
    decided_in_front: bool,
}

impl Notifications {
    /// The notifications of a run whose program shows them.
    #[cfg(test)]
    pub(in crate::app) fn with_sink(sink: Box<dyn Sink>) -> Self {
        Self {
            sink: Some(sink),
            ..Self::default()
        }
    }

    /// Starts the program of this computer, once for the run.
    pub(in crate::app) fn use_desktop_sink(&mut self) {
        self.sink = Some(Box::new(DesktopSink::start()));
    }

    /// The sentence of the settings row while this computer has no program that
    /// shows a notification, and `None` while it has one or has not looked yet.
    pub(in crate::app) fn missing_program_sentence(&self) -> Option<String> {
        let available = self.sink.as_ref()?.available()?;
        (!available).then(|| horizon_core::notify::missing_sentence(System::of_this_computer()))
    }

    /// Queues one change for the next notification, when the setting tells it.
    pub(in crate::app) fn note_change(&mut self, change: Change, setting: NotificationSetting, now: Instant) {
        if !setting.speaks() || (change.kind == ChangeKind::Done && !setting.tells_done()) {
            return;
        }
        self.bounds.note(change, now);
    }

    /// Clears the changes that the person can see again, and sends the notice that
    /// is due. The focus of the windows is the value that this frame already has,
    /// so this reads no window and starts no poll.
    pub(in crate::app) fn decide(&mut self, several_workspaces: bool, now: Instant, ctx: &egui::Context) {
        let in_front = window_in_front(ctx);
        if in_front && !self.decided_in_front {
            self.bounds.note_front();
        }
        self.decided_in_front = in_front;
        self.poll(several_workspaces, now, ctx);
    }

    /// Sends the notice that is due, and wakes the frame at the end of the group of
    /// the changes that wait.
    pub(in crate::app) fn poll(&mut self, several_workspaces: bool, now: Instant, ctx: &egui::Context) {
        let Some(sink) = &self.sink else {
            return;
        };
        if !self.bounds.waiting() {
            return;
        }
        if let Some(notice) = self.bounds.due(several_workspaces, now) {
            sink.send(notice);
        }
        if let Some(due) = self.bounds.due_at() {
            ctx.request_repaint_after(due.saturating_duration_since(now));
        }
    }

    /// Whether a change waits for its notification.
    #[cfg(test)]
    pub(in crate::app) fn waiting(&self) -> bool {
        self.bounds.waiting()
    }
}

impl HorizonApp {
    /// Notes the changes of `panels`, which are the agent panels whose state
    /// changed, and queues a notification for each one that the person did not see.
    pub(in crate::app) fn note_attention_changes(&mut self, panels: Vec<(String, Attention)>, ctx: &egui::Context) {
        if panels.is_empty() {
            return;
        }
        let setting = self.template_config.features.notifications;
        if !setting.speaks() || window_in_front(ctx) {
            return;
        }
        let now = Instant::now();
        for (local_id, attention) in panels {
            let Some(kind) = change_kind(attention) else {
                continue;
            };
            let Some(panel) = self.board.panels.iter().find(|panel| panel.local_id == local_id) else {
                continue;
            };
            let change = Change {
                panel: panel.title.clone(),
                workspace: self.workspace_name(panel.id),
                kind,
            };
            self.notifications.note_change(change, setting, now);
        }
    }

    /// The name of the workspace of `panel`, when the panel is on the board.
    fn workspace_name(&self, panel: PanelId) -> Option<String> {
        let workspace = self.board.panel_workspace_id(panel)?;
        Some(self.board.workspace(workspace)?.name.clone())
    }
}

/// The change that a state of an agent panel tells, if it tells one.
const fn change_kind(attention: Attention) -> Option<ChangeKind> {
    match attention {
        Attention::NeedsYou => Some(ChangeKind::NeedsYou),
        Attention::Done => Some(ChangeKind::Done),
        Attention::Idle | Attention::Working | Attention::Background => None,
    }
}

#[cfg(test)]
mod tests;

/// Whether a window of Horizon has the focus of the desktop: the main window or a
/// workspace in a window of its own. Every one of them counts, so the person who
/// works in a second Horizon window sees the sidebar.
///
/// The value is the focus that this frame already holds: no window is polled here.
pub(in crate::app) fn window_in_front(ctx: &egui::Context) -> bool {
    ctx.input(|input| {
        input.focused
            || input
                .raw
                .viewports
                .iter()
                .any(|(viewport, info)| *viewport != egui::ViewportId::ROOT && info.focused == Some(true))
    })
}
