//! The state that a program reports about the panel it runs in.
//!
//! The Program Status Protocol (OSC 7501, revision 0.4) is an open protocol: a
//! program writes `ESC ] 7501 ; state=<word>[:key=value]... ST` to its terminal and
//! the terminal keeps that state for the panel. A program needs no configuration
//! and no file of Horizon, so this source is stronger than the title of a program:
//! a title can be changed by the person, and it cannot tell a finished turn from an
//! interrupted or a failed one.
//!
//! The bytes arrive from the reader of the panel's PTY (see `horizon_pty_tap`),
//! which reads them on their way to the VT parser. The report of one panel is
//! untrusted input: the parser keeps no text of it, it ignores every key that it
//! does not know, and it answers `Unknown` for a report that this round does not
//! read. A `state` that the claim hears enters the same [`TurnLedger`] that a hook
//! event and the title claim write, so the four words of the sidebar have one path.

use crate::PanelKind;

use super::{HookEvent, TurnLedger, TurnState};

/// The longest payload of one report that this parser reads, in bytes.
///
/// The protocol caps one report at 4096 bytes and lets a terminal choose lower
/// limits; a report beyond that limit is discarded whole by the reader, and this
/// bound is the second half of the same rule.
pub const MAX_REPORT_BYTES: usize = 256;

/// The longest key of one pair, in bytes, as the protocol limits it.
const MAX_KEY_BYTES: usize = 16;

/// The reply of the feature detection query of the protocol, `ESC ] 7501 ; ? ST`.
///
/// A supporting terminal answers with `?` and may list the states and kinds that it
/// accepts. This terminal reads every state of revision 0.4 and shows every `kind`
/// of a wait as one word, so it lists all of them. The reply is one constant
/// sequence and it holds nothing of any report.
pub const DETECTION_REPLY: &[u8] =
    b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";

/// The state that the report of a program names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProgramStatus {
    /// A report of another program, of a record beneath the root, of a state that
    /// the protocol does not name, or of a protocol this parser does not read.
    #[default]
    Unknown,
    /// At rest, waiting for the next instruction.
    Idle,
    /// Running a turn.
    Working,
    /// Waiting for the person, whatever the `kind` of the wait is.
    Blocked,
    /// A turn that finished.
    Done,
    /// A turn that failed.
    Failed,
    /// Remove the record of the panel.
    Clear,
}

/// The `app` value that the program of `kind` reports, or `None` for a kind whose
/// program reports no state of its own. Another program that speaks the protocol
/// is one more row of this table.
fn program_app(kind: PanelKind) -> Option<&'static str> {
    match kind {
        PanelKind::Codex => Some("codex"),
        _ => None,
    }
}

/// Whether the agent of `kind` reports its state with the protocol, so a panel of
/// that kind reads more than the signal of its screen and more than its title.
#[must_use]
pub fn program_status_reports_state(kind: PanelKind) -> bool {
    program_app(kind).is_some()
}

/// The state that one payload of a report claims for a panel of `kind`.
///
/// A report is read only whole: a report whose `app` is not the program of the
/// panel, whose `id` names a record beneath the root, or whose `state` is not a
/// word of the protocol claims nothing. The `kind` of a wait and every other key
/// of the report are recognised and dropped, because one word of the sidebar
/// covers every wait and this round keeps no text of a program.
#[must_use]
pub fn program_status(kind: PanelKind, payload: &[u8]) -> ProgramStatus {
    let Some(app) = program_app(kind) else {
        return ProgramStatus::Unknown;
    };
    if payload.len() > MAX_REPORT_BYTES {
        return ProgramStatus::Unknown;
    }
    let Ok(report) = std::str::from_utf8(payload) else {
        return ProgramStatus::Unknown;
    };
    let mut state = None;
    let mut app_matches = false;
    for pair in report.split(':') {
        // A pair without `=` or outside the character set of the protocol is
        // skipped, and the rest of the report is still read.
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if !is_key(key) || !is_value(value) {
            continue;
        }
        match key {
            "state" => state = Some(value),
            // The last value of a repeated key wins, as the protocol says.
            "app" => app_matches = value == app,
            // A record beneath the root belongs to a worker of the program, and
            // this round holds one record for the panel.
            "id" if !value.is_empty() => return ProgramStatus::Unknown,
            _ => {}
        }
    }
    if !app_matches {
        return ProgramStatus::Unknown;
    }
    match state {
        Some("idle") => ProgramStatus::Idle,
        Some("working") => ProgramStatus::Working,
        Some("blocked") => ProgramStatus::Blocked,
        Some("done") => ProgramStatus::Done,
        Some("error") => ProgramStatus::Failed,
        Some("clear") => ProgramStatus::Clear,
        _ => ProgramStatus::Unknown,
    }
}

