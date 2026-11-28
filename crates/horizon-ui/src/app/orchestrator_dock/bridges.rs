//! The bridges of the conversations of the dock: the start and the stop of each
//! one on the worker, the wake of a finished job, and the bound of the close.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use horizon_core::agent_bridge::{Bridge, BridgeError, STOP_TIMEOUT};

use super::programs::{self, Program};
use super::session::{self, Session, Start};
use super::{FRONT_DESK, OrchestratorDock};

impl OrchestratorDock {
    /// The conversation of `local_id`, which a message of the person creates.
    pub(super) fn session_mut(&mut self, local_id: &str) -> &mut Session {
        self.sessions.entry(local_id.to_owned()).or_insert_with(Session::new)
    }

    /// Takes the request of one start of the conversation of `local_id`.
    pub(super) fn begin_start(&mut self, local_id: &str) -> Option<Start> {
        self.sessions.get_mut(local_id).map(Session::begin_start)
    }

    /// Whether the start of the conversation of `local_id` waits on the worker.
    pub(super) fn is_starting(&self, local_id: &str) -> bool {
        self.sessions.get(local_id).is_some_and(Session::is_starting)
    }

    /// Keeps the bridge of a start that answers `generation`, and answers the
    /// bridge that the caller stops instead.
    fn adopt(
        &mut self,
        local_id: &str,
        generation: u64,
        outcome: Option<Result<Bridge, BridgeError>>,
    ) -> Option<Bridge> {
        match self.sessions.get_mut(local_id) {
            Some(session) => session.adopt(generation, outcome),
            // The conversation went with its workspace: the bridge of that
            // start serves nobody.
            None => outcome.and_then(Result::ok),
        }
    }

    /// Stops the conversation of `local_id`, and answers the bridge that the
    /// caller stops on the worker.
    pub(super) fn stop(&mut self, local_id: &str) -> Option<Bridge> {
        self.sessions.get_mut(local_id)?.stop()
    }

    /// Stops every conversation of the dock, the front desk with them, and forgets
    /// the record of the front desk: the board that they speak for goes, and so
    /// does the session that the record belongs to.
    pub(in crate::app) fn stop_and_clear(&mut self) {
        self.stop_all_bridges();
        self.sessions.clear();
        self.drafts.clear();
        self.front_desk.clear();
    }

    /// Takes the bridge of every conversation, for the end of Horizon.
    fn stop_all(&mut self) -> Vec<Bridge> {
        self.sessions.values_mut().filter_map(Session::shutdown).collect()
    }

    /// Takes the conversation of `local_id`, with its draft, for a workspace
    /// that closes.
    pub(super) fn forget(&mut self, local_id: &str) -> Option<Session> {
        self.drafts.remove(local_id);
        self.sessions.remove(local_id)
    }

    /// Starts the bridge of the conversation of `local_id` on the worker of the
    /// dock, and keeps the bridge of the answer to its start. A start opens a
    /// process, so a frame never waits for it, and the specification of that
    /// program is built on the worker as well: the identity of a workspace reads
    /// the file system.
    pub(super) fn start_bridge(
        &mut self,
        program: PathBuf,
        directory: PathBuf,
        local_id: &str,
        start: Start,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) {
        let generation = start.generation;
        let session = start.session;
        let started_id = local_id.to_owned();
        let applied_id = local_id.to_owned();
        let handed = self.worker.then(
            move || {
                // The folder of the front desk is made here and not on the frame:
                // a frame reads no directory and makes none.
                if started_id == FRONT_DESK {
                    session::make_directory(&directory, FRONT_DESK);
                }
                let spec = session::program_spec(&program, &directory, session, &started_id);
                Bridge::start(spec, session::codec(), wake)
            },
            move |dock, outcome| dock.apply_start(&applied_id, generation, outcome),
        );
        if !handed {
            // No worker took the start, so no program of this conversation runs:
            // the person reads the notice that names it, and the next message
            // tries a start again.
            self.apply_start(local_id, generation, None);
        }
    }

    /// Keeps the bridge of the start that answers `generation`, and answers the
    /// bridge that the caller stops instead.
    fn apply_start(&mut self, local_id: &str, generation: u64, outcome: Option<Result<Bridge, BridgeError>>) {
        if let Some(bridge) = self.adopt(local_id, generation, outcome) {
            // The answer of a start that the person replaced, or of a second start
            // of a live conversation, serves nobody: it is stopped here.
            self.stop_bridge(bridge);
        }
    }

