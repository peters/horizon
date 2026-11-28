//! The workers of one conversation of the dock: the panels that the orchestrator
//! started, the last report of each of them, the two rate bounds, and the programs
//! that the chip `Workers` of the person holds.
//!
//! A worker is an agent panel of the workspace of this conversation, started from
//! a preset of the person's settings. The record of a worker is the state of the
//! run: it holds the local id of the panel of that worker, the name of its preset,
//! the kind of its program, its last report and the instant of its last message
//! and of its last report. A panel that leaves the board leaves the record on the
//! pass of the dock that reads the board, so the set of workers stays as small as
//! the board.
//!
//! The programs that the chip holds are the choice of the person, so they outlive
//! the run: they are stored with the record of the conversation of the workspace,
//! and an older record loads with no program.
//!
//! The reports of a worker go to the program of the orchestrator as one turn each.
//! One program serves one turn at a time, so a report that arrives while a turn
//! runs waits in [`WorkersOf::reports`], in order, and at most
//! [`MAX_WAITING_REPORTS`] of them wait. The pass of the dock sends the first one
//! that the conversation can take.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use horizon_browser_control::manifest::block_workers::{
    MAX_WAITING_REPORTS, MAX_WORKER_REPORT_CHARS, MAX_WORKERS, WORKER_INTERVAL_MILLIS,
};

use super::OrchestratorDock;

/// One worker that the orchestrator of a conversation started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct WorkerRecord {
    /// The local id of the panel of the worker.
    pub(in crate::app) panel: String,
    /// The name of the preset that the worker started from.
    pub(in crate::app) program: String,
    /// The kind of that preset, as the settings store it, such as `claude`.
    pub(in crate::app) kind: String,
    /// The last text that the worker reported, cut for the read of `workers`.
    last_report: Option<String>,
    /// The instant of the last message that the orchestrator sent to this worker.
    message_at: Option<Instant>,
    /// The instant of the last report of this worker.
    report_at: Option<Instant>,
}

impl WorkerRecord {
    /// The record of one worker, which has sent no report yet.
    pub(in crate::app) fn new(panel: &str, program: &str, kind: &str) -> Self {
        Self {
            panel: panel.to_owned(),
            program: program.to_owned(),
            kind: kind.to_owned(),
            last_report: None,
            message_at: None,
            report_at: None,
        }
    }

    /// The last text that this worker reported.
    pub(in crate::app) fn last_report(&self) -> Option<&str> {
        self.last_report.as_deref()
    }
}

/// The workers of one conversation, and the programs that its chip holds.
#[derive(Default)]
pub(in crate::app) struct WorkersOf {
    workers: Vec<WorkerRecord>,
    /// The names of the presets that the person lets this orchestrator start.
    allowed: Vec<String>,
    /// The reports that wait for the end of a turn of the orchestrator, in order.
    reports: VecDeque<String>,
}

impl WorkersOf {
    /// The workers of this conversation, in the order of their start.
    pub(in crate::app) fn workers(&self) -> &[WorkerRecord] {
        &self.workers
    }

    /// The programs that the chip of this conversation holds.
    pub(in crate::app) fn allowed(&self) -> &[String] {
        &self.allowed
    }

    /// Whether the person lets this orchestrator start `program`.
    pub(in crate::app) fn allows(&self, program: &str) -> bool {
        self.allowed.iter().any(|held| held == program)
    }

    /// Adds `program` to the chip of this conversation, or removes it, and answers
    /// whether the chip holds it now.
    pub(in crate::app) fn toggle(&mut self, program: &str) -> bool {
        if let Some(index) = self.allowed.iter().position(|held| held == program) {
            self.allowed.remove(index);
            return false;
        }
        self.allowed.push(program.to_owned());
        true
    }

    /// Adds `program` to the chip of this conversation, once.
    pub(in crate::app) fn allow(&mut self, program: &str) {
        if !self.allows(program) {
            self.allowed.push(program.to_owned());
        }
    }

    /// Replaces the chip of this conversation with `programs`.
    pub(in crate::app) fn set_allowed(&mut self, programs: Vec<String>) {
        self.allowed = programs;
    }

    /// The worker of the panel `panel`.
    pub(in crate::app) fn worker(&self, panel: &str) -> Option<&WorkerRecord> {
        self.workers.iter().find(|worker| worker.panel == panel)
    }

