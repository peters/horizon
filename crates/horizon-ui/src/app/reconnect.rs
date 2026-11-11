//! A machine terminal that reaches its tmux session again after a lost link.
//!
//! When the ssh process of such a panel ends with the failure of the connection,
//! Horizon starts the panel's own command again after a growing delay, so the
//! person does not have to pick "Reconnect" in the context menu. A command that
//! ran and ended, and a panel of a plain SSH host, stay disconnected: only a
//! panel that attaches a tmux session loses nothing when it starts again.
//!
//! Nothing here owns a thread and nothing repaints every frame. A waiting panel
//! is one entry with a due time, and Horizon asks for one frame at that time.
//! Quitting Horizon drops the entries, so a new run starts at the first delay.

use std::time::Instant;

use egui::Context;
use horizon_core::place::{AfterConnectionLoss, ConnectionLoss, after_connection_loss};
use horizon_core::{PanelId, SshConnectionEnd};

use super::HorizonApp;

/// One panel that waits for its next attempt.
#[derive(Clone, Copy, Debug)]
struct Wait {
    panel_id: PanelId,
    /// When its ssh command starts again.
    due: Instant,
}

/// The panels that wait for their next attempt, and the attempt that each panel
/// reached since its last connection that lasted.
#[derive(Default)]
pub(in crate::app) struct ReconnectWaits {
    waits: Vec<Wait>,
    attempts: Vec<(PanelId, u32)>,
}

impl ReconnectWaits {
    /// How many attempts `panel_id` reached since its last lasting connection.
    pub(in crate::app) fn attempts(&self, panel_id: PanelId) -> u32 {
        self.attempts
            .iter()
            .find(|(candidate, _)| *candidate == panel_id)
            .map_or(0, |(_, attempt)| *attempt)
    }

    /// Arms the attempt `attempt` of `panel_id` for `due`.
    pub(in crate::app) fn arm(&mut self, panel_id: PanelId, attempt: u32, due: Instant) {
        match self.attempts.iter_mut().find(|(candidate, _)| *candidate == panel_id) {
            Some((_, reached)) => *reached = attempt,
            None => self.attempts.push((panel_id, attempt)),
        }
        match self.waits.iter_mut().find(|wait| wait.panel_id == panel_id) {
            Some(wait) => wait.due = due,
            None => self.waits.push(Wait { panel_id, due }),
        }
    }

    /// Stops the wait of `panel_id` and forgets its attempts: the person asked
    /// for a connection now, closed the panel, or nothing starts again.
    pub(in crate::app) fn cancel(&mut self, panel_id: PanelId) {
        self.waits.retain(|wait| wait.panel_id != panel_id);
        self.attempts.retain(|(candidate, _)| *candidate != panel_id);
    }

    /// Whether `panel_id` waits for an attempt now.
    #[cfg(test)]
    pub(in crate::app) fn is_waiting(&self, panel_id: PanelId) -> bool {
        self.waits.iter().any(|wait| wait.panel_id == panel_id)
    }

    /// Whether no panel waits, so a frame has nothing to clean up.
    pub(in crate::app) fn is_empty(&self) -> bool {
        self.waits.is_empty()
    }

    /// The panels whose attempt is due at `now`, which stops their wait.
    pub(in crate::app) fn due(&mut self, now: Instant) -> Vec<PanelId> {
        let mut due = Vec::new();
        self.waits.retain(|wait| {
            if wait.due > now {
                return true;
            }
            due.push(wait.panel_id);
            false
        });
        due
    }

    /// Drops the waits and attempts of panels that are gone.
    pub(in crate::app) fn retain(&mut self, mut keep: impl FnMut(PanelId) -> bool) {
        self.waits.retain(|wait| keep(wait.panel_id));
        self.attempts.retain(|(panel_id, _)| keep(*panel_id));
    }

    /// When the earliest wait is due, so a test can read the schedule.
    #[cfg(test)]
    pub(in crate::app) fn next_due(&self) -> Option<Instant> {
        self.waits.iter().map(|wait| wait.due).min()
    }
}

impl HorizonApp {
    /// Reads the end of every machine terminal, arms its next attempt, and
    /// starts the attempts that are due.
    pub(super) fn poll_reconnects(&mut self, ctx: &Context, now: Instant) {
        self.poll_reconnects_with(ctx, now, Self::start_next_attempt);
    }

