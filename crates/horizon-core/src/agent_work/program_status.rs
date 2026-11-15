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
//! read. The claim of these reports is the one in [`super::Claim`].

use crate::PanelKind;

use super::{ClaimSource, Step, TurnLedger, TurnState};

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

/// The report of a program, translated into the steps of the claim of its launch.
///
/// A report names its own state, so it ends a turn itself: an `idle` report after a
/// turn is a turn that the person stopped, and it ends as an interruption.
impl ClaimSource for ProgramStatus {
    const SESSION: &'static str = "program-status";
    const TURN_TAG: &'static str = "status";
    const SETTLE_MILLIS: Option<i64> = None;

    fn steps(self, ready: bool) -> &'static [Step] {
        match self {
            ProgramStatus::Unknown => &[],
            // A report that the program works before its first idle is the
            // program loading, and no turn of the person runs in it. The claim
            // does not hold it either, so every such report does the same work.
            ProgramStatus::Working if !ready => &[],
            ProgramStatus::Working => &[Step::Open],
            ProgramStatus::Blocked => &[Step::Wait],
            ProgramStatus::Idle => &[Step::Ready, Step::End(TurnState::Interrupted)],
            ProgramStatus::Done => &[Step::End(TurnState::Finished)],
            ProgramStatus::Failed => &[Step::End(TurnState::Failed)],
            ProgramStatus::Clear => &[Step::Forget],
        }
    }

    fn answers(self, _ledger: &TurnLedger) -> bool {
        self != ProgramStatus::Clear
    }
}

#[cfg(test)]
mod tests;