/// Whether `key` is a key of the protocol: one or more lowercase letters.
fn is_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= MAX_KEY_BYTES && key.bytes().all(|byte| byte.is_ascii_lowercase())
}

/// Whether `value` holds only the bytes that the protocol allows in a value.
fn is_value(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b',' | b'+' | b'/' | b'=' | b'-'))
}

/// The state that the reports of one launch claim, and the turn that they hold.
///
/// A report names no turn, so a turn is numbered from a counter of the launch: a
/// report that repeats the state the claim already holds does no work, a `done`
/// closes only the turn that is open, and a report of a turn that already ended
/// changes nothing. A `clear` forgets the launch, and the claim answers again from
/// the next `working` or `blocked` that arrives.
///
/// The first `idle` of a launch is the moment its program is ready for the person.
/// A program that reports `working` while it loads claims no turn, and a `blocked`
/// report before that moment is the program's own question, which the person must
/// answer, so it claims **Needs you** like any other wait.
#[derive(Clone, Debug, Default)]
pub struct ProgramClaim {
    ledger: TurnLedger,
    /// The number of the turn that the reports of this launch opened.
    turn: u32,
    /// The last state that a report named, so a repeated report does no work.
    heard: Option<ProgramStatus>,
    /// Whether the `idle` report that says the program is ready for the person has
    /// arrived. The reports of a launch before it are its start-up.
    ready: bool,
}

impl ProgramClaim {
    /// Whether a program of this launch reported a state. A launch that has
    /// reported once answers for its panel, and its title is no longer read. The
    /// `clear` of a program removes that answer again.
    #[must_use]
    pub fn answers(&self) -> bool {
        matches!(self.heard, Some(status) if status != ProgramStatus::Clear)
    }

    /// Whether an `idle` report of this launch has arrived, which is the moment
    /// the program is ready for the person.
    #[must_use]
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// The ledger of the claim, read the way the record of a launch with hooks is.
    #[must_use]
    pub fn ledger(&self) -> &TurnLedger {
        &self.ledger
    }

