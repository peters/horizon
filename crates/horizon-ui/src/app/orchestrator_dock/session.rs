//! One conversation of the dock: the entries of one workspace, the bridge that
//! speaks for it, and the messages that wait for that bridge.
//!
//! A [`Session`] is the memory of one workspace, keyed by the `local_id` that a
//! restart keeps. It holds the [`Conversation`] of `horizon-core`, which is the
//! list of entries that the dock draws, and it holds the [`Bridge`] of that
//! conversation while the program runs.
//!
//! The start of a bridge starts a process, so it never runs on the frame: the
//! frame queues it on the worker of the dock, a thread of its own named
//! `orchestrator-dock` ([`super::worker`]), and keeps its `generation`. A result
//! of an earlier generation is stale, because the person stopped the conversation
//! or closed its workspace while the start ran, and the result is then stopped
//! instead of kept. [`Bridge::stop`] can wait, so it runs on that worker too.
//!
//! Every text of the program is untrusted. It enters the conversation only as
//! one entry of `horizon_core::agent_bridge`, which cuts it, and the dock draws
//! it as plain text. The notices of this module are the words of Horizon.
//!
//! A conversation outlives the run: the file of its entries and the record of
//! its session live in the session folder, and [`Session::restored`] brings both
//! back on the first frame of a start. [`Storage`] holds the bookkeeping of
//! that file, so a frame writes one batch when its events settled an entry.

use std::sync::Arc;

use horizon_core::OrchestratorConversationState;
use horizon_core::agent_bridge::{
    Bridge, BridgeEvent, BridgeIdentity, ClaudeCodec, Conversation, ConversationSnapshot, Entry, ProgramSpec,
    ProposalItem, ProposalState, SessionChoice,
};

use super::storage::Storage;

/// The notice of a turn that the person stopped.
pub(super) const STOPPED_NOTICE: &str = "Stopped by you";
/// The first words of the notice of a program that did not start.
pub(super) const START_FAILURE: &str = "Claude Code did not start";
/// The notice of an account that reached the limit of its plan and uses overage.
pub(super) const LIMIT_NOTICE: &str = "Claude Code reports that the plan limit is reached. Extra usage is in use.";
/// The notice of one message that the field holds and the program cannot take.
pub(super) const TOO_LONG_NOTICE: &str = "The message is too long to send. Shorten it.";
/// The notice of a message that the program of a conversation took no.
pub(super) const UNSENT_NOTICE: &str = "The message was not sent to the program. Send it again.";
/// The notice of a stored session that the program did not continue.
pub(super) const RESUME_FAILURE_NOTICE: &str =
    "The earlier session could not continue. A new one starts with your next message.";
/// The notice of the turn that Horizon started for the press of the person, and
/// that the program of this conversation did not write itself.
pub(in crate::app) const WAKE_NOTICE: &str = "A press on a card woke the orchestrator";

/// The state of one conversation, as the second line of the header names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    /// No bridge ran for this workspace yet.
    NotConnected,
    /// The start of a bridge waits on the worker of the dock.
    Starting,
    /// A bridge runs for this workspace.
    Running,
    /// The person stopped the bridge.
    Stopped,
    /// The program ended, or it did not start.
    Ended,
}

impl Phase {
    /// The words of the chip at the right edge of the header. The line under the
    /// name keeps the program only.
    pub(super) fn word(self, busy: bool) -> &'static str {
        match self {
            Self::NotConnected => "not connected",
            Self::Starting => "starting",
            Self::Running if busy => "working",
            Self::Running => "ready",
            Self::Stopped => "stopped",
            Self::Ended => "ended",
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

/// What the frame does after the person sent one message.
#[derive(Debug)]
pub(super) enum Said {
    /// The bridge of this conversation took the message.
    Taken,
    /// No bridge runs for this workspace, so the message waits for a start.
    Start,
    /// The bridge that ran took no message: it goes to the worker for its stop,
    /// and a new bridge carries what waits.
    StartAfter(Bridge),
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
    /// Whether the bridge of this conversation resumed a stored session and no
    /// turn of it ended well yet.
    resume_watch: bool,
    /// The lines of Horizon that wait for the end of the turn that runs, such as
    /// the line that names the proposal that the person started.
    system_waiting: Vec<String>,
    /// The entries of the front of the conversation that the file brought back on
    /// this run, so a row of an earlier run reads as a row of an earlier run.
    loaded_entries: usize,
    /// The stored file of this conversation, and the jobs that change it.
    storage: Storage,
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
            resume_watch: false,
            system_waiting: Vec::new(),
            loaded_entries: 0,
            storage: Storage::default(),
        }
    }

