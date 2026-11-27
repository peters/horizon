//! The conversation that a window will read: one list of entries, its link and
//! its limit, behind an [`Arc`] that changes only with the events.
//!
//! [`Conversation::apply`] takes one event and changes the snapshot; a frame
//! calls [`Conversation::snapshot`] and draws what it holds. The snapshot is
//! copy-on-write: a frame that keeps an old `Arc` pays one clone only when an
//! event changes the conversation under it.
//!
//! Every text in an entry comes from the program or from the person, so it is
//! cut before it enters the snapshot: [`MAX_TEXT_BYTES`] bytes for the text of
//! one message and [`MAX_SUMMARY_CHARS`] characters for a notice. The list keeps
//! [`MAX_ENTRIES`] entries; older entries leave, and one [`Entry::Notice`] at
//! the top of the list names how many it holds no longer. The snapshot also sums
//! the cost of every ended turn in `cost_usd_total`, which a window shows as the
//! cost of the conversation.

use std::sync::Arc;

use super::codec::{cut_bytes, tidy};
use super::event::BridgeEvent;
use super::{MAX_ENTRIES, MAX_SUMMARY_CHARS, MAX_TEXT_BYTES};

/// The link of one bridge, as the person sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Link {
    /// The program did not name its session yet.
    #[default]
    NotStarted,
    /// The program named its session.
    Running,
    /// The standard output of the program ended.
    Ended { exit_code: Option<i32> },
}

/// The state of one tool call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Done,
    Failed,
}

/// One entry of a conversation.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// A message of the person.
    Person { text: String },
    /// The whole text of one message of the agent so far.
    Agent { message_id: String, text: String },
    /// One tool call of the agent.
    Tool {
        tool_id: String,
        name: String,
        summary: String,
        state: ToolState,
    },
    /// A line that Horizon writes for the person: a refusal, a dropped line or
    /// the end of the program.
    Notice { text: String },
}

/// The limit of the account that the program reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitNote {
    pub status: String,
    pub using_overage: bool,
    pub resets_at_unix: Option<u64>,
}

/// The conversation of one bridge at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct ConversationSnapshot {
    /// Grows by one with each change of this value.
    pub version: u64,
    pub link: Link,
    pub session_id: Option<String>,
    pub entries: Vec<Entry>,
    pub busy: bool,
    pub limit: Option<LimitNote>,
    /// The sum of the cost of every ended turn of this program.
    pub cost_usd_total: f64,
}

impl ConversationSnapshot {
    /// The conversation before its program said anything.
    fn empty() -> Self {
        Self {
            version: 0,
            link: Link::NotStarted,
            session_id: None,
            entries: Vec::new(),
            busy: false,
            limit: None,
            cost_usd_total: 0.0,
        }
    }
}

/// The conversation of one bridge, which a window will read.
#[derive(Debug)]
pub struct Conversation {
    snapshot: Arc<ConversationSnapshot>,
    /// How many entries left the list at its top.
    capped: u64,
    /// The text of the notice of the entries that left.
    cap_text: String,
    /// How many lines of the program the codec refused.
    dropped: u32,
    /// The text of the notice of the refused lines.
    dropped_text: String,
}

impl Default for Conversation {
    fn default() -> Self {
        Self::new()
    }
}