    /// Records one report of the protocol.
    pub fn hear(&mut self, status: ProgramStatus, now_millis: i64) {
        if status == ProgramStatus::Unknown || self.heard == Some(status) {
            return;
        }
        if status == ProgramStatus::Clear {
            // The program removes its record: the panel answers from the screen
            // again, as a panel that started now does. The report stays the state
            // that this claim heard, so a repeated `clear` does no work. It
            // removes a record and not the launch, so the program that reports
            // again is the ready program that it was.
            let ready = self.ready;
            *self = Self {
                heard: Some(ProgramStatus::Clear),
                ready,
                ..Self::default()
            };
            return;
        }
        match status {
            ProgramStatus::Idle => self.become_ready(now_millis),
            // A report that the program works before its first idle is the
            // program loading, and no turn of the person runs in it.
            ProgramStatus::Working if !self.ready => return,
            ProgramStatus::Unknown
            | ProgramStatus::Working
            | ProgramStatus::Blocked
            | ProgramStatus::Done
            | ProgramStatus::Failed
            | ProgramStatus::Clear => {}
        }
        self.heard = Some(status);
        match status {
            ProgramStatus::Unknown | ProgramStatus::Clear => {}
            ProgramStatus::Working => {
                let prompt = self.next_prompt();
                self.ledger
                    .apply(&Self::event("UserPromptSubmit", Some(prompt), None), now_millis);
            }
            ProgramStatus::Blocked => {
                let prompt = self.next_prompt();
                // A wait of the first report of a launch opens the turn it waits
                // in, because a question belongs to a turn.
                if !self.open() {
                    self.ledger
                        .apply(&Self::event("UserPromptSubmit", Some(prompt.clone()), None), now_millis);
                }
                let wait = format!("status-wait-{}", self.turn);
                self.ledger
                    .apply(&Self::event("PermissionRequest", Some(prompt), Some(wait)), now_millis);
            }
            ProgramStatus::Done => self.end(TurnState::Finished, now_millis),
            ProgramStatus::Failed => self.end(TurnState::Failed, now_millis),
            ProgramStatus::Idle => self.end(TurnState::Interrupted, now_millis),
        }
    }

    /// Records the `idle` report of a launch: from here the reports of the launch
    /// claim the turns of the person.
    ///
    /// A wait before that report was the program's own question while it loaded.
    /// The person answered it, so the launch forgets the turn that the wait opened
    /// instead of ending it: no turn of the person ran in it, and its end would say
    /// **Done** for a launch that has not started to work yet.
    fn become_ready(&mut self, now_millis: i64) {
        if self.ready {
            return;
        }
        self.ready = true;
        if self.open() {
            self.ledger.apply(&Self::event("SessionStart", None, None), now_millis);
        }
    }

    /// Ends the turn that the claim holds open, and nothing when it holds none: a
    /// report of a turn that ended cannot end another one.
    ///
    /// A turn that ends while an approval waited is a turn that the person stopped
    /// or answered: a denial and an interruption leave a request that no answer
    /// closed, so it is removed before the turn closes. The ledger keeps a turn
    /// whose question is still open blocked, and a denial would stay **Needs you**
    /// for the rest of the turn.
    fn end(&mut self, state: TurnState, now_millis: i64) {
        if !self.open() {
            return;
        }
        let prompt = self.ledger.prompt_id.clone();
        if self.ledger.state == TurnState::Blocked {
            let wait = format!("status-wait-{}", self.turn);
            self.ledger
                .apply(&Self::event("PostToolUse", prompt.clone(), Some(wait)), now_millis);
        }
        let name = match state {
            TurnState::Failed => "StopFailure",
            TurnState::Interrupted => "Interrupt",
            _ => "Stop",
        };
        self.ledger.apply(&Self::event(name, prompt, None), now_millis);
    }

    /// The id of the turn that the claim holds open, or the id of a new turn.
    fn next_prompt(&mut self) -> String {
        if self.open()
            && let Some(prompt) = self.ledger.prompt_id.clone()
        {
            return prompt;
        }
        self.turn = self.turn.saturating_add(1);
        format!("status-{}", self.turn)
    }

    /// Whether the claim holds a turn that has not ended.
    fn open(&self) -> bool {
        matches!(self.ledger.state, TurnState::Working | TurnState::Blocked) && !self.ledger.turn_closed
    }

    /// One event of the ledger of the claim. The session is the launch whose
    /// reports these are, and no text of a report takes part.
    fn event(name: &str, prompt: Option<String>, tool: Option<String>) -> HookEvent {
        HookEvent {
            session_id: CLAIM_SESSION.to_owned(),
            prompt_id: prompt,
            hook_event_name: name.to_owned(),
            reason: None,
            source: None,
            tool_name: None,
            tool_use_id: tool,
            background_tasks: None,
        }
    }
}

/// The session that the ledger of a claim names: the reports of one launch.
const CLAIM_SESSION: &str = "program-status";

#[cfg(test)]
mod tests;