    /// Adds the worker of the panel `panel`, which no record of this conversation
    /// names yet.
    pub(in crate::app) fn add(&mut self, worker: WorkerRecord) {
        self.workers.push(worker);
    }

    /// The reports that wait for the end of a turn of the orchestrator.
    #[cfg(test)]
    pub(in crate::app) fn waiting_reports(&self) -> usize {
        self.reports.len()
    }
}

/// The workers of every conversation of this dock, by the `local_id` that a
/// restart keeps.
#[derive(Default)]
pub(in crate::app) struct WorkerBook {
    conversations: HashMap<String, WorkersOf>,
    /// The agent presets whose program this computer did not find, once the search
    /// of this run answered, and whether the search of this run is on the worker.
    missing_programs: Option<Vec<usize>>,
    asked_programs: bool,
}

impl WorkerBook {
    /// Whether the search of the programs of the agent presets of this run is due,
    /// and the request of it is taken: true the first time and false after it.
    pub(in crate::app) fn ask_programs(&mut self) -> bool {
        if self.asked_programs {
            return false;
        }
        self.asked_programs = true;
        true
    }

    /// Keeps the answer of the search: the indices of the agent presets whose
    /// program this computer does not have. A search that did not answer leaves
    /// every program missing, because Horizon then knows of none.
    pub(in crate::app) fn adopt_programs(&mut self, missing: Option<Vec<usize>>) {
        self.missing_programs = Some(missing.unwrap_or_default());
    }

    /// Whether this computer has the program of the agent preset at `index`. A
    /// search that has not answered yet has found no program.
    pub(in crate::app) fn program_found(&self, index: usize) -> bool {
        match &self.missing_programs {
            Some(missing) => !missing.contains(&index),
            None => false,
        }
    }

    /// The workers of the conversation of `local_id`, or nothing when it started
    /// none and holds no program.
    fn of(&self, local_id: &str) -> Option<&WorkersOf> {
        self.conversations.get(local_id)
    }

    /// The same, for the paths that change the record. A conversation that holds
    /// nothing yet gets one.
    fn of_mut(&mut self, local_id: &str) -> &mut WorkersOf {
        self.conversations.entry(local_id.to_owned()).or_default()
    }

    /// The programs that the chip of `local_id` holds.
    pub(in crate::app) fn allowed(&self, local_id: &str) -> &[String] {
        self.of(local_id).map_or(&[], WorkersOf::allowed)
    }

    /// The workers of `local_id`, in the order of their start.
    pub(in crate::app) fn workers(&self, local_id: &str) -> &[WorkerRecord] {
        self.of(local_id).map_or(&[], WorkersOf::workers)
    }

    /// The worker of the panel `panel`, with the conversation that started it.
    fn of_panel(&self, panel: &str) -> Option<(&str, &WorkerRecord)> {
        self.conversations
            .iter()
            .find_map(|(local_id, workers)| workers.worker(panel).map(|worker| (local_id.as_str(), worker)))
    }

    /// Forgets every worker whose panel `held` does not find, so the record of a
    /// conversation names only panels of this board.
    pub(in crate::app) fn retain_panels(&mut self, held: impl Fn(&str) -> bool) {
        for workers in self.conversations.values_mut() {
            workers.workers.retain(|worker| held(&worker.panel));
        }
    }

    /// Forgets the record of every conversation that `held` does not find, so a
    /// conversation that the board left takes its chip with it.
    pub(in crate::app) fn retain_conversations(&mut self, held: impl Fn(&str) -> bool) {
        self.conversations.retain(|local_id, _| held(local_id));
    }
}

/// What the dock answers about the worker of one panel: the conversation that
/// started it and the name of its preset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct WorkerOwner {
    pub(in crate::app) local_id: String,
    pub(in crate::app) program: String,
}

impl OrchestratorDock {
    /// The programs that the chip `Workers` of the conversation of `local_id`
    /// holds.
    pub(in crate::app) fn workers_allowed(&self, local_id: &str) -> &[String] {
        self.workers.allowed(local_id)
    }

    /// Adds `program` to the chip of `local_id`, or removes it, and answers whether
    /// the chip holds it now.
    pub(in crate::app) fn toggle_worker_program(&mut self, local_id: &str, program: &str) -> bool {
        self.workers.of_mut(local_id).toggle(program)
    }

    /// Adds `program` to the chip of `local_id`, once.
    pub(in crate::app) fn allow_worker_program(&mut self, local_id: &str, program: &str) {
        self.workers.of_mut(local_id).allow(program);
    }

