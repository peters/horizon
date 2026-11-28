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
//!
//! An entry has a stored form, which [`super::store`] writes and reads: the
//! derive of [`Entry`] names each kind with `kind`, and the store adds the
//! version of its line format around it.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::codec::{cut_bytes, tidy};
use super::event::{BridgeEvent, DropReason};
use super::{MAX_ENTRIES, MAX_SUMMARY_CHARS, MAX_TEXT_BYTES};

/// The most characters of the heading of a card that its reference row of the
/// dock keeps.
pub const MAX_CARD_TITLE_CHARS: usize = 80;

/// The most characters of the title of one item of a proposal.
pub const MAX_PROPOSAL_TITLE_CHARS: usize = 60;
/// The most characters of the task of one item of a proposal.
pub const MAX_PROPOSAL_TASK_CHARS: usize = 4_000;
/// The most items that one proposal holds.
pub const MAX_PROPOSAL_ITEMS: usize = 6;
/// The most characters of the name of a work item that its reference row keeps.
pub const MAX_WORK_ITEM_NAME_CHARS: usize = 80;

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
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolState {
    Running,
    Done,
    Failed,
}

/// The state of one proposal of the front desk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalState {
    /// The person did not decide yet.
    Open,
    /// The person started the work items of the proposal.
    Started,
    /// The person did not start the work items of the proposal.
    Declined,
    /// A newer proposal took the place of this one while it was open.
    Replaced,
}

/// One item of a proposal: the work item that the person may start.
///
/// The title, the task and the folder are text of a program. Horizon holds them
/// in the conversation only, and it makes a workspace from them when the person
/// presses the control that starts the proposal.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalItem {
    pub title: String,
    pub task: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