    /// The conversation of a workspace that keeps one, as the record of its
    /// last run names it: the session that the next message resumes, and the
    /// entries that the worker loads from its file. A session that another
    /// program named is not resumed, because that program does not know it.
    pub(super) fn restored(session_id: Option<String>, program: &str, program_name: &str) -> Self {
        let mut session = Self::new();
        if program == program_name {
            session.resume_id = session_id;
        }
        session.storage = Storage::restored();
        session
    }

    /// The record that the runtime state of this workspace keeps, or `None`
    /// when this conversation has nothing to resume and no file.
    pub(super) fn record(&self, program: &str, allowed_workers: &[String]) -> Option<OrchestratorConversationState> {
        (self.storage.keeps() || self.resume_id.is_some() || !allowed_workers.is_empty()).then(|| {
            OrchestratorConversationState {
                session_id: self.resume_id.clone(),
                program: program.to_owned(),
                allowed_workers: allowed_workers.to_vec(),
            }
        })
    }

    /// The entries that the file of this conversation does not hold yet, with
    /// the number of entries that it holds after the write.
    pub(super) fn pending_append(&self, flush: bool) -> Option<(Vec<Entry>, usize)> {
        self.storage.pending(&self.conversation, flush)
    }

    /// Records the append of the entries that reach `count`, which the worker
    /// writes now.
    pub(super) fn append_queued(&mut self, count: usize) {
        self.storage.queued(count);
    }

    /// Applies the answer of the worker to the append that waited.
    pub(super) fn append_finished(&mut self, written: bool) {
        self.storage.finished(written);
    }

    /// Applies the entries that the worker read from the file: they stand above
    /// the entries that this run already holds.
    pub(super) fn loaded(&mut self, entries: Vec<Entry>) {
        self.loaded_entries = self.loaded_entries.saturating_add(entries.len());
        self.storage.loaded(&mut self.conversation, entries);
    }

    /// The entries of the front of the conversation that the file brought back on
    /// this run. An entry at an index below this count is an entry of an earlier
    /// run, so a proposal that it holds has no program behind it now.
    pub(super) fn loaded_entries(&self) -> usize {
        self.loaded_entries
    }

    /// Whether a load of the stored conversation of this workspace waits on the
    /// worker, which a test reads.
    #[cfg(test)]
    pub(super) fn is_loading(&self) -> bool {
        self.storage.is_loading()
    }

    /// The conversation of this moment, which one frame draws.
    pub(super) fn snapshot(&self) -> Arc<ConversationSnapshot> {
        self.conversation.snapshot()
    }

    /// Whether this conversation owns a bridge now: its program runs.
    pub(super) fn has_bridge(&self) -> bool {
        self.bridge.is_some()
    }

    /// Whether the start of a bridge waits on the worker of the dock.
    pub(super) fn is_starting(&self) -> bool {
        self.phase == Phase::Starting
    }

    /// The turn of this conversation, as the sidebar reads it: whether a turn runs
    /// and how many turns failed.
    pub(super) fn turn_snapshot(&self) -> horizon_core::workspace_list::Turn {
        let snapshot = self.snapshot();
        horizon_core::workspace_list::Turn {
            busy: snapshot.busy,
            failed: snapshot
                .entries
                .iter()
                .filter(|entry| matches!(entry, Entry::Notice { text } if text == super::TURN_FAILED_LINE))
                .count(),
        }
    }

    /// Ends the turn of this conversation, for a test that reads the state of a
    /// conversation without a program behind it.
    #[cfg(test)]
    pub(super) fn end_turn_for_test(&mut self) {
        self.conversation.apply(BridgeEvent::TurnEnded {
            failed: false,
            duration_ms: None,
            cost_usd: None,
        });
    }

    /// The state of this conversation, which the chip of the header names.
    pub(super) fn phase(&self) -> Phase {
        self.phase
    }