    /// Keeps the chip of `local_id` as the record of the conversation of its
    /// workspace holds it, or answers the programs that the chip holds now.
    pub(in crate::app) fn set_workers_allowed(&mut self, local_id: &str, programs: Vec<String>) {
        self.workers.of_mut(local_id).set_allowed(programs);
    }

    /// The workers of `local_id`, in the order of their start.
    pub(in crate::app) fn workers_of(&self, local_id: &str) -> &[WorkerRecord] {
        self.workers.workers(local_id)
    }

    /// The worker of the panel `panel`, with the conversation that started it.
    pub(in crate::app) fn worker_owner(&self, panel: &str) -> Option<WorkerOwner> {
        self.workers.of_panel(panel).map(|(local_id, worker)| WorkerOwner {
            local_id: local_id.to_owned(),
            program: worker.program.clone(),
        })
    }

    /// Whether `workers` of `local_id` may still start one more worker: at most
    /// four of one orchestrator run at the same time.
    pub(in crate::app) fn worker_slot_free(&self, local_id: &str) -> bool {
        self.workers.workers(local_id).len() < MAX_WORKERS
    }

    /// Keeps the worker of the panel `panel` of `local_id`, which the frame opened
    /// now.
    pub(in crate::app) fn note_worker(&mut self, local_id: &str, worker: WorkerRecord) {
        self.workers.of_mut(local_id).add(worker);
    }

    /// Whether the orchestrator of `local_id` may send a message to the worker of
    /// the panel `panel` at `now`: the first message passes, and so does one that
    /// follows the interval of the last. It answers `None` for a panel that this
    /// conversation did not start.
    pub(in crate::app) fn worker_message_due(&self, local_id: &str, panel: &str, now: Instant) -> Option<bool> {
        self.workers
            .of(local_id)?
            .worker(panel)
            .map(|worker| due(worker.message_at, now))
    }

    /// Records one message that the orchestrator of `local_id` sent to the worker
    /// of the panel `panel` at `now`.
    pub(in crate::app) fn note_worker_message(&mut self, local_id: &str, panel: &str, now: Instant) {
        if let Some(worker) = self
            .workers
            .of_mut(local_id)
            .workers
            .iter_mut()
            .find(|worker| worker.panel == panel)
        {
            worker.message_at = Some(now);
        }
    }

    /// Whether the worker of the panel `panel` may report to the orchestrator of
    /// `local_id` at `now`.
    pub(in crate::app) fn worker_report_due(&self, local_id: &str, panel: &str, now: Instant) -> bool {
        self.workers
            .of(local_id)
            .and_then(|workers| workers.worker(panel))
            .is_some_and(|worker| due(worker.report_at, now))
    }

    /// Queues one report of the worker of the panel `panel` for the end of the turn
    /// of the orchestrator of `local_id`, records it, and answers whether the queue
    /// took it: a full queue of reports takes none.
    pub(in crate::app) fn queue_worker_report(
        &mut self,
        local_id: &str,
        panel: &str,
        line: &str,
        text: &str,
        now: Instant,
    ) -> bool {
        let workers = self.workers.of_mut(local_id);
        if workers.reports.len() >= MAX_WAITING_REPORTS {
            return false;
        }
        workers.reports.push_back(line.to_owned());
        if let Some(worker) = workers.workers.iter_mut().find(|worker| worker.panel == panel) {
            worker.last_report = Some(cut_report(text));
            worker.report_at = Some(now);
        }
        true
    }

    /// Sends the first report that waits, when no turn of the conversation runs and
    /// its queue of the lines of Horizon is empty. It answers whether a line went
    /// out, so a frame that sends one asks for another frame.
    pub(in crate::app) fn flush_worker_reports(&mut self) -> bool {
        let mut sent = false;
        for (local_id, workers) in &mut self.workers.conversations {
            while let Some(line) = workers.reports.front().cloned() {
                let Some(session) = self.sessions.get_mut(local_id.as_str()) else {
                    // The conversation left the dock: its reports serve nobody.
                    workers.reports.clear();
                    break;
                };
                if !session.takes_a_system_line() {
                    break;
                }
                workers.reports.pop_front();
                if !session.send_system_line(&line) {
                    // The program took no line, so the report goes back to the front
                    // of the queue and the end of that program arrives with the
                    // events of its bridge.
                    workers.reports.push_front(line);
                    break;
                }
                sent = true;
            }
        }
        sent
    }

