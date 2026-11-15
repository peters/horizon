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
//! The claim of a launch answers from the first working or waiting title that
//! follows the ready title of the launch. A program whose titles never take one of
//! the forms of its kind keeps the signal of its screen, and a spinner frame that
//! repeats the state that the claim already holds does no work: the title of a
//! working program changes about ten times a second, and only a change of the
//! state is an event.

use crate::PanelKind;

use super::{HookEvent, TurnLedger, TurnState};

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

/// The titles of one launch of a program, and the turn that they claim.
///
/// The claim holds a [`TurnLedger`], so a title reaches the four words of the
/// panel through the same fields that a lifecycle event of a hook writes. A turn
/// is numbered from a counter of the launch: a plain title closes the turn that a
/// working or waiting title opened, and a late or repeated title cannot close a
/// turn that a later one opened.
///
/// The first plain title of a launch is the moment its program is ready for the
/// person: a program loads with frames of the spinner, and those frames claim no
/// turn. A wait before that title is the program's own question, which the person
/// must answer, so it claims **Needs you** like any other wait.
#[derive(Clone, Debug, Default)]
pub struct TitleClaim {
    ledger: TurnLedger,
    /// The number of the turn that the titles of this launch opened.
    turn: u32,
    /// The last state that the title claimed, so a repeated frame does no work.
    heard: Option<TitleState>,
    /// When a plain title started to stand while a turn was open.
    plain_since_millis: Option<i64>,
    /// Whether the plain title that says the program is ready for the person has
    /// arrived. The titles of a launch before it are its start-up.
    ready: bool,
}

impl TitleClaim {
    /// Whether the titles of this launch answer for the panel. The claim answers
    /// from the first working or waiting title of the launch that follows its
    /// ready title.
    #[must_use]
    pub fn answers(&self) -> bool {
        self.ledger.heard_an_event()
    }

    /// Whether a plain title of this launch has arrived, which is the moment the
    /// program is ready for the person.
    #[must_use]
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// The ledger of the claim, read the way the record of a launch with hooks is
    /// read.
    #[must_use]
    pub fn ledger(&self) -> &TurnLedger {
        &self.ledger
    }

    /// Records one state of the title. A state that the claim already holds
    /// changes nothing.
    pub fn hear(&mut self, state: TitleState, now_millis: i64) {
        if self.heard == Some(state) {
            return;
        }
        match state {
            TitleState::Plain => self.become_ready(now_millis),
            // A frame of the spinner before the ready title is the program
            // loading, and it claims no turn of the person.
            TitleState::Working if !self.ready => return,
            TitleState::Working | TitleState::NeedsYou => {}
        }
        self.heard = Some(state);
        match state {
            TitleState::Working => {
                self.plain_since_millis = None;
                let prompt = self.next_prompt();
                self.ledger
                    .apply(&Self::event("UserPromptSubmit", Some(prompt), None), now_millis);
            }
            TitleState::NeedsYou => {
                self.plain_since_millis = None;
                let prompt = self.next_prompt();
                // The wait of the first title of a launch opens the turn that it
                // waits in, because a question belongs to a turn.
                if !self.open() {
                    self.ledger
                        .apply(&Self::event("UserPromptSubmit", Some(prompt.clone()), None), now_millis);
                }
                let wait = format!("title-wait-{}", self.turn);
                self.ledger
                    .apply(&Self::event("PermissionRequest", Some(prompt), Some(wait)), now_millis);
            }
            TitleState::Plain => {
                if self.open() {
                    self.plain_since_millis = Some(now_millis);
                }
            }
        }
    }

    /// Ends the open turn when a plain title has stood for the settle time.
    /// Answers whether an open turn ended.
    pub fn settle(&mut self, now_millis: i64) -> bool {
        let Some(since) = self.plain_since_millis else {
            return false;
        };
        if now_millis.saturating_sub(since) < TITLE_SETTLE_MILLIS {
            return false;
        }
        self.plain_since_millis = None;
        if !self.open() {
            return false;
        }
        let prompt = self.ledger.prompt_id.clone();
        self.ledger.apply(&Self::event("Stop", prompt, None), now_millis);
        true
    }

    /// Records the ready title of a launch: from here the titles of the launch
    /// claim the turns of the person.
    ///
    /// A wait before that title was the program's own question while it loaded.
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

    /// The id of the turn that the claim holds open, or the id of a new turn.
    fn next_prompt(&mut self) -> String {
        if self.open()
            && let Some(prompt) = self.ledger.prompt_id.clone()
        {
            return prompt;
        }
        self.turn = self.turn.saturating_add(1);
        format!("title-{}", self.turn)
    }

    /// Whether the claim holds a turn that has not ended.
    fn open(&self) -> bool {
        matches!(self.ledger.state, TurnState::Working | TurnState::Blocked) && !self.ledger.turn_closed
    }

    /// One event of the ledger of the claim. The session is the launch that these
    /// titles belong to, and no text of the title takes part.
    fn event(name: &str, prompt: Option<String>, tool: Option<String>) -> HookEvent {
        HookEvent {
            session_id: TITLE_SESSION.to_owned(),
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

/// The session that the ledger of a claim names: the titles of one launch.
const TITLE_SESSION: &str = "title";

#[cfg(test)]
mod tests;
