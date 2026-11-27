//! One conversation of the dock: the entries of one workspace, the bridge that
//! speaks for it, and the messages that wait for that bridge.
//!
//! A [`Session`] is the memory of one workspace, keyed by the `local_id` that a
//! restart keeps. It holds the [`Conversation`] of `horizon-core`, which is the
//! list of entries that the dock draws, and it holds the [`Bridge`] of that
//! conversation while the program runs.
//!
//! The start of a bridge starts a process, so it never runs on the frame: the
//! frame queues it on the coordination worker of
//! [`super::super::browser_host_io`] and keeps its `generation`. A result of an
//! earlier generation is stale, because the person stopped the conversation or
//! closed its workspace while the start ran, and the result is then stopped
//! instead of kept. [`Bridge::stop`] can wait, so it runs on that worker too.
//!
//! Every text of the program is untrusted. It enters the conversation only as
//! one entry of `horizon_core::agent_bridge`, which cuts it, and the dock draws
//! it as plain text. The notices of this module are the words of Horizon.

use std::sync::Arc;

use horizon_core::agent_bridge::{
    Bridge, BridgeEvent, ClaudeCodec, Conversation, ConversationSnapshot, ProgramSpec, SessionChoice,
};

/// The notice of a turn that the person stopped.
pub(super) const STOPPED_NOTICE: &str = "Stopped by you";
/// The first words of the notice of a program that did not start.
pub(super) const START_FAILURE: &str = "Claude Code did not start";
/// The notice of an account that reached the limit of its plan and uses overage.
pub(super) const LIMIT_NOTICE: &str = "Claude Code reports that the plan limit is reached. Extra usage is in use.";

/// The state of one conversation, as the second line of the header names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    /// No bridge ran for this workspace yet.
    NotConnected,
    /// The start of a bridge waits on the coordination worker.
    Starting,
    /// A bridge runs for this workspace.
    Running,
    /// The person stopped the bridge.
    Stopped,
    /// The program ended, or it did not start.
    Ended,
}

impl Phase {
    /// The second line of the header: the program, and this state.
    pub(super) fn line(self, busy: bool) -> &'static str {
        match self {
            Self::NotConnected => "Orchestrator \u{b7} Claude Code \u{b7} not connected",
            Self::Starting => "Orchestrator \u{b7} Claude Code \u{b7} starting",
            Self::Running if busy => "Orchestrator \u{b7} Claude Code \u{b7} working",
            Self::Running => "Orchestrator \u{b7} Claude Code \u{b7} ready",
            Self::Stopped => "Orchestrator \u{b7} Claude Code \u{b7} stopped",
            Self::Ended => "Orchestrator \u{b7} Claude Code \u{b7} ended",
        }
    }
}

/// What the worker starts for one conversation, and what the frame must
/// recognise as the answer to this request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Start {
    pub(super) generation: u64,
    pub(super) session: SessionChoice,
}

/// The conversation of one workspace.
pub(super) struct Session {
    conversation: Conversation,
    phase: Phase,
    /// The bridge of this conversation, while its program runs.
    bridge: Option<Bridge>,
    /// The session id that the program named, which a later bridge resumes.
    resume_id: Option<String>,
    /// The messages that the person sent while no bridge was up.
    waiting: Vec<String>,
    /// Counts the starts and the stops of this conversation, so a late answer
    /// of the worker is known to be stale.
    generation: u64,
    /// Whether the limit of the plan was already named to the person for the
    /// bridge that runs.
    limit_noticed: bool,
}

impl Session {
    /// A conversation that holds nothing and runs no program.
    pub(super) fn new() -> Self {
        Self {
            conversation: Conversation::new(),
            phase: Phase::NotConnected,
            bridge: None,
            resume_id: None,
            waiting: Vec::new(),
            generation: 0,
            limit_noticed: false,
        }
    }

    /// The conversation of this moment, which one frame draws.
    pub(super) fn snapshot(&self) -> Arc<ConversationSnapshot> {
        self.conversation.snapshot()
    }

    /// Whether this conversation owns a bridge now.
    #[cfg(test)]
    pub(super) fn has_bridge(&self) -> bool {
        self.bridge.is_some()
    }

    /// Whether the start of a bridge waits on the coordination worker.
    pub(super) fn is_starting(&self) -> bool {
        self.phase == Phase::Starting
    }