    /// Moves the instant of the last message to the worker `panel` back by `by`, so
    /// a test reads the end of the interval of the rate bound without waiting.
    #[cfg(test)]
    pub(in crate::app) fn rewind_worker_message(&mut self, local_id: &str, panel: &str, by: std::time::Duration) {
        self.rewind(local_id, panel, by, Round::Message);
    }

    /// The same for the instant of the last report of the worker `panel`.
    #[cfg(test)]
    pub(in crate::app) fn rewind_worker_report(&mut self, local_id: &str, panel: &str, by: std::time::Duration) {
        self.rewind(local_id, panel, by, Round::Report);
    }

    /// Moves one instant of one worker back by `by`.
    #[cfg(test)]
    fn rewind(&mut self, local_id: &str, panel: &str, by: std::time::Duration, round: Round) {
        let Some(worker) = self
            .workers
            .of_mut(local_id)
            .workers
            .iter_mut()
            .find(|worker| worker.panel == panel)
        else {
            return;
        };
        let at = match round {
            Round::Message => &mut worker.message_at,
            Round::Report => &mut worker.report_at,
        };
        if let Some(instant) = at {
            *instant -= by;
        }
    }

    /// Adds the reference row of the worker `panel`, with the name of its preset
    /// and the first line of its task, to the conversation of `local_id`.
    pub(in crate::app) fn worker_started(&mut self, local_id: &str, panel: &str, program: &str, task: &str) {
        self.session_mut(local_id).worker_started(panel, program, task);
    }

    /// Adds one message that the orchestrator of `local_id` typed into the worker
    /// `panel`.
    pub(in crate::app) fn worker_message(&mut self, local_id: &str, panel: &str, program: &str, text: &str) {
        self.session_mut(local_id).worker_message(panel, program, text);
    }

    /// Adds one report of the worker `panel` to the conversation of `local_id`.
    pub(in crate::app) fn worker_report(&mut self, local_id: &str, panel: &str, program: &str, text: &str) {
        self.session_mut(local_id).worker_report(panel, program, text);
    }

    /// Adds one request of the orchestrator of `local_id` for a worker that the
    /// chip does not hold.
    pub(in crate::app) fn worker_ask(&mut self, local_id: &str, id: &str, program: &str, task: &str) {
        self.session_mut(local_id).worker_ask(id, program, task);
    }

    /// Moves the request `id` of a worker to `state`, and answers whether the
    /// conversation of `local_id` held it.
    pub(in crate::app) fn set_worker_ask_state(
        &mut self,
        local_id: &str,
        id: &str,
        state: horizon_core::agent_bridge::WorkerAskState,
    ) -> bool {
        self.sessions
            .get_mut(local_id)
            .is_some_and(|session| session.set_worker_ask_state(id, state))
    }

    /// Whether the person answered `Not now` to a request for a worker of
    /// `program` in the conversation of `local_id`.
    pub(in crate::app) fn worker_ask_declined(&self, local_id: &str, program: &str) -> bool {
        self.sessions
            .get(local_id)
            .is_some_and(|session| session.declined_worker_ask(program))
    }

    /// The program and the first line of the task of the request `id` of a worker,
    /// while it waits for the person.
    pub(in crate::app) fn open_worker_ask(&self, local_id: &str, id: &str) -> Option<(String, String)> {
        self.sessions.get(local_id)?.open_worker_ask(id)
    }

    /// Sends one line of Horizon to the program of the conversation of `local_id`
    /// as a turn of its own, and answers whether it went out now. A conversation
    /// that runs a turn keeps the line until that turn ends.
    pub(in crate::app) fn session_system_message(&mut self, local_id: &str, line: &str) -> bool {
        self.sessions
            .get_mut(local_id)
            .is_some_and(|session| session.system_message(line))
    }

    /// Forgets every worker whose panel `held` does not find, on the pass of the
    /// dock that reads the board.
    pub(in crate::app) fn retain_worker_panels(&mut self, held: impl Fn(&str) -> bool) {
        self.workers.retain_panels(&held);
    }

    /// Forgets the workers and the chip of every conversation that `held` does not
    /// find, so the record stays as small as the board.
    pub(in crate::app) fn retain_worker_conversations(&mut self, held: impl Fn(&str) -> bool) {
        self.workers.retain_conversations(&held);
    }
}