    /// Whether a turn of this conversation runs.
    pub(super) fn busy(&self) -> bool {
        self.snapshot().busy
    }

    /// The cost of the ended turns of this conversation, which the header shows
    /// when it is above zero.
    pub(super) fn cost_usd_total(&self) -> f64 {
        self.snapshot().cost_usd_total
    }

    /// Adds one line of Horizon to this conversation, such as the notice of a
    /// message that the program cannot take. The turn of the conversation does not
    /// change.
    pub(super) fn notice(&mut self, text: &str) {
        self.conversation.notice(text);
    }

    /// Adds one message of the person, and sends it to the bridge of this
    /// conversation when one runs. The answer names what the frame does next.
    /// `via_front_desk` marks a message that the front desk passed: the dock draws
    /// the caption of the front desk above that bubble.
    pub(super) fn say_via(&mut self, text: &str, via_front_desk: bool) -> Said {
        self.conversation.person_said_via(text, via_front_desk);
        let Some(bridge) = self.bridge.take() else {
            self.waiting.push(text.to_owned());
            return Said::Start;
        };
        if bridge.send(text.to_owned()).is_ok() {
            self.bridge = Some(bridge);
            Said::Taken
        } else {
            // The program of this conversation took no message. It ends on the
            // worker, because a bridge that runs is never dropped, and the words
            // of Horizon name the message that did not reach it.
            self.conversation.notice(UNSENT_NOTICE);
            Said::StartAfter(bridge)
        }
    }

    /// Sends the one wake line of Horizon to the program of this conversation as
    /// a turn of its own, so the program learns that a press waits, and names
    /// that turn in the conversation. The turn is no message of the person, so
    /// it adds no [`Entry::Person`]. It answers whether the line went out.
    pub(super) fn wake(&mut self, line: &str) -> bool {
        let Some(bridge) = self.bridge.take() else {
            return false;
        };
        if bridge.send(line.to_owned()).is_err() {
            // The program took no line, so its end arrives with the events of this
            // bridge: no notice claims a wake that did not go out.
            self.bridge = Some(bridge);
            return false;
        }
        self.bridge = Some(bridge);
        self.conversation.system_said(WAKE_NOTICE);
        true
    }

    /// Adds or moves the reference row of the card `panel` of this workspace,
    /// which the dock draws under the text of the program.
    pub(super) fn card_row(&mut self, panel: &str, title: &str) {
        self.conversation.card_drawn(panel, title);
    }

    /// Adds the proposal `id` to this conversation, and replaces the proposal that
    /// was still open.
    pub(super) fn proposal(&mut self, id: &str, items: &[ProposalItem]) {
        self.conversation.proposal(id, items);
    }

    /// Moves the proposal `id` to `state`, and answers whether this conversation
    /// held it.
    pub(super) fn proposal_state(&mut self, id: &str, state: ProposalState) -> bool {
        if !self
            .snapshot()
            .entries
            .iter()
            .any(|entry| matches!(entry, Entry::Proposal { id: shown, .. } if shown == id))
        {
            return false;
        }
        self.conversation.proposal_state(id, state);
        true
    }

    /// The items of the proposal `id`, while it is open.
    pub(super) fn open_proposal(&self, id: &str) -> Option<Vec<ProposalItem>> {
        self.snapshot().entries.iter().find_map(|entry| match entry {
            Entry::Proposal {
                id: shown,
                items,
                state: ProposalState::Open,
            } if shown == id => Some(items.clone()),
            _ => None,
        })
    }

    /// Adds the reference row of the work item `work_item` to this conversation.
    pub(super) fn work_item_row(&mut self, work_item: &str, name: &str) {
        self.conversation.work_item_row(work_item, name);
    }

    /// Adds the reference row of the worker `panel` to this conversation.
    pub(super) fn worker_started(&mut self, panel: &str, program: &str, task: &str) {
        self.conversation.worker_started(panel, program, task);
    }

    /// Adds one message that the orchestrator typed into the worker `panel`.
    pub(super) fn worker_message(&mut self, panel: &str, program: &str, text: &str) {
        self.conversation.worker_message(panel, program, text);
    }

    /// Adds one report of the worker `panel`, as it went to the program.
    pub(super) fn worker_report(&mut self, panel: &str, program: &str, text: &str) {
        self.conversation.worker_report(panel, program, text);
    }