impl Conversation {
    /// A conversation that holds nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            snapshot: Arc::new(ConversationSnapshot::empty()),
            capped: 0,
            cap_text: String::new(),
            dropped: 0,
            dropped_text: String::new(),
        }
    }

    /// The conversation of this moment, which a frame holds.
    #[must_use]
    pub fn snapshot(&self) -> Arc<ConversationSnapshot> {
        Arc::clone(&self.snapshot)
    }

    /// Appends one message of the person and marks the conversation busy.
    pub fn person_said(&mut self, text: &str) {
        let snapshot = Arc::make_mut(&mut self.snapshot);
        snapshot.version += 1;
        snapshot.busy = true;
        snapshot.entries.push(Entry::Person {
            text: cut_bytes(text, MAX_TEXT_BYTES).to_owned(),
        });
        Self::trim(snapshot, &mut self.capped, &mut self.cap_text);
    }

    /// Appends one line that Horizon writes for the person, such as the notice
    /// of a turn that the person stopped. The text is cut to
    /// [`MAX_SUMMARY_CHARS`] characters and the busy state does not change.
    pub fn notice(&mut self, text: &str) {
        let snapshot = Arc::make_mut(&mut self.snapshot);
        snapshot.version += 1;
        snapshot.entries.push(Entry::Notice {
            text: tidy(text, MAX_SUMMARY_CHARS),
        });
        Self::trim(snapshot, &mut self.capped, &mut self.cap_text);
    }

    /// Applies one event of the program.
    pub fn apply(&mut self, event: BridgeEvent) {
        let snapshot = Arc::make_mut(&mut self.snapshot);
        snapshot.version += 1;
        match event {
            BridgeEvent::SessionStarted { session_id } => {
                snapshot.link = Link::Running;
                snapshot.session_id = Some(tidy(&session_id, MAX_SUMMARY_CHARS));
            }
            BridgeEvent::TurnStarted => snapshot.busy = true,
            BridgeEvent::AgentText { message_id, text } => apply_agent_text(snapshot, &message_id, &text),
            BridgeEvent::ToolStarted { tool_id, name, summary } => snapshot.entries.push(Entry::Tool {
                tool_id,
                name,
                summary,
                state: ToolState::Running,
            }),
            BridgeEvent::ToolEnded {
                tool_id,
                failed,
                summary,
            } => apply_tool_end(snapshot, &tool_id, failed, &summary),
            BridgeEvent::PermissionDenied { tool, message } => snapshot.entries.push(Entry::Notice {
                text: denied_text(&tool, &message),
            }),
            BridgeEvent::Limit {
                status,
                using_overage,
                resets_at_unix,
            } => {
                snapshot.limit = Some(LimitNote {
                    status: tidy(&status, MAX_SUMMARY_CHARS),
                    using_overage,
                    resets_at_unix,
                });
            }
            BridgeEvent::TurnEnded { failed, cost_usd, .. } => {
                snapshot.busy = false;
                if let Some(cost_usd) = cost_usd
                    && cost_usd.is_finite()
                    && cost_usd > 0.0
                {
                    snapshot.cost_usd_total += cost_usd;
                }
                if failed {
                    snapshot.entries.push(Entry::Notice {
                        text: "the turn failed".to_owned(),
                    });
                }
            }
            BridgeEvent::Dropped { .. } => apply_drop(snapshot, &mut self.dropped, &mut self.dropped_text),
            BridgeEvent::Ended { exit_code, stderr_tail } => apply_end(snapshot, exit_code, &stderr_tail),
        }
        Self::trim(snapshot, &mut self.capped, &mut self.cap_text);
    }

    /// Keeps at most [`MAX_ENTRIES`] entries, and one notice at the top of the
    /// list that says how many left.
    fn trim(snapshot: &mut ConversationSnapshot, capped: &mut u64, cap_text: &mut String) {
        let count = snapshot.entries.len();
        if count <= MAX_ENTRIES {
            return;
        }
        // A notice of an earlier trim holds the first place, so it stays.
        let first = match snapshot.entries.first() {
            Some(Entry::Notice { text }) if text == cap_text => 1,
            _ => 0,
        };
        let excess = count - MAX_ENTRIES;
        let remove = if first == 1 { excess } else { excess + 1 };
        snapshot.entries.drain(first..first + remove);
        *capped += remove as u64;
        let text = cap_notice(*capped);
        cap_text.clone_from(&text);
        match first {
            1 => {
                if let Some(Entry::Notice { text: shown }) = snapshot.entries.first_mut() {
                    shown.clone_from(&text);
                }
            }
            _ => snapshot.entries.insert(0, Entry::Notice { text }),
        }
    }
}

