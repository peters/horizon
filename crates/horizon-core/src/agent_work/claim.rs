//! The turn that the state of one launch opens, holds and ends.
//!
//! A panel hears the state of its program from the title of its terminal
//! ([`TitleState`]) or from a report of the Program Status Protocol
//! ([`ProgramStatus`]). The protocols differ, and the claim that they make is one:
//! a [`TurnLedger`], a number for the turn of the launch, the last state the source
//! named, and the ready gate of the launch. A source translates a state into
//! [`Step`]s and [`Claim`] applies them, so the four words of a row have one path.
//!
//! A turn is numbered from a counter of the launch, so a repeated state does no
//! work and a state of a turn that ended cannot end another one.

use super::{HookEvent, TurnLedger, TurnState};

/// What one state of a [`ClaimSource`] does to the claim of its launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The state ends the start-up of the launch: from here its states claim the
    /// turns of the person.
    Ready,
    /// The turn that is open ends once the state stands for the settle time.
    Settle,
    /// The state opens a turn, or continues the turn that is open.
    Open,
    /// The state waits for an answer of the person.
    Wait,
    /// The turn that is open ends in this state.
    End(TurnState),
    /// The source removed the record of the panel, so the screen answers again.
    Forget,
}

/// One protocol that names the state of a panel, translated into [`Step`]s. The
/// parser and the per-kind rule of the protocol stay with the protocol.
pub trait ClaimSource: Copy + Default + PartialEq {
    /// The session that the ledger of the claim names: the states of one launch.
    const SESSION: &'static str;
    /// The tag of the ids of the turns that the source opens.
    const TURN_TAG: &'static str;
    /// How long a plain state of the source must stand before it ends an open
    /// turn, or `None` for a source whose protocol ends a turn itself.
    const SETTLE_MILLIS: Option<i64>;
    /// What one state of the source does to the claim of a launch, which is ready
    /// or still loading.
    fn steps(self, ready: bool) -> &'static [Step];
    /// Whether the claim answers for the panel after it heard `self`.
    fn answers(self, ledger: &TurnLedger) -> bool;
}

/// The titles of one launch of a program, and the turn that they claim.
pub type TitleClaim = Claim<super::TitleState>;

/// The reports of one launch of a program, and the turn that they claim.
pub type ProgramClaim = Claim<super::ProgramStatus>;

/// The states of one launch of one source, and the turn that they claim.
#[derive(Clone, Debug, Default)]
pub struct Claim<S: ClaimSource> {
    ledger: TurnLedger,
    /// The number of the turn that the states of this launch opened.
    turn: u32,
    /// The last state that the source named, so a repeated state does no work.
    heard: Option<S>,
    /// When the plain state of the source started to stand while a turn was open.
    plain_since_millis: Option<i64>,
    /// Whether the state that ends the start-up of the launch has arrived.
    ready: bool,
}

impl<S: ClaimSource> Claim<S> {
    /// Whether the states of this launch answer for the panel, which they do from
    /// the first state that follows the ready state of the launch.
    #[must_use]
    pub fn answers(&self) -> bool {
        self.heard.is_some_and(|state| state.answers(&self.ledger))
    }

    /// The ledger of the claim, read the way the record of a launch with hooks is.
    #[must_use]
    pub fn ledger(&self) -> &TurnLedger {
        &self.ledger
    }

    /// Records one state of the source. A state that the claim already holds
    /// changes nothing, and so does a state that claims nothing.
    pub fn hear(&mut self, state: S, now_millis: i64) {
        if self.heard == Some(state) {
            return;
        }
        let steps = state.steps(self.ready);
        if steps.is_empty() {
            return;
        }
        self.heard = Some(state);
        for step in steps {
            self.apply_step(*step, now_millis);
        }
    }

    /// Ends the open turn when the plain state of the source has stood for the
    /// settle time, and answers whether an open turn ended.
    ///
    /// A turn that the plain state ends while an approval waited is a turn that the
    /// person stopped: nothing answers the request, and the ledger keeps such a
    /// turn blocked, so the wait goes first and the turn ends interrupted.
    pub fn settle(&mut self, now_millis: i64) -> bool {
        let Some(settle) = S::SETTLE_MILLIS else {
            return false;
        };
        let Some(since) = self.plain_since_millis else {
            return false;
        };
        if now_millis.saturating_sub(since) < settle {
            return false;
        }
        self.plain_since_millis = None;
        if !self.open() {
            return false;
        }
        let state = if self.ledger.state == TurnState::Blocked {
            TurnState::Interrupted
        } else {
            TurnState::Finished
        };
        self.end(state, now_millis);
        true
    }