    /// The second line of the header.
    pub(super) fn state_line(&self) -> &'static str {
        self.phase.line(self.snapshot().busy)
    }

    /// The cost of the ended turns of this conversation, which the header shows
    /// when it is above zero.
    pub(super) fn cost_usd_total(&self) -> f64 {
        self.snapshot().cost_usd_total
    }

    /// Adds one message of the person. The answer says whether a bridge must
    /// start first: then the message waits for it.
    pub(super) fn say(&mut self, text: &str) -> bool {
        self.conversation.person_said(text);
        let Some(bridge) = self.bridge.take() else {
            self.waiting.push(text.to_owned());
            return true;
        };
        if bridge.send(text.to_owned()).is_ok() {
            self.bridge = Some(bridge);
            return false;
        }
        // The program ended before this message: a new bridge carries it.
        self.waiting.push(text.to_owned());
        true
    }

    /// Takes the request of one start, and makes every start in flight stale.
    pub(super) fn begin_start(&mut self) -> Start {
        self.generation = self.generation.saturating_add(1);
        self.phase = Phase::Starting;
        self.limit_noticed = false;
        let session = match &self.resume_id {
            Some(session_id) => SessionChoice::Resume(session_id.clone()),
            None => SessionChoice::New(horizon_core::new_local_id()),
        };
        Start {
            generation: self.generation,
            session,
        }
    }

    /// Keeps the bridge of a start that answers this generation and sends the
    /// messages that waited for it. It answers the bridge of a stale answer,
    /// which the caller stops, and `None` for a stale answer that failed.
    pub(super) fn adopt(
        &mut self,
        generation: u64,
        outcome: Option<Result<Bridge, horizon_core::agent_bridge::BridgeError>>,
    ) -> Option<Bridge> {
        if generation != self.generation {
            // The person stopped this conversation, or its workspace closed,
            // while the start ran: the bridge of that start is not ours, and the
            // messages that wait belong to the start that replaced it.
            return outcome.and_then(Result::ok);
        }
        match outcome {
            Some(Ok(bridge)) => {
                self.bridge = Some(bridge);
                self.phase = Phase::Running;
                let waiting = std::mem::take(&mut self.waiting);
                for text in waiting {
                    if self.bridge.as_ref().is_some_and(|bridge| bridge.send(text).is_err()) {
                        self.bridge = None;
                        self.phase = Phase::Ended;
                        break;
                    }
                }
                None
            }
            Some(Err(error)) => {
                self.fail(&failure_detail(&error));
                None
            }
            None => {
                self.fail("the start did not answer");
                None
            }
        }
    }

    /// Ends this conversation with the notice of a program that did not start.
    fn fail(&mut self, detail: &str) {
        self.phase = Phase::Ended;
        self.waiting.clear();
        // The message of that start ends with it, so the field offers the send
        // control again.
        self.conversation.apply(BridgeEvent::TurnEnded {
            failed: false,
            duration_ms: None,
            cost_usd: None,
        });
        self.conversation.notice(&format!("{START_FAILURE}: {detail}"));
    }

    /// Reads every event of the bridge of this conversation. It never waits.
    pub(super) fn poll(&mut self) {
        let events = match &self.bridge {
            Some(bridge) => bridge.poll(),
            None => return,
        };
        if events.is_empty() {
            return;
        }
        let mut ended = false;
        for event in events {
            match &event {
                BridgeEvent::SessionStarted { session_id } => self.resume_id = Some(session_id.clone()),
                BridgeEvent::Limit {
                    using_overage: true, ..
                } if !self.limit_noticed => {
                    self.limit_noticed = true;
                    self.conversation.notice(LIMIT_NOTICE);
                }
                BridgeEvent::Ended { .. } => ended = true,
                _ => {}
            }
            self.conversation.apply(event);
        }
        if ended {
            self.bridge = None;
            self.phase = Phase::Ended;
        }
    }

    /// Takes the bridge of this conversation for the end of Horizon. The entries
    /// stay as they are: nothing draws them again.
    pub(super) fn shutdown(&mut self) -> Option<Bridge> {
        self.generation = self.generation.saturating_add(1);
        self.waiting.clear();
        self.bridge.take()
    }

    /// Stops this conversation: the turn in flight ends, the notice says why,
    /// and the next message resumes the session. It answers the bridge that the
    /// caller stops on the worker.
    pub(super) fn stop(&mut self) -> Option<Bridge> {
        self.generation = self.generation.saturating_add(1);
        self.waiting.clear();
        self.conversation.apply(BridgeEvent::TurnEnded {
            failed: false,
            duration_ms: None,
            cost_usd: None,
        });
        self.conversation.notice(STOPPED_NOTICE);
        self.phase = Phase::Stopped;
        self.bridge.take()
    }
}