/// Which of the two instants of the rate bounds of one worker a test moves back.
#[cfg(test)]
#[derive(Clone, Copy)]
enum Round {
    Message,
    Report,
}

/// Whether a text may follow the one of `last` at `now`: the first passes, and so
/// does one that follows the interval of the bound.
fn due(last: Option<Instant>, now: Instant) -> bool {
    match last {
        Some(at) => now.saturating_duration_since(at).as_millis() >= u128::from(WORKER_INTERVAL_MILLIS),
        None => true,
    }
}

/// One report cut to the bound of the read of `workers`, without a control
/// character but the line break.
fn cut_report(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() && character != '\n' {
                ' '
            } else {
                character
            }
        })
        .take(MAX_WORKER_REPORT_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{WorkerBook, WorkerRecord, cut_report, due};
    use horizon_browser_control::manifest::block_workers::{
        MAX_WAITING_REPORTS, MAX_WORKER_REPORT_CHARS, WORKER_INTERVAL_MILLIS,
    };

    #[test]
    fn the_chip_of_workers_adds_and_removes_one_program() {
        let mut book = WorkerBook::default();
        assert!(book.allowed("ws-1").is_empty(), "a new conversation holds none");
        let workers = book.of_mut("ws-1");
        assert!(workers.toggle("Codex"), "the chip holds Codex now");
        assert!(workers.allows("Codex"));
        assert!(workers.toggle("Pi"));
        assert_eq!(workers.allowed(), ["Codex", "Pi"]);
        assert!(!workers.toggle("Codex"), "a second press removes it");
        assert_eq!(workers.allowed(), ["Pi"]);
        workers.allow("Pi");
        assert_eq!(
            workers.allowed(),
            ["Pi"],
            "an allow of a program that the chip holds changes nothing"
        );
        assert!(
            book.allowed("ws-2").is_empty(),
            "the chip of one conversation is no chip of another"
        );
    }

    #[test]
    fn a_worker_leaves_the_record_when_its_panel_leaves_the_board() {
        let mut book = WorkerBook::default();
        book.of_mut("ws-1").add(WorkerRecord::new("p-1", "Codex", "codex"));
        book.of_mut("ws-1").add(WorkerRecord::new("p-2", "Pi", "pi"));
        book.of_mut("ws-2").add(WorkerRecord::new("p-3", "Codex", "codex"));
        assert_eq!(book.workers("ws-1").len(), 2);

        book.retain_panels(|panel| panel != "p-1");

        assert_eq!(
            book.workers("ws-1")
                .iter()
                .map(|worker| worker.panel.as_str())
                .collect::<Vec<_>>(),
            ["p-2"],
            "the worker whose panel left the board leaves the record"
        );
        assert_eq!(book.workers("ws-2").len(), 1, "a panel of another conversation stays");

        book.retain_conversations(|local_id| local_id != "ws-1");
        assert!(
            book.workers("ws-1").is_empty(),
            "a conversation that left takes its workers"
        );
    }

    #[test]
    fn one_message_and_one_report_of_a_worker_wait_ten_seconds() {
        let start = Instant::now();
        assert!(due(None, start), "the first text of a worker passes");
        assert!(!due(Some(start), start), "a text in the same instant waits");
        let later = start + Duration::from_millis(WORKER_INTERVAL_MILLIS);
        assert!(due(Some(start), later), "the interval of the bound has passed");
        let before = later
            .checked_sub(Duration::from_millis(1))
            .expect("a moment before the bound");

        assert!(!due(Some(start), before));
    }

    #[test]
    fn the_last_report_of_a_worker_is_cut_and_keeps_its_lines() {
        assert_eq!(cut_report("one\ntwo"), "one\ntwo");
        assert_eq!(cut_report("one\u{7}two"), "one two");
        let long = "r".repeat(MAX_WORKER_REPORT_CHARS + 40);
        assert_eq!(cut_report(&long).chars().count(), MAX_WORKER_REPORT_CHARS);
    }

    #[test]
    fn the_queue_of_the_reports_of_one_worker_is_bounded() {
        let mut book = WorkerBook::default();
        book.of_mut("ws-1").add(WorkerRecord::new("p-1", "Codex", "codex"));
        let workers = book.of_mut("ws-1");
        for _ in 0..MAX_WAITING_REPORTS {
            workers.reports.push_back("a report".to_owned());
        }
        assert_eq!(workers.waiting_reports(), MAX_WAITING_REPORTS);
    }
}
