//! Whether a machine terminal reaches its tmux session again by itself after a
//! lost link, and how long it waits before the next attempt.
//!
//! Only the connection of the panel counts here. A panel whose saved command
//! attaches a tmux session keeps its work on the machine, so a second `ssh`
//! loses nothing. A remote shell that ended, a person who closed the panel and a
//! plain SSH host of the remote hosts chooser all stay as they are: with `tmux
//! new-session -A` a restart of a session that ended would silently make a new
//! one, which only the person may ask for.

use std::time::Duration;

/// The exit status that `ssh` reports for a failure of the connection itself,
/// such as a host that stops answering. A command that ran and failed has its
/// own status, so any other status means the connection was not the problem.
pub const SSH_CONNECTION_FAILURE: i32 = 255;

/// How long a connection must be up before the next loss starts the delays
/// again. A link that drops just after it came up continues where it was.
pub const RECONNECT_RESET_AFTER: Duration = Duration::from_secs(10);

/// The delays of the attempts, in order; every attempt after the last one waits
/// [`REPEAT_DELAY`].
const DELAYS: [Duration; 4] = [
    Duration::from_secs(3),
    Duration::from_secs(6),
    Duration::from_secs(12),
    Duration::from_secs(24),
];

/// The delay of every attempt after the ones of [`DELAYS`].
const REPEAT_DELAY: Duration = Duration::from_secs(30);

/// How long Horizon waits before attempt `attempt`, counted from 1.
#[must_use]
pub fn reconnect_delay(attempt: u32) -> Duration {
    usize::try_from(attempt)
        .ok()
        .and_then(|index| index.checked_sub(1))
        .and_then(|index| DELAYS.get(index).copied())
        .unwrap_or(REPEAT_DELAY)
}

/// Whether a saved remote command attaches to a tmux session, so the same work
/// comes back after a lost link.
///
/// Only `new -A` and `new-session -A`, which attach when the session exists,
/// and an `attach` count: a `new` without `-A` would make a second session or
/// fail, so Horizon leaves that command to the person.
#[must_use]
pub fn attaches_a_tmux_session(remote_command: Option<&str>) -> bool {
    let Some(command) = remote_command else {
        return false;
    };
    let words: Vec<&str> = command.split_whitespace().collect();
    match words.as_slice() {
        ["tmux", "new" | "new-session", rest @ ..] => rest.contains(&"-A"),
        ["tmux", "attach" | "attach-session" | "a", rest @ ..] => !rest.is_empty(),
        _ => false,
    }
}

/// What Horizon knows when the ssh process of a panel ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectionLoss {
    /// The exit status of the panel's ssh process, when the system reported one.
    pub exit_code: Option<i32>,
    /// How long the connection that just ended was up. `None` when it never
    /// reached the machine.
    pub connected_for: Option<Duration>,
    /// The panel's saved command attaches a tmux session that outlives the
    /// link, so Horizon may reach it again without losing work.
    pub machine_tied: bool,
    /// The person closed the panel: nothing starts again.
    pub closed_by_the_person: bool,
    /// How many attempts this panel has made since its last connection that
    /// lasted [`RECONNECT_RESET_AFTER`].
    pub attempts: u32,
}

/// What Horizon does after a lost link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterConnectionLoss {
    /// Run the panel's own command again after `after`, as attempt `attempt`.
    Reconnect {
        /// The number of the attempt that comes next, counted from 1.
        attempt: u32,
        /// How long Horizon waits before it starts that attempt.
        after: Duration,
    },
    /// Leave the panel disconnected. Its last screen stays, and the person
    /// decides whether to reconnect.
    Stay,
}

/// The decision after the ssh process of a panel ended.
#[must_use]
pub fn after_connection_loss(loss: ConnectionLoss) -> AfterConnectionLoss {
    if loss.closed_by_the_person || !loss.machine_tied || loss.exit_code != Some(SSH_CONNECTION_FAILURE) {
        return AfterConnectionLoss::Stay;
    }
    let attempts = if loss.connected_for.is_some_and(|up| up >= RECONNECT_RESET_AFTER) {
        0
    } else {
        loss.attempts
    };
    let attempt = attempts.saturating_add(1);
    AfterConnectionLoss::Reconnect {
        attempt,
        after: reconnect_delay(attempt),
    }
}

#[cfg(test)]
mod tests;