/// Replaces the text of the message with `message_id`, or appends it.
fn apply_agent_text(snapshot: &mut ConversationSnapshot, message_id: &str, text: &str) {
    let text = cut_bytes(text, MAX_TEXT_BYTES).to_owned();
    let found = snapshot
        .entries
        .iter()
        .rposition(|entry| matches!(entry, Entry::Agent { message_id: id, .. } if id.as_str() == message_id));
    match found {
        Some(index) => {
            if let Entry::Agent { text: shown, .. } = &mut snapshot.entries[index] {
                *shown = text;
            }
        }
        None => snapshot.entries.push(Entry::Agent {
            message_id: message_id.to_owned(),
            text,
        }),
    }
}

/// Moves one tool call to its end. A result of a tool that this conversation did
/// not see start changes nothing.
fn apply_tool_end(snapshot: &mut ConversationSnapshot, tool_id: &str, failed: bool, summary: &str) {
    let found = snapshot
        .entries
        .iter()
        .rposition(|entry| matches!(entry, Entry::Tool { tool_id: id, .. } if id.as_str() == tool_id));
    if let Some(index) = found
        && let Entry::Tool {
            state: shown,
            summary: kept,
            ..
        } = &mut snapshot.entries[index]
    {
        *shown = if failed { ToolState::Failed } else { ToolState::Done };
        if !summary.is_empty() {
            kept.clear();
            kept.push_str(summary);
        }
    }
}

/// Counts one refused line in one notice, so a program that sends bad lines
/// keeps one line of the conversation and not a page.
fn apply_drop(snapshot: &mut ConversationSnapshot, dropped: &mut u32, dropped_text: &mut String) {
    *dropped = dropped.saturating_add(1);
    let text = dropped_notice(*dropped);
    let previous = dropped_text.as_str();
    let found = snapshot
        .entries
        .iter()
        .rposition(|entry| matches!(entry, Entry::Notice { text: shown } if shown.as_str() == previous));
    dropped_text.clone_from(&text);
    if let Some(index) = found {
        if let Entry::Notice { text: shown } = &mut snapshot.entries[index] {
            shown.clone_from(&text);
        }
    } else {
        snapshot.entries.push(Entry::Notice { text });
    }
}

/// Ends the conversation with one notice.
fn apply_end(snapshot: &mut ConversationSnapshot, exit_code: Option<i32>, stderr_tail: &str) {
    snapshot.link = Link::Ended { exit_code };
    snapshot.busy = false;
    snapshot.entries.push(Entry::Notice {
        text: ended_notice(exit_code, stderr_tail),
    });
}

/// The text of the notice of the entries that left the list.
fn cap_notice(capped: u64) -> String {
    format!("{capped} earlier entries were dropped")
}

/// The text of the notice of the lines that the codec refused.
fn dropped_notice(dropped: u32) -> String {
    if dropped == 1 {
        "one line of the program was dropped".to_owned()
    } else {
        format!("{dropped} lines of the program were dropped")
    }
}

/// The text of the notice of one refusal of the program.
fn denied_text(tool: &str, message: &str) -> String {
    let tool = tidy(tool, MAX_SUMMARY_CHARS);
    let message = tidy(message, MAX_SUMMARY_CHARS);
    if message.is_empty() {
        format!("{tool} was denied")
    } else {
        format!("{tool} was denied: {message}")
    }
}

/// The text of the notice of the end of the program.
fn ended_notice(exit_code: Option<i32>, stderr_tail: &str) -> String {
    let tail = tidy(stderr_tail, MAX_SUMMARY_CHARS);
    let end = match exit_code {
        Some(code) => format!("the program ended with code {code}"),
        None => "the program ended".to_owned(),
    };
    if tail.is_empty() { end } else { format!("{end}: {tail}") }
}