    /// Applies one step of a state to the claim.
    fn apply_step(&mut self, step: Step, now_millis: i64) {
        match step {
            Step::Ready => self.become_ready(now_millis),
            Step::Settle => {
                if self.open() {
                    self.plain_since_millis = Some(now_millis);
                }
            }
            Step::Open => self.open_turn(now_millis),
            Step::Wait => self.wait(now_millis),
            Step::End(state) => self.end(state, now_millis),
            Step::Forget => {
                // The source removed a record and not the launch, so the program
                // that reports again is the ready program that it was. The state
                // stays the one this claim heard, so a repeated one does no work.
                let (heard, ready) = (self.heard, self.ready);
                *self = Self {
                    heard,
                    ready,
                    ..Self::default()
                };
            }
        }
    }

    /// Records the ready state of a launch: from here its states claim the turns of
    /// the person.
    ///
    /// A wait before that state was the program's own question while it loaded. The
    /// person answered it, so the launch forgets the turn that the wait opened
    /// instead of ending it: its end would say **Done** for a launch that has not
    /// started to work yet.
    fn become_ready(&mut self, now_millis: i64) {
        if self.ready {
            return;
        }
        self.ready = true;
        if self.open() {
            self.ledger.apply(&Self::event("SessionStart", None, None), now_millis);
        }
    }

    /// Opens a turn, or continues the turn that is open.
    fn open_turn(&mut self, now_millis: i64) {
        self.plain_since_millis = None;
        let prompt = self.next_prompt();
        self.ledger
            .apply(&Self::event("UserPromptSubmit", Some(prompt), None), now_millis);
    }

    /// Records a wait for the person, which opens a turn when the claim holds
    /// none: a question belongs to a turn.
    fn wait(&mut self, now_millis: i64) {
        self.plain_since_millis = None;
        let prompt = self.next_prompt();
        if !self.open() {
            self.ledger
                .apply(&Self::event("UserPromptSubmit", Some(prompt.clone()), None), now_millis);
        }
        let wait = self.wait_id();
        self.ledger
            .apply(&Self::event("PermissionRequest", Some(prompt), Some(wait)), now_millis);
    }

    /// Ends the turn that the claim holds open, and nothing when it holds none.
    ///
    /// A denial and an interruption leave a request that no answer closed, so it is
    /// removed before the turn closes: the ledger keeps a turn whose question is
    /// still open blocked, and a denial would stay **Needs you** for the rest of
    /// the turn.
    fn end(&mut self, state: TurnState, now_millis: i64) {
        if !self.open() {
            return;
        }
        let prompt = self.ledger.prompt_id.clone();
        if self.ledger.state == TurnState::Blocked {
            let wait = self.wait_id();
            self.ledger
                .apply(&Self::event("PostToolUse", prompt.clone(), Some(wait)), now_millis);
        }
        let name = end_event(state);
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
        format!("{}-{}", S::TURN_TAG, self.turn)
    }

    /// The id of the request that the wait of the open turn leaves in the ledger.
    fn wait_id(&self) -> String {
        format!("{}-wait-{}", S::TURN_TAG, self.turn)
    }

    /// Whether the claim holds a turn that has not ended.
    fn open(&self) -> bool {
        matches!(self.ledger.state, TurnState::Working | TurnState::Blocked) && !self.ledger.turn_closed
    }

    /// One event of the ledger of the claim: the session is the launch that these
    /// states belong to, and no text of the source takes part.
    fn event(name: &str, prompt: Option<String>, tool: Option<String>) -> HookEvent {
        HookEvent {
            session_id: S::SESSION.to_owned(),
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

/// The lifecycle event that ends a turn in `state`.
const fn end_event(state: TurnState) -> &'static str {
    match state {
        TurnState::Failed => "StopFailure",
        TurnState::Interrupted => "Interrupt",
        _ => "Stop",
    }
}