/// One entry of a conversation.
///
/// The stored form of an entry is one JSON object with a `kind` field, which
/// [`super::store`] writes and reads.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entry {
    /// A message of the person. `via_front_desk` is true for the message that the
    /// front desk passed to the orchestrator of a work item, and the dock draws
    /// the caption of the front desk above that bubble. An entry of an older
    /// file holds no such field and loads with `false`.
    Person {
        text: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        via_front_desk: bool,
    },
    /// The whole text of one message of the agent so far.
    Agent { message_id: String, text: String },
    /// One tool call of the agent.
    Tool {
        tool_id: String,
        name: String,
        /// What the tool was asked to do: the command of a `Bash` call.
        summary: String,
        /// The first line of the result of the call, once it ended.
        result: Option<String>,
        state: ToolState,
    },
    /// A line that Horizon writes for the person: a refusal, a dropped line or
    /// the end of the program.
    Notice { text: String },
    /// A reference to a card that the program drew in this workspace: the local
    /// id of that card and the heading it held. The dock draws one row that
    /// brings the card into view, so the person reads which card the program
    /// speaks about. A card that is gone reads as a closed row.
    Card { panel: String, title: String },
    /// A proposal of the front desk: the work items that its program read in the
    /// words of the person, which start only when the person presses the control
    /// of the row. `id` is the id that Horizon made for the proposal.
    Proposal {
        id: String,
        items: Vec<ProposalItem>,
        state: ProposalState,
    },
    /// A reference to one work item that the person started from a proposal: the
    /// local id of its workspace and its name. The dock draws one row that shows
    /// that workspace, so the person reads which work item the front desk speaks
    /// about. A work item that left the board reads as a row without a press.
    WorkItem { work_item: String, name: String },
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
    /// Whether the notice of an over-long line of the current turn is in the
    /// list, so one turn gives one such notice and not one for each line.
    too_long_noticed: bool,
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
            too_long_noticed: false,
        }
    }

    /// The conversation of this moment, which a frame holds.
    #[must_use]
    pub fn snapshot(&self) -> Arc<ConversationSnapshot> {
        Arc::clone(&self.snapshot)
    }

    /// The conversation of a stored history, which a restart shows before its
    /// program runs: the entries, the session id that the history resumed, and
    /// no turn in flight. The link is [`Link::NotStarted`], because no program
    /// of this run named a session yet.
    #[must_use]
    pub fn restored(entries: Vec<Entry>, session_id: Option<String>) -> Self {
        let mut conversation = Self::new();
        let snapshot = Arc::make_mut(&mut conversation.snapshot);
        snapshot.version = 1;
        snapshot.session_id = session_id;
        snapshot.entries = entries;
        Self::trim(snapshot, &mut conversation.capped, &mut conversation.cap_text);
        conversation
    }

    /// Puts a stored history in front of the entries that this conversation
    /// holds. A frame that already showed a message of the person sees the
    /// earlier entries above it.
    pub fn prepend(&mut self, entries: Vec<Entry>) {
        if entries.is_empty() {
            return;
        }
        let snapshot = Arc::make_mut(&mut self.snapshot);
        snapshot.version += 1;
        snapshot.entries.splice(0..0, entries);
        Self::trim(snapshot, &mut self.capped, &mut self.cap_text);
    }

    /// Appends one message of the person and marks the conversation busy.
    pub fn person_said(&mut self, text: &str) {
        self.person_said_via(text, false);
    }

    /// Appends one message of the person and marks the conversation busy.
    /// `via_front_desk` marks the message that the front desk passed to this
    /// conversation: the dock draws the caption of the front desk above it.
    pub fn person_said_via(&mut self, text: &str, via_front_desk: bool) {
        let snapshot = Arc::make_mut(&mut self.snapshot);
        snapshot.version += 1;
        snapshot.busy = true;
        snapshot.entries.push(Entry::Person {
            text: cut_bytes(text, MAX_TEXT_BYTES).to_owned(),
            via_front_desk,
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

    /// Appends one line that Horizon writes for a turn of its own, such as the
    /// wake of an orchestrator after a press, and marks the conversation busy.
    /// The wake line itself is no message of the person, so it adds no [`Entry::Person`].
    pub fn system_said(&mut self, text: &str) {
        let snapshot = Arc::make_mut(&mut self.snapshot);
        snapshot.version += 1;
        snapshot.busy = true;
        snapshot.entries.push(Entry::Notice {
            text: tidy(text, MAX_SUMMARY_CHARS),
        });
        Self::trim(snapshot, &mut self.capped, &mut self.cap_text);
    }

    /// Marks the conversation busy for a turn that Horizon started with one line of
    /// its own, such as the line that names a proposal that the person started. The
    /// turn holds no text of the person, so it adds no entry: the notice of such a
    /// turn, when there is one, is the words of its caller.
    pub fn turn_started(&mut self) {
        let snapshot = Arc::make_mut(&mut self.snapshot);
        snapshot.version += 1;
        snapshot.busy = true;
    }

    /// Adds the reference row of the card `panel`, or moves the row that the
    /// card already has to its heading now. One card keeps one row, so a second
    /// create or an update of the same card changes that row and adds none.
    /// The heading is untrusted text of the program, so it is cut here.
    pub fn card_drawn(&mut self, panel: &str, title: &str) {
        let title = tidy(title, MAX_CARD_TITLE_CHARS);
        let snapshot = Arc::make_mut(&mut self.snapshot);
        snapshot.version += 1;
        let found = snapshot
            .entries
            .iter()
            .position(|entry| matches!(entry, Entry::Card { panel: shown, .. } if shown.as_str() == panel));
        match found {
            Some(index) => {
                if let Entry::Card { title: shown, .. } = &mut snapshot.entries[index] {
                    *shown = title;
                }
            }
            None => snapshot.entries.push(Entry::Card {
                panel: panel.to_owned(),
                title,
            }),
        }
        Self::trim(snapshot, &mut self.capped, &mut self.cap_text);
    }

    /// Appends one proposal of the front desk, and marks every older proposal
    /// that is still open as replaced. One conversation holds at most one open
    /// proposal, because the press of the person decides one of them. The title
    /// and the task of an item are text of a program, so they are cut here.
    pub fn proposal(&mut self, id: &str, items: &[ProposalItem]) {
        let items: Vec<ProposalItem> = items
            .iter()
            .take(MAX_PROPOSAL_ITEMS)
            .map(|item| ProposalItem {
                title: tidy(&item.title, MAX_PROPOSAL_TITLE_CHARS),
                task: tidy(&item.task, MAX_PROPOSAL_TASK_CHARS),
                folder: item.folder.clone(),
            })
            .collect();
        let snapshot = Arc::make_mut(&mut self.snapshot);
        snapshot.version += 1;
        for entry in &mut snapshot.entries {
            if let Entry::Proposal { state, .. } = entry
                && *state == ProposalState::Open
            {
                *state = ProposalState::Replaced;
            }
        }
        snapshot.entries.push(Entry::Proposal {
            id: id.to_owned(),
            items,
            state: ProposalState::Open,
        });
        Self::trim(snapshot, &mut self.capped, &mut self.cap_text);
    }

    /// Moves the proposal `id` to `state`. A proposal that this conversation does
    /// not hold changes nothing.
    pub fn proposal_state(&mut self, id: &str, state: ProposalState) {
        let snapshot = Arc::make_mut(&mut self.snapshot);
        let Some(entry) = snapshot.entries.iter_mut().find_map(|entry| match entry {
            Entry::Proposal {
                id: shown,
                state: shown_state,
                ..
            } if shown == id => Some(shown_state),
            _ => None,
        }) else {
            return;
        };
        *entry = state;
        snapshot.version += 1;
    }

    /// Adds the reference row of the work item `work_item`, or moves the row that
    /// it already has to its name now. One work item keeps one row, as a card
    /// does. The name is text that Horizon makes from the title of a proposal, so
    /// it is cut here.
    pub fn work_item_row(&mut self, work_item: &str, name: &str) {
        let name = tidy(name, MAX_WORK_ITEM_NAME_CHARS);
        let snapshot = Arc::make_mut(&mut self.snapshot);
        snapshot.version += 1;
        let found = snapshot
            .entries
            .iter()
            .position(|entry| matches!(entry, Entry::WorkItem { work_item: shown, .. } if shown == work_item));
        match found {
            Some(index) => {
                if let Entry::WorkItem { name: shown, .. } = &mut snapshot.entries[index] {
                    *shown = name;
                }
            }
            None => snapshot.entries.push(Entry::WorkItem {
                work_item: work_item.to_owned(),
                name,
            }),
        }
        Self::trim(snapshot, &mut self.capped, &mut self.cap_text);
    }

    /// Applies one event of the program.
    pub fn apply(&mut self, event: BridgeEvent) {
        let snapshot = Arc::make_mut(&mut self.snapshot);
        snapshot.version = snapshot.version.saturating_add(1);
        match event {
            BridgeEvent::SessionStarted { session_id } => {
                snapshot.link = Link::Running;
                snapshot.session_id = Some(tidy(&session_id, MAX_SUMMARY_CHARS));
            }
            BridgeEvent::TurnStarted => {
                snapshot.busy = true;
                self.too_long_noticed = false;
            }
            BridgeEvent::AgentText { message_id, text } => apply_agent_text(snapshot, &message_id, &text),
            BridgeEvent::ToolStarted { tool_id, name, summary } => snapshot.entries.push(Entry::Tool {
                tool_id,
                name,
                summary,
                result: None,
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
                // One turn ends every tool call that it left running: a result
                // line that the reader could not read must not leave a row of
                // the list that says that a tool still runs.
                settle_tools(snapshot, failed);
                if failed {
                    snapshot.entries.push(Entry::Notice {
                        text: "the turn failed".to_owned(),
                    });
                }
            }
            BridgeEvent::Dropped { reason } => apply_drop(
                snapshot,
                reason,
                &mut self.dropped,
                &mut self.dropped_text,
                &mut self.too_long_noticed,
            ),
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

/// Moves one tool call to its end. The summary of the start stays, because it
/// says what the tool was asked to do, and the first line of the result goes to
/// `result`. A result of a tool that this conversation did not see start changes
/// nothing.
fn apply_tool_end(snapshot: &mut ConversationSnapshot, tool_id: &str, failed: bool, summary: &str) {
    let found = snapshot
        .entries
        .iter()
        .rposition(|entry| matches!(entry, Entry::Tool { tool_id: id, .. } if id.as_str() == tool_id));
    if let Some(index) = found
        && let Entry::Tool {
            state: shown,
            result: kept,
            ..
        } = &mut snapshot.entries[index]
    {
        *shown = if failed { ToolState::Failed } else { ToolState::Done };
        *kept = first_line(summary);
    }
}

/// Ends every tool call that is still running. One program serves one turn at a
/// time, so an ended turn leaves no tool of this list running: without this, a
/// tool result that the reader could not read leaves its row running for the rest
/// of the conversation.
fn settle_tools(snapshot: &mut ConversationSnapshot, failed: bool) {
    let state = if failed { ToolState::Failed } else { ToolState::Done };
    for entry in &mut snapshot.entries {
        if let Entry::Tool { state: shown, .. } = entry
            && *shown == ToolState::Running
        {
            *shown = state;
        }
    }
}

/// The first line of `text`, cut like a summary, or `None` when it is empty.
fn first_line(text: &str) -> Option<String> {
    let line = text.lines().next().unwrap_or_default();
    if line.trim().is_empty() {
        None
    } else {
        Some(tidy(line, MAX_SUMMARY_CHARS))
    }
}

/// Counts one refused line in one notice, so a program that sends bad lines
/// keeps one line of the conversation and not a page. A line above the bound of
/// the reader also gets one notice of its own for each turn: the line that ends a
/// turn can be such a line, so the person reads why a turn ended without its
/// result.
fn apply_drop(
    snapshot: &mut ConversationSnapshot,
    reason: DropReason,
    dropped: &mut u32,
    dropped_text: &mut String,
    too_long_noticed: &mut bool,
) {
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
    if reason == DropReason::TooLong && !*too_long_noticed {
        *too_long_noticed = true;
        snapshot.entries.push(Entry::Notice {
            text: TOO_LONG_NOTICE.to_owned(),
        });
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

/// The text of the notice of a line of the program that the reader could not
/// hold. It is the words of Horizon and names its own cause, so no text of the
/// line reaches it.
const TOO_LONG_NOTICE: &str = "A line of the program was too long to read";

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