    /// [`Self::poll_reconnects`] with the start of an attempt given, so a test
    /// waits for no delay and starts no ssh.
    pub(in crate::app) fn poll_reconnects_with(
        &mut self,
        ctx: &Context,
        now: Instant,
        mut start_attempt: impl FnMut(&mut Self, PanelId),
    ) {
        if !self.reconnects.is_empty() {
            // A panel that the person closed, or that this frame closes, waits
            // for nothing.
            let closing = &self.panels_to_close;
            let open: Vec<PanelId> = self
                .board
                .panels
                .iter()
                .map(|panel| panel.id)
                .filter(|panel_id| !closing.contains(panel_id))
                .collect();
            self.reconnects.retain(|panel_id| open.contains(&panel_id));
        }
        self.arm_attempts(ctx, now);
        for panel_id in self.reconnects.due(now) {
            start_attempt(self, panel_id);
        }
    }

    /// Arms the next attempt of each machine terminal whose ssh process ended.
    fn arm_attempts(&mut self, ctx: &Context, now: Instant) {
        let mut ends = Vec::new();
        for panel in &mut self.board.panels {
            if let Some(end) = panel.take_ssh_end() {
                ends.push((panel.id, end));
            }
        }
        for (panel_id, end) in ends {
            self.arm_attempt(ctx, now, panel_id, end);
        }
    }

    fn arm_attempt(&mut self, ctx: &Context, now: Instant, panel_id: PanelId, end: SshConnectionEnd) {
        if self.machine_record_is_gone(panel_id) {
            // The record that opened this panel is gone, so ssh's last words
            // stay on the screen and Horizon stops trying.
            self.reconnects.cancel(panel_id);
            return;
        }
        // A panel that the person closed holds no ssh process to report:
        // closing cancels its wait, so this loss is of a panel still open.
        let plan = after_connection_loss(ConnectionLoss {
            exit_code: end.exit_code,
            connected_for: end.connected_for,
            machine_tied: end.machine_tied,
            closed_by_the_person: false,
            attempts: self.reconnects.attempts(panel_id),
        });
        match plan {
            AfterConnectionLoss::Reconnect { attempt, after } => {
                if let Some(panel) = self.board.panel_mut(panel_id) {
                    panel.show_reconnecting(attempt);
                }
                self.reconnects.arm(panel_id, attempt, now + after);
                tracing::info!(
                    panel_id = panel_id.0,
                    attempt,
                    after_ms = u64::try_from(after.as_millis()).unwrap_or(u64::MAX),
                    "a machine terminal waits to reconnect after a lost link"
                );
                // One wake at the due time, not a frame every tick.
                ctx.request_repaint_after(after);
            }
            AfterConnectionLoss::Stay => self.reconnects.cancel(panel_id),
        }
    }

    /// Whether the machine record that opened this panel is no longer in the
    /// machine list. A panel that no record opened has nothing to lose.
    fn machine_record_is_gone(&self, panel_id: PanelId) -> bool {
        self.board
            .panel(panel_id)
            .and_then(horizon_core::Panel::machine)
            .is_some_and(|name| self.machine_list.find(name).is_none())
    }

    /// Starts the attempt that a wait saved, through the path of the
    /// "Reconnect" action.
    fn start_next_attempt(&mut self, panel_id: PanelId) {
        match self.board.restart_panel(panel_id) {
            Ok(()) => {
                // The new process draws a new screen, so the grid of the lost
                // one must not come back from the render cache.
                self.panel_render_caches.terminal_grid_cache.remove(&panel_id);
                self.panel_render_caches.editor_preview_cache.remove(&panel_id);
                self.panel_render_caches.browser_ui_state.remove(&panel_id);
                self.panel_render_caches.device_ui_state.remove(&panel_id);
                tracing::info!(panel_id = panel_id.0, "a machine terminal reconnects after a lost link");
            }
            Err(error) => {
                // A command that does not start at all is not a lost link, so
                // the panel stays disconnected until the person acts.
                tracing::warn!(panel_id = panel_id.0, %error, "a machine terminal did not reconnect");
                self.reconnects.cancel(panel_id);
                if let Some(panel) = self.board.panel_mut(panel_id) {
                    panel.show_disconnected();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
