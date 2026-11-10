//! Whether a queued click may wake the agent that owns its card.
//!
//! An agent that is not listening reads its card only when Horizon tells it
//! that something happened. Horizon types one fixed line into the owner's
//! terminal for that. The line never carries text from the card, and it goes
//! out only when the agent is idle, is not the panel the person is typing in,
//! and has not been woken for this streak of clicks already.

use std::time::Duration;

use crate::agents::AgentStatus;

/// What Horizon types into an owner's terminal when a person pressed a button
/// on its card. A constant: it never carries text from the card, so a card
/// cannot write into an agent's input.
pub const WAKE_LINE: &str =
    "A button was pressed on your Horizon card. Call the block_panel tool with operation next_click to read it.";

/// The least time between two wake lines to the same agent panel.
pub const WAKE_LINE_INTERVAL: Duration = Duration::from_secs(30);

/// What Horizon knows about one card and its owner when a click waits.
#[derive(Clone, Copy, Debug)]
pub struct WakeContext {
    /// Clicks this card holds for its agent.
    pub queued_clicks: usize,
    /// A `next_click` call of the owner waits on this card.
    pub listening: bool,
    /// An agent panel with the owner's actor is in the card's workspace.
    pub owner_present: bool,
    /// That agent panel holds keyboard focus.
    pub owner_focused: bool,
    /// That agent panel's working status.
    pub owner_status: AgentStatus,
    /// A wake line already went out for this streak of clicks.
    pub wake_already_sent: bool,
    /// Time since the last wake line to that agent panel, when one went out.
    pub since_last_wake: Option<Duration>,
}

/// Why a queued click caused no wake line. Each reason is also the condition a
/// later board tick tests again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeHold {
    /// The card holds no click.
    NothingQueued,
    /// The owner waits in `next_click`, so the click reaches it directly.
    Listening,
    /// No agent panel of the owner is in the card's workspace.
    AgentAbsent,
    /// The person is typing in the owner's panel; a wake line would collide.
    AgentFocused,
    /// The owner is in the middle of a turn; a wake line would corrupt its
    /// input.
    AgentWorking,
    /// A wake line already went out for this streak of clicks.
    AlreadyWoke,
    /// The last wake line to this agent panel is younger than
    /// [`WAKE_LINE_INTERVAL`].
    TooSoon,
}

/// Whether to type the wake line now, or what to test again later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeVerdict {
    Send,
    Hold(WakeHold),
}

/// Decide one wake attempt. A click that is held is tried again on a later
/// board tick while it still waits.
#[must_use]
pub fn wake_verdict(context: &WakeContext) -> WakeVerdict {
    use WakeHold::{AgentAbsent, AgentFocused, AgentWorking, AlreadyWoke, Listening, NothingQueued, TooSoon};
    let hold = if context.queued_clicks == 0 {
        NothingQueued
    } else if context.listening {
        Listening
    } else if !context.owner_present {
        AgentAbsent
    } else if context.owner_focused {
        AgentFocused
    } else if context.owner_status == AgentStatus::Working {
        AgentWorking
    } else if context.wake_already_sent {
        AlreadyWoke
    } else if context.since_last_wake.is_some_and(|since| since < WAKE_LINE_INTERVAL) {
        TooSoon
    } else {
        return WakeVerdict::Send;
    };
    WakeVerdict::Hold(hold)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> WakeContext {
        WakeContext {
            queued_clicks: 1,
            listening: false,
            owner_present: true,
            owner_focused: false,
            owner_status: AgentStatus::Idle,
            wake_already_sent: false,
            since_last_wake: Some(WAKE_LINE_INTERVAL),
        }
    }

    #[test]
    fn a_queued_click_wakes_an_idle_absent_minded_owner_once() {
        assert_eq!(wake_verdict(&ready()), WakeVerdict::Send);
        assert_eq!(
            wake_verdict(&WakeContext {
                since_last_wake: None,
                ..ready()
            }),
            WakeVerdict::Send,
            "an owner that was never woken is woken at once"
        );
    }

    #[test]
    fn each_condition_holds_the_wake_line() {
        for (context, expected) in [
            (
                WakeContext {
                    queued_clicks: 0,
                    ..ready()
                },
                WakeHold::NothingQueued,
            ),
            (
                WakeContext {
                    listening: true,
                    ..ready()
                },
                WakeHold::Listening,
            ),
            (
                WakeContext {
                    owner_present: false,
                    ..ready()
                },
                WakeHold::AgentAbsent,
            ),
            (
                WakeContext {
                    owner_focused: true,
                    ..ready()
                },
                WakeHold::AgentFocused,
            ),
            (
                WakeContext {
                    owner_status: AgentStatus::Working,
                    ..ready()
                },
                WakeHold::AgentWorking,
            ),
            (
                WakeContext {
                    wake_already_sent: true,
                    ..ready()
                },
                WakeHold::AlreadyWoke,
            ),
            (
                WakeContext {
                    since_last_wake: Some(WAKE_LINE_INTERVAL.saturating_sub(Duration::from_millis(1))),
                    ..ready()
                },
                WakeHold::TooSoon,
            ),
        ] {
            assert_eq!(wake_verdict(&context), WakeVerdict::Hold(expected), "{context:?}");
        }
    }

    #[test]
    fn the_wake_line_is_one_fixed_line_without_a_placeholder() {
        assert!(!WAKE_LINE.contains('{') && !WAKE_LINE.contains('}'), "{WAKE_LINE}");
        assert!(!WAKE_LINE.contains('\n') && !WAKE_LINE.contains('\r'), "{WAKE_LINE}");
        assert!(!WAKE_LINE.contains('%'), "{WAKE_LINE}");
        assert!(
            WAKE_LINE.contains("next_click"),
            "the line tells the agent how to read the click"
        );
        assert_eq!(
            WAKE_LINE,
            "A button was pressed on your Horizon card. Call the block_panel tool with operation next_click to read it."
        );
    }
}