    /// Adds one request of the orchestrator for a worker that the chip does not
    /// hold.
    pub(super) fn worker_ask(&mut self, id: &str, program: &str, task: &str) {
        self.conversation.worker_ask(id, program, task);
    }

    /// Moves the request `id` of a worker to `state`, and answers whether this
    /// conversation held it.
    pub(super) fn set_worker_ask_state(&mut self, id: &str, state: horizon_core::agent_bridge::WorkerAskState) -> bool {
        self.conversation.worker_ask_state(id, state)
    }

    /// Whether the person answered `Not now` to a request for a worker of
    /// `program` in this conversation.
    pub(super) fn declined_worker_ask(&self, program: &str) -> bool {
        self.conversation.declined_worker_ask(program)
    }

    /// The program and the first line of the task of the request `id`, while it
    /// waits for the person.
    pub(super) fn open_worker_ask(&self, id: &str) -> Option<(String, String)> {
        self.conversation.open_worker_ask(id)
    }

    /// The last text of the program of this conversation, cut to `max` characters
    /// and without a control character, or `None` when it said nothing yet.
    pub(super) fn last_text(&self, max: usize) -> Option<String> {
        let snapshot = self.snapshot();
        let text = snapshot.entries.iter().rev().find_map(|entry| match entry {
            Entry::Agent { text, .. } => Some(text.as_str()),
            _ => None,
        })?;
        let cut: String = text
            .chars()
            .map(|character| if character.is_control() { ' ' } else { character })
            .take(max)
            .collect();
        (!cut.trim().is_empty()).then_some(cut)
    }

    /// Whether this conversation can take a line of Horizon now: its program runs,
    /// no turn of it runs, and no earlier line of Horizon waits.
    pub(super) fn takes_a_system_line(&self) -> bool {
        !self.busy() && self.has_bridge() && self.system_waiting.is_empty()
    }

    /// Sends one line of Horizon into the bridge of this conversation now, and
    /// answers whether the program took it. The caller has asked
    /// [`Self::takes_a_system_line`]. The line is no message of the person, so it
    /// adds no [`Entry::Person`].
    pub(super) fn send_system_line(&mut self, line: &str) -> bool {
        self.send_system_now(line)
    }

    /// Sends one line of Horizon to the program of this conversation as a turn of
    /// its own, and answers whether it went out now. A conversation that runs a turn
    /// keeps the line until that turn ends, so the words of Horizon never cut into
    /// the answer of the program; a conversation without a bridge holds it too.
    pub(super) fn system_message(&mut self, line: &str) -> bool {
        if self.busy() || !self.has_bridge() {
            self.system_waiting.push(line.to_owned());
            return false;
        }
        self.send_system_now(line)
    }

    /// Sends the line of Horizon that waits, when no turn of this conversation
    /// runs. It answers whether a line went out.
    pub(super) fn flush_system(&mut self) -> bool {
        if self.system_waiting.is_empty() || self.busy() || !self.has_bridge() {
            return false;
        }
        let line = self.system_waiting.remove(0);
        if self.send_system_now(&line) {
            return true;
        }
        self.system_waiting.insert(0, line);
        false
    }