    /// Stops `bridge` on the worker of the dock: a stop closes the input, waits
    /// for the program and ends it, so a frame never runs it.
    ///
    /// The identity of the program is recorded here and not in the job of the
    /// worker, because a job that never runs, from a worker that does not start or
    /// from a queue that the exit left behind, must still leave the identity of a
    /// program that the close of Horizon has to end. One stop reads the process
    /// table once, and no repeated frame path does.
    pub(super) fn stop_bridge(&mut self, bridge: Bridge) {
        let program = Program::record(bridge.pid());
        let pid = program.pid();
        self.programs.push(program);
        self.stops_pending = self.stops_pending.saturating_add(1);
        let handed = self
            .worker
            .then(move || bridge.stop(), move |dock, _stopped| dock.stop_finished(pid));
        if !handed {
            // No worker took the stop, so nothing closes the input of that
            // program: it stays recorded, and the close of Horizon ends it.
            self.stops_pending = self.stops_pending.saturating_sub(1);
        }
    }

    /// Applies the end of the stop of `pid` on the worker: the program of that
    /// stop ended there, so the close of Horizon has nothing to end for it.
    fn stop_finished(&mut self, pid: u32) {
        self.stops_pending = self.stops_pending.saturating_sub(1);
        self.programs.retain(|program| program.pid() != pid);
    }

    /// Stops every bridge of the dock on the worker, which the exit calls once.
    pub(super) fn stop_all_bridges(&mut self) {
        let bridges = self.stop_all();
        for bridge in bridges {
            self.stop_bridge(bridge);
        }
    }

    /// The stops of the dock that the worker did not finish, which the pass of a
    /// close shows beside the panels of the terminal.
    pub(in crate::app) fn stops_pending(&self) -> usize {
        self.stops_pending
    }

    /// Whether the pass of a close still waits for a stop of the dock. A stop
    /// closes the input of its program, waits for it and ends it, so the wait is
    /// on the worker and never on this thread. Its bound is one stop of a bridge
    /// for each stop that it waits for; a bound that passes ends every program
    /// that its stop did not reach, by the identity that the dock recorded.
    pub(in crate::app) fn stops_hold_exit(&mut self) -> bool {
        if self.stops_pending == 0 {
            self.close_wait = None;
            self.end_remaining_programs(Duration::ZERO);
            return false;
        }
        let bound = self.stop_bound();
        let (started, bound) = *self.close_wait.get_or_insert((Instant::now(), bound));
        if started.elapsed() < bound {
            return true;
        }
        tracing::warn!(
            stops = self.stops_pending,
            "the exit ends the programs that their stop did not reach"
        );
        self.close_wait = None;
        self.end_remaining_programs(Duration::ZERO);
        false
    }

    /// The bound of a wait for the stops of the dock: one stop of a bridge for
    /// each stop that it waits for.
    fn stop_bound(&self) -> Duration {
        let stops = u32::try_from(self.stops_pending.max(1)).unwrap_or(u32::MAX);
        STOP_TIMEOUT.saturating_mul(stops)
    }

    /// Ends every program whose stop did not end it, by the identity that the dock
    /// recorded, so no program of a conversation outlives the exit of Horizon. A
    /// frame passes no grace, and an exit that already holds its thread passes the
    /// moment that the termination signal gets before the kill signal.
    fn end_remaining_programs(&mut self, grace: Duration) {
        for program in self.programs.drain(..) {
            if !program.end(grace) {
                tracing::error!(pid = program.pid(), "a program of a conversation outlives its end");
            }
        }
    }

    /// Applies, in order, the outcomes of the worker of the dock.
    pub(super) fn poll_worker(&mut self) {
        for apply in self.worker.take_finished() {
            apply(self);
        }
    }

    /// Closes the worker of the dock for an exit that waits for it, so no start,
    /// stop or write outlives the app. The wait is one stop of a bridge for each
    /// stop that it waits for, and a program that its stop did not reach ends by
    /// the identity that the dock recorded. The entries that the exit leaves open
    /// go to their files first: a tool call that still runs reads as interrupted
    /// on the next start.
    pub(in crate::app) fn flush_worker(&mut self) {
        self.flush_conversation_appends();
        let bound = self.stop_bound();
        self.worker.flush(bound);
        self.stops_pending = 0;
        self.close_wait = None;
        self.end_remaining_programs(programs::END_GRACE);
    }
}
