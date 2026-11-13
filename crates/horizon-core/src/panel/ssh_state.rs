//! The connection state of an SSH panel.
//!
//! An SSH panel keeps its last screen when its ssh process ends. The state here
//! says which of the two cases it holds: an attempt that Horizon makes again by
//! itself reaches the same tmux session, and a link that stays lost waits for the
//! person.

use std::time::Duration;

use super::{Panel, PanelKind, current_unix_millis};
use crate::ssh::SshConnectionStatus;
use crate::terminal::Terminal;

/// The end of one ssh process of a panel, as the app reads it once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SshConnectionEnd {
    /// The exit status of the process, when the system reported one.
    pub exit_code: Option<i32>,
    /// How long the connection was up before it ended; `None` when it never
    /// reached the machine.
    pub connected_for: Option<Duration>,
    /// The panel's saved command attaches a tmux session on a machine.
    pub machine_tied: bool,
}

impl Panel {
    #[must_use]
    pub fn ssh_status(&self) -> Option<SshConnectionStatus> {
        self.ssh_status
    }

    /// Keep the connection status of an SSH panel, and record the end of its
    /// ssh process once, so the app can decide whether to connect again.
    pub(super) fn track_ssh_connection(&mut self, child_exited: bool, had_output: bool) {
        if child_exited {
            // The status of a panel whose process ended more than once is the
            // app's: it waits for an attempt, or it stays disconnected.
            if self.ssh_exit_recorded {
                return;
            }
            self.ssh_exit_recorded = true;
            let now = current_unix_millis();
            self.ssh_end = Some(SshConnectionEnd {
                exit_code: self
                    .content
                    .terminal()
                    .and_then(Terminal::child_exit_status)
                    .and_then(|status| status.code()),
                connected_for: self
                    .ssh_connected_at_millis
                    .map(|up| Duration::from_millis(u64::try_from(now.saturating_sub(up)).unwrap_or_default())),
                machine_tied: self.attaches_a_tmux_session(),
            });
            self.ssh_status = Some(SshConnectionStatus::Disconnected);
        } else if had_output && matches!(self.ssh_status, Some(SshConnectionStatus::Connecting)) {
            self.ssh_status = Some(SshConnectionStatus::Connected);
            self.ssh_connected_at_millis = Some(current_unix_millis());
            self.ssh_reconnect_attempt = 0;
        }
    }

    /// The end of this panel's ssh process, read once. `None` while its process
    /// runs, and after the app made its decision.
    #[must_use]
    pub fn take_ssh_end(&mut self) -> Option<SshConnectionEnd> {
        self.ssh_end.take()
    }

    /// Whether the saved command of this panel attaches to a tmux session on a
    /// machine, so Horizon can reach the same work again by itself.
    #[must_use]
    pub fn attaches_a_tmux_session(&self) -> bool {
        self.kind == PanelKind::Ssh
            && crate::place::attaches_a_tmux_session(
                self.ssh_connection
                    .as_ref()
                    .and_then(|connection| connection.remote_command.as_deref()),
            )
    }

    /// Marks this panel as waiting for its next attempt `attempt` after a lost
    /// link, and says how many attempts Horizon already made. The body keeps the
    /// last screen until the command starts again.
    pub fn show_reconnecting(&mut self, attempt: u32) {
        if self.kind != PanelKind::Ssh {
            return;
        }
        self.ssh_status = Some(SshConnectionStatus::Reconnecting);
        self.ssh_reconnect_attempt = attempt;
    }

    /// The attempt that the reconnect of this panel reached, while its mark is
    /// the one of a lost link that waits for the next one.
    #[must_use]
    pub fn ssh_status_attempt(&self) -> Option<u32> {
        matches!(self.ssh_status, Some(SshConnectionStatus::Reconnecting))
            .then_some(self.ssh_reconnect_attempt)
            .filter(|attempt| *attempt > 0)
    }

    /// Marks this panel as disconnected, after Horizon did not start its command
    /// again.
    pub fn show_disconnected(&mut self) {
        if self.kind == PanelKind::Ssh {
            self.ssh_status = Some(SshConnectionStatus::Disconnected);
            self.ssh_reconnect_attempt = 0;
        }
    }
}