/// The program of one conversation: Claude Code, or the program that the
/// environment names in its place, with the same arguments.
pub(super) fn program_spec(program: &std::path::Path, cwd: &std::path::Path, session: SessionChoice) -> ProgramSpec {
    let mut spec = horizon_core::agent_bridge::claude(cwd, session);
    spec.program = program.to_path_buf();
    spec
}

/// The events of a program that the dock runs, as the line format of Claude
/// Code reads them.
pub(super) fn codec() -> Box<dyn horizon_core::agent_bridge::LineCodec> {
    Box::new(ClaudeCodec::new())
}

/// The words of the operating system under the error of a start.
fn failure_detail(error: &horizon_core::agent_bridge::BridgeError) -> String {
    match std::error::Error::source(error) {
        Some(source) => source.to_string(),
        None => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Phase, Session, Start, program_spec};
    use horizon_core::agent_bridge::{Bridge, Entry, SessionChoice};

    /// The bridge of a program that is not there, and the error of its start.
    fn failed_start(session: &Start) -> horizon_core::agent_bridge::BridgeError {
        Bridge::start(
            program_spec(
                std::path::Path::new("/nonexistent/horizon-stand-in"),
                &std::env::temp_dir(),
                session.session.clone(),
            ),
            super::codec(),
            std::sync::Arc::new(|| {}),
        )
        .expect_err("a program that is not there does not start")
    }

    #[test]
    fn the_header_names_the_state_of_the_conversation() {
        assert_eq!(
            Phase::NotConnected.line(false),
            "Orchestrator \u{b7} Claude Code \u{b7} not connected"
        );
        assert_eq!(
            Phase::Starting.line(false),
            "Orchestrator \u{b7} Claude Code \u{b7} starting"
        );
        assert_eq!(
            Phase::Running.line(false),
            "Orchestrator \u{b7} Claude Code \u{b7} ready"
        );
        assert_eq!(
            Phase::Running.line(true),
            "Orchestrator \u{b7} Claude Code \u{b7} working"
        );
        assert_eq!(
            Phase::Stopped.line(false),
            "Orchestrator \u{b7} Claude Code \u{b7} stopped"
        );
        assert_eq!(Phase::Ended.line(false), "Orchestrator \u{b7} Claude Code \u{b7} ended");
    }

    #[test]
    fn a_start_of_a_new_conversation_names_a_new_session_and_the_next_resumes_it() {
        let mut session = Session::new();
        let first: Start = session.begin_start();
        let SessionChoice::New(id) = first.session else {
            panic!("the first start names a new session: {:?}", first.session);
        };
        assert_eq!(first.generation, 1, "the first start of the conversation");
        // The program named its session, which the next start resumes.
        session.resume_id = Some(id.clone());
        let second = session.begin_start();
        assert_eq!(second.session, SessionChoice::Resume(id));
        assert_eq!(second.generation, 2, "the start after it");
    }

    #[test]
    fn a_program_that_cannot_start_names_the_error_of_the_system_and_asks_again_later() {
        let mut session = Session::new();
        session.say("alpha");
        let start = session.begin_start();
        let error = failed_start(&start);
        assert!(session.adopt(start.generation, Some(Err(error))).is_none());
        let snapshot = session.snapshot();
        let Some(Entry::Notice { text }) = snapshot.entries.last() else {
            panic!("the frame draws one notice: {:?}", snapshot.entries);
        };
        assert!(text.starts_with("Claude Code did not start: "), "{text}");
        assert!(text.len() > "Claude Code did not start: ".len(), "{text}");
        assert!(!snapshot.busy, "the turn of that message ended with the failure");
        assert_eq!(session.state_line(), "Orchestrator \u{b7} Claude Code \u{b7} ended");
        // The next message asks for one more start, and no message waits twice.
        assert!(session.say("bravo"));
        let again = session.begin_start();
        assert_eq!(again.generation, start.generation + 1);
    }

    #[test]
    fn a_stopped_conversation_keeps_the_session_that_the_program_named() {
        let mut session = Session::new();
        session.say("alpha");
        let _start = session.begin_start();
        session.resume_id = Some("00000000-0000-4000-8000-000000000001".to_owned());
        assert!(session.stop().is_none(), "no bridge of this conversation is up yet");
        let snapshot = session.snapshot();
        assert_eq!(
            snapshot.entries.last(),
            Some(&Entry::Notice {
                text: super::STOPPED_NOTICE.to_owned()
            })
        );
        assert_eq!(session.state_line(), "Orchestrator \u{b7} Claude Code \u{b7} stopped");
        let resumed = session.begin_start();
        assert_eq!(
            resumed.session,
            SessionChoice::Resume("00000000-0000-4000-8000-000000000001".to_owned())
        );
    }
}
