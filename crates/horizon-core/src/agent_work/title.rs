//! The state that a program writes into the title of its own terminal.
//!
//! Some agents write their state into the title of the terminal that runs them:
//! a spinner frame while they work, a flagged line while they wait for the
//! person, and a plain title when nothing runs. The title is the program's own
//! statement about its own panel, so it enters the same ledger as a lifecycle
//! event of a hook: the same [`TurnLedger`], the same four words, and the same
//! rule for a turn that the person did not see. A title and a hook differ only in
//! how the event arrives at Horizon.
//!
//! The claim of these titles is the one in [`super::Claim`]. A program whose
//! titles never take one of the forms of its kind keeps the signal of its screen.

use crate::PanelKind;

use super::{ClaimSource, Step, TurnLedger};

/// The state that the title of a panel claims.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TitleState {
    /// The title names no state, so the screen of the panel decides.
    #[default]
    Plain,
    /// The program works on a turn.
    Working,
    /// The program waits for an answer of the person.
    NeedsYou,
}

/// The longest title that still counts as the claim of a program, in bytes. A
/// longer title is a line of text about something else and claims nothing.
pub const MAX_STATE_TITLE_BYTES: usize = 512;

/// How long a plain title must stand before it ends an open turn, in
/// milliseconds.
///
/// A program may write a plain title between two parts of one turn, and the turn
/// would end and open again. The end therefore waits for this time of plain
/// title, and a working or waiting title in that window cancels it. The window is
/// a guess from the titles that a probe of the real program read; no live run has
/// measured a plain title in the middle of a turn yet.
pub const TITLE_SETTLE_MILLIS: i64 = 400;

/// The two frames of the line that a program writes while an approval waits.
const CODEX_APPROVAL: [&str; 2] = ["[ . ] Action Required", "[ ! ] Action Required"];

/// The first part of the title that separates the words of the claim.
const TITLE_FIELD_SEPARATOR: &str = " | ";

/// The state that the title of a Codex panel claims.
///
/// Codex writes a braille spinner frame and the name of the folder while it
/// works, and one of two frames of `Action Required` with the request and the
/// folder while an approval waits. The words of the request and the folder are
/// the text of the program: no store keeps them, and the name of the panel is the
/// folder alone, which [`title_name`] reads without the state around it.
fn codex_title_state(title: &str) -> TitleState {
    let mut characters = title.chars();
    if let Some(first) = characters.next()
        && is_spinner_frame(first)
        && characters.next().is_none_or(|next| next == ' ')
    {
        return TitleState::Working;
    }
    if approval_line(title).is_some() {
        return TitleState::NeedsYou;
    }
    TitleState::Plain
}

/// The line of a wait without the frame before it, or `None` for a title that is
/// no wait of that form.
fn approval_line(title: &str) -> Option<&str> {
    CODEX_APPROVAL.iter().find_map(|frame| {
        title
            .strip_prefix(frame)
            .filter(|rest| rest.is_empty() || rest.starts_with(TITLE_FIELD_SEPARATOR))
    })
}

/// Whether `character` is one frame of the braille spinner of a program.
const fn is_spinner_frame(character: char) -> bool {
    character >= '\u{2800}' && character <= '\u{28FF}'
}

/// The rule of an agent kind that writes its state into the title of its own
/// terminal, or `None` for a kind that reports nothing there. Another program
/// with a state title is one more row of this table.
fn title_rule(kind: PanelKind) -> Option<fn(&str) -> TitleState> {
    match kind {
        PanelKind::Codex => Some(codex_title_state),
        _ => None,
    }
}

/// Whether the agent of `kind` writes its state into the title of its terminal,
/// so a panel of that kind can report more than the signal of its screen.
#[must_use]
pub fn title_reports_state(kind: PanelKind) -> bool {
    title_rule(kind).is_some()
}

/// The state that `title` claims for a panel of `kind`.
///
/// A title of a kind without a rule, an empty title and a title beyond
/// [`MAX_STATE_TITLE_BYTES`] claim nothing, and text that is not a whole glyph
/// claims nothing either.
#[must_use]
pub fn title_state(kind: PanelKind, title: &str) -> TitleState {
    let Some(rule) = title_rule(kind) else {
        return TitleState::Plain;
    };
    if title.is_empty() || title.len() > MAX_STATE_TITLE_BYTES {
        return TitleState::Plain;
    }
    rule(title)
}

/// The name that the state title `title` gives a panel of `kind`: the title
/// without the part of it that names the state.
///
/// A working title gives the words after its spinner frame, and a waiting title
/// gives the last part of its line after its last ` | `, which is the folder of
/// the program. The name of a panel stands still while the state of its program
/// changes, so the row of a panel does not blink and its text is laid out once.
///
/// Answers `None` for a title that names no state of this kind, and `Some("")` for
/// a state title that leaves no name: the panel keeps the name that it had before.
#[must_use]
pub fn title_name(kind: PanelKind, title: &str) -> Option<&str> {
    match title_state(kind, title) {
        TitleState::Plain => None,
        TitleState::Working => {
            // One braille glyph and the space that follows it name no panel.
            let rest = title.char_indices().nth(1).map_or("", |(index, _)| &title[index..]);
            Some(rest.strip_prefix(' ').unwrap_or(rest))
        }
        TitleState::NeedsYou => Some(
            approval_line(title)
                .unwrap_or_default()
                .rsplit(TITLE_FIELD_SEPARATOR)
                .next()
                .unwrap_or_default(),
        ),
    }
}

/// The title of a program, translated into the steps of the claim of its launch.
///
/// A title differs from a report of the protocol in one rule: the plain title of a
/// launch is its ready state, and it ends the open turn only once it stands for
/// [`TITLE_SETTLE_MILLIS`], because a program may write a plain title between two
/// parts of one turn.
impl ClaimSource for TitleState {
    const SESSION: &'static str = "title";
    const TURN_TAG: &'static str = "title";
    const SETTLE_MILLIS: Option<i64> = Some(TITLE_SETTLE_MILLIS);

    fn steps(self, ready: bool) -> &'static [Step] {
        match self {
            // A frame of the spinner before the ready title is the program
            // loading, and it claims no turn of the person. The claim does not
            // hold it either, so every frame of the start-up does the same work.
            TitleState::Working if !ready => &[],
            TitleState::Working => &[Step::Open],
            TitleState::NeedsYou => &[Step::Wait],
            TitleState::Plain => &[Step::Ready, Step::Settle],
        }
    }

    fn answers(self, ledger: &TurnLedger) -> bool {
        ledger.heard_an_event()
    }
}

#[cfg(test)]
mod tests;