    /// Sends one line of Horizon into the bridge that runs, and marks the
    /// conversation busy for it. The line is no message of the person, so it adds
    /// no [`Entry::Person`] and no notice: the words of Horizon that name such a
    /// turn are the words of its caller.
    fn send_system_now(&mut self, line: &str) -> bool {
        let Some(bridge) = self.bridge.take() else {
            return false;
        };
        if bridge.send(line.to_owned()).is_err() {
            // The program took no line, so its end arrives with the events of this
            // bridge: no state of this conversation claims a turn that did not start.
            self.bridge = Some(bridge);
            return false;
        }
        self.bridge = Some(bridge);
        self.conversation.turn_started();
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
        self.resume_watch = matches!(session, SessionChoice::Resume(_));
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
                // A live bridge is never replaced by a second start of one
                // workspace: the bridge that runs keeps the conversation, and the
                // bridge of this start goes back to the caller for its stop.
                if self.bridge.is_some() {
                    return Some(bridge);
                }
                self.bridge = Some(bridge);
                self.phase = Phase::Running;
                self.send_waiting();
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

    /// Sends the messages that waited to the bridge of this conversation.
    fn send_waiting(&mut self) {
        for text in std::mem::take(&mut self.waiting) {
            let Some(bridge) = self.bridge.as_ref() else {
                return;
            };
            if bridge.send(text).is_err() {
                // The program ended before this message reached it. Its events
                // still hold the end of the program, and a poll of a later frame
                // reads them and names why it ended, so the bridge stays until
                // then.
                return;
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
        if self.resume_watch {
            // The stored session of this start did not continue either, so no
            // later start resumes it: the next message starts a new session.
            self.forget_resumed_session();
        }
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
        let mut resume_failed = false;
        let mut started = false;
        for event in events {
            match &event {
                BridgeEvent::SessionStarted { .. } => started = true,
                BridgeEvent::Limit {
                    using_overage: true, ..
                } if !self.limit_noticed => {
                    self.limit_noticed = true;
                    self.conversation.notice(LIMIT_NOTICE);
                }
                BridgeEvent::TurnEnded { failed, .. } => {
                    if *failed {
                        resume_failed |= self.resume_watch;
                    } else {
                        self.resume_watch = false;
                    }
                }
                BridgeEvent::Ended { .. } => {
                    ended = true;
                    resume_failed |= self.resume_watch;
                }
                _ => {}
            }
            self.conversation.apply(event);
        }
        if started {
            // The session id is text of the program, so the dock keeps the value
            // that the conversation cut: one line of `MAX_SUMMARY_CHARS`
            // characters, which the next start passes as one argument.
            let session_id = self.conversation.snapshot().session_id.clone();
            if session_id.as_deref().is_some_and(|id| !id.is_empty()) {
                self.resume_id = session_id;
            }
        }
        if resume_failed {
            self.forget_resumed_session();
        }
        if ended {
            self.bridge = None;
            self.phase = Phase::Ended;
        }
    }

    /// Forgets the stored session that the program did not continue, and says so
    /// in one notice. The entries of the conversation stay, and the next message
    /// starts a new session.
    fn forget_resumed_session(&mut self) {
        self.resume_watch = false;
        self.resume_id = None;
        self.conversation.notice(RESUME_FAILURE_NOTICE);
    }

    /// Takes the bridge of this conversation for the end of Horizon. The entries
    /// stay as they are: nothing draws them again.
    pub(super) fn shutdown(&mut self) -> Option<Bridge> {
        self.generation = self.generation.saturating_add(1);
        self.waiting.clear();
        self.resume_watch = false;
        self.bridge.take()
    }

    /// Stops this conversation: the turn in flight ends, the notice says why,
    /// and the next message resumes the session. It answers the bridge that the
    /// caller stops on the worker.
    pub(super) fn stop(&mut self) -> Option<Bridge> {
        self.generation = self.generation.saturating_add(1);
        self.waiting.clear();
        self.resume_watch = false;
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
/// environment names in its place, with the same arguments and the identity of
/// the conversation that `local_id` names.
///
/// The identity is the one of the active host: its actor, its host instance, its
/// home and its executable, plus the plugin directory of Claude Code, so the
/// program has the Horizon MCP server and the Horizon skills and draws cards in
/// its own workspace. A local id that the actor form does not accept gives no
/// identity, and the program then draws no card.
///
/// The front desk under [`super::FRONT_DESK`] has an identity of its own and the
/// program of a front desk ([`horizon_core::agent_bridge::claude_for_front_desk`]).
/// That program has no built-in tool, so the conversation writes the words of the
/// person to the work items and does no work itself.
///
/// The frame never calls this function: the identity reads the file system, so
/// the job of [`super::worker`] builds the specification of a start.
pub(super) fn program_spec(
    program: &std::path::Path,
    cwd: &std::path::Path,
    session: SessionChoice,
    local_id: &str,
) -> ProgramSpec {
    let mut spec = if local_id == super::FRONT_DESK {
        let identity = BridgeIdentity::front_desk();
        horizon_core::agent_bridge::claude_for_front_desk(cwd, session, &identity)
    } else if let Some(identity) = BridgeIdentity::for_workspace(local_id) {
        horizon_core::agent_bridge::claude_for_workspace(cwd, session, &identity)
    } else {
        tracing::warn!(local_id, "the workspace of this conversation has no orchestrator actor");
        horizon_core::agent_bridge::claude(cwd, session)
    };
    spec.program = program.to_path_buf();
    spec
}

/// Makes the folder that the program of `local_id` runs in, which the front desk
/// keeps under the Horizon home. The worker of the dock calls this before a start,
/// so no frame reads a directory or makes one. A folder that cannot be made leaves
/// the program in the directory that the job names, and its start names why.
pub(super) fn make_directory(directory: &std::path::Path, local_id: &str) {
    if let Err(error) = std::fs::create_dir_all(directory) {
        tracing::warn!(%error, %local_id, "the folder of this conversation is not there");
    }
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
    use super::{Phase, Said, Session, Start, program_spec};
    use horizon_core::agent_bridge::{Bridge, Entry, SessionChoice};

    /// A workspace local id of the form that `new_local_id` makes.
    const WORKSPACE: &str = "8f14e45f-ceea-4a67-9f0b-3a1f2b3c4d5e";

    /// The stand-in of a program that names its session and then waits for the
    /// lines of the person, so a test owns a bridge without an agent program.
    const SILENT: &str = r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000001"}'
while IFS= read -r line; do :; done
"#;

    /// The bridge of a program that is not there, and the error of its start.
    fn failed_start(session: &Start) -> horizon_core::agent_bridge::BridgeError {
        Bridge::start(
            program_spec(
                std::path::Path::new("/nonexistent/horizon-stand-in"),
                &std::env::temp_dir(),
                session.session.clone(),
                WORKSPACE,
            ),
            super::codec(),
            std::sync::Arc::new(|| {}),
        )
        .expect_err("a program that is not there does not start")
    }

    /// The presence of a card of the orchestrator of a workspace, which is the
    /// bridge that the dock runs for it.
    #[test]
    fn the_presence_of_a_conversation_follows_its_bridge() {
        let folder = tempfile::tempdir().expect("a folder for the stand-in");
        let program = horizon_core::stand_in_program_in(folder.path(), "stand-in.sh", SILENT)
            .expect("the stand-in program of the test");
        let mut session = Session::new();
        assert!(
            !session.has_bridge(),
            "a conversation that started no program owns no bridge"
        );
        let start = session.begin_start();
        let bridge = Bridge::start(
            program_spec(&program, folder.path(), start.session.clone(), WORKSPACE),
            super::codec(),
            std::sync::Arc::new(|| {}),
        )
        .expect("the stand-in program starts");
        assert!(
            session.adopt(start.generation, Some(Ok(bridge))).is_none(),
            "the bridge of this generation is the bridge of this conversation"
        );
        assert!(session.has_bridge(), "the program of this workspace runs");
        assert!(
            session.stop().is_some(),
            "the stop takes the bridge of the conversation"
        );
        assert!(
            !session.has_bridge(),
            "a stopped conversation holds no program, so its orchestrator is gone"
        );
    }

    #[test]
    fn the_chip_of_the_header_names_the_state_of_the_conversation() {
        assert_eq!(Phase::NotConnected.word(false), "not connected");
        assert_eq!(Phase::Starting.word(false), "starting");
        assert_eq!(Phase::Running.word(false), "ready");
        assert_eq!(Phase::Running.word(true), "working");
        assert_eq!(Phase::Stopped.word(false), "stopped");
        assert_eq!(Phase::Ended.word(false), "ended");
        // The line under the name keeps the program and drops the state word.
        assert_eq!(
            super::super::header::PROGRAM_LINE,
            "Orchestrator \u{b7} Claude Code on this computer"
        );
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
        session.say_via("alpha", false);
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
        assert_eq!(session.phase(), Phase::Ended);
        // The next message asks for one more start, and no message waits twice.
        assert!(matches!(session.say_via("bravo", false), Said::Start));
        let again = session.begin_start();
        assert_eq!(again.generation, start.generation + 1);
    }

    #[test]
    fn a_start_that_could_not_continue_the_session_starts_a_new_one_next_time() {
        // A conversation that a restart brought back, with the session of its
        // earlier run.
        let mut session = Session::new();
        session.resume_id = Some("00000000-0000-4000-8000-000000000001".to_owned());
        session.say_via("alpha", false);
        let start = session.begin_start();
        assert_eq!(
            start.session,
            SessionChoice::Resume("00000000-0000-4000-8000-000000000001".to_owned()),
            "the first start of the restored conversation resumes its session"
        );
        let error = failed_start(&start);
        assert!(session.adopt(start.generation, Some(Err(error))).is_none());

        // One notice names why the session did not continue, and the next message
        // starts a new session.
        let snapshot = session.snapshot();
        assert_eq!(
            snapshot.entries.last(),
            Some(&Entry::Notice {
                text: super::RESUME_FAILURE_NOTICE.to_owned()
            }),
            "the notice of the stored session follows the notice of the start"
        );
        let next = session.begin_start();
        assert!(
            matches!(next.session, SessionChoice::New(_)),
            "the next start names a new session: {:?}",
            next.session
        );
    }

    #[test]
    fn a_message_that_the_program_did_not_take_names_it_and_stops_that_bridge() {
        let folder = tempfile::tempdir().expect("a folder for the stand-in");
        let program = horizon_core::stand_in_program_in(folder.path(), "stand-in.sh", SILENT)
            .expect("the stand-in program of the test");
        let mut session = Session::new();
        session.say_via("alpha", false);
        let start = session.begin_start();
        let bridge = Bridge::start(
            program_spec(&program, folder.path(), start.session.clone(), WORKSPACE),
            super::codec(),
            std::sync::Arc::new(|| {}),
        )
        .expect("the stand-in program starts");
        assert!(session.adopt(start.generation, Some(Ok(bridge))).is_none());

        // A text above the bound of one message takes no bridge.
        let Said::StartAfter(bridge) =
            session.say_via(&"x".repeat(horizon_core::agent_bridge::MAX_TEXT_BYTES + 1), false)
        else {
            panic!("a message that the program does not take hands its bridge back");
        };
        assert!(!session.has_bridge(), "that bridge leaves the conversation");
        assert_eq!(
            session.snapshot().entries.last(),
            Some(&Entry::Notice {
                text: super::UNSENT_NOTICE.to_owned()
            })
        );
        // The bridge that took no message is stopped by its caller, never dropped.
        bridge.stop();
        assert!(
            matches!(session.say_via("bravo", false), Said::Start),
            "the next message waits for a new bridge"
        );
    }

    #[test]
    fn a_second_start_does_not_replace_a_live_bridge() {
        let folder = tempfile::tempdir().expect("a folder for the stand-in");
        let program = horizon_core::stand_in_program_in(folder.path(), "stand-in.sh", SILENT)
            .expect("the stand-in program of the test");
        let mut session = Session::new();
        session.say_via("alpha", false);
        let first = session.begin_start();
        let bridge = Bridge::start(
            program_spec(&program, folder.path(), first.session.clone(), WORKSPACE),
            super::codec(),
            std::sync::Arc::new(|| {}),
        )
        .expect("the stand-in program starts");
        assert!(session.adopt(first.generation, Some(Ok(bridge))).is_none());
        assert!(session.has_bridge(), "the program of this workspace runs");

        // A second start answers while a bridge runs: it serves nobody, and the
        // bridge that runs keeps the conversation.
        session.say_via("bravo", false);
        let second = session.begin_start();
        let extra = Bridge::start(
            program_spec(&program, folder.path(), second.session.clone(), WORKSPACE),
            super::codec(),
            std::sync::Arc::new(|| {}),
        )
        .expect("the stand-in program starts");
        let extra = session
            .adopt(second.generation, Some(Ok(extra)))
            .expect("the second bridge of a live conversation goes back for its stop");
        assert!(
            session.has_bridge(),
            "the first program still speaks for this workspace"
        );

        extra.stop();
        let live = session.stop().expect("the bridge that runs");
        live.stop();
        assert!(!session.has_bridge());
    }

    #[test]
    fn a_stopped_conversation_keeps_the_session_that_the_program_named() {
        let mut session = Session::new();
        session.say_via("alpha", false);
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
        assert_eq!(session.phase(), Phase::Stopped);
        let resumed = session.begin_start();
        assert_eq!(
            resumed.session,
            SessionChoice::Resume("00000000-0000-4000-8000-000000000001".to_owned())
        );
    }
}
