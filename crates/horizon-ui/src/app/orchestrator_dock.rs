//! The orchestrator dock: the column at the right edge of the root window that
//! holds the conversation with the orchestrator of a workspace.
//!
//! The dock draws the frame of the experiment, the header, the list of the
//! entries and the composer. One conversation belongs to one workspace, keyed by
//! the `local_id` that a restart keeps, and each conversation keeps its own
//! bridge of `horizon_core::agent_bridge`: the program of that conversation runs
//! with no terminal, and its events arrive on a channel that a frame reads with
//! `try_iter`. A conversation that the dock does not show keeps running.
//!
//! The start of a bridge starts a process and a stop can wait, so neither runs on
//! the thread of a frame: both go to the worker of [`worker`], a thread of the
//! dock named `orchestrator-dock`. That worker is separate from the coordination
//! worker of [`super::browser_host_io`], so a stop that waits never takes the
//! room of a browser job. The messages of the person wait in
//! [`session::Session`] until the bridge of their workspace is up.
//!
//! The dock keeps the right edge of the window, and an open Settings panel stands
//! to its left, so the canvas ends at the left edge of the leftmost of the two.
//! The setting `features.orchestrator_dock` turns the dock on, and the
//! environment switch [`DOCK_SWITCH`] does the same for the fixture of the smoke
//! test. The conversation of a saved session outlives the run in the session
//! folder ([`storage`]); the switch of a run that the palette turned, the
//! rectangle of a frame and the drafts are the state of this run, as
//! `sidebar_visible` and `minimap_visible` are.

mod composer;
mod empty;
mod header;
mod layout;
mod list;
mod programs;
mod session;
mod storage;
#[cfg(test)]
mod tests;
mod text;
mod window;
mod worker;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::Rect;
use horizon_core::agent_bridge::{Bridge, BridgeError, STOP_TIMEOUT};

use self::programs::Program;
use self::session::{Session, Start};
use super::HorizonApp;

/// The environment switch of this part. `1` turns the dock on, for the fixture of
/// the smoke test: the setting `features.orchestrator_dock` turns it on for a
/// person.
pub(in crate::app) const DOCK_SWITCH: &str = "HORIZON_ORCHESTRATOR_DOCK";
/// The environment variable that replaces the program path of the orchestrator.
/// The arguments of the program stay the same. It exists for the stand-in of the
/// fixture and for the tests.
pub(in crate::app) const PROGRAM_SWITCH: &str = "HORIZON_ORCHESTRATOR_PROGRAM";
/// The program of a start that the environment does not replace.
const PROGRAM: &str = "claude";
/// The panel id of the dock, which egui remembers the place of.
const DOCK_PANEL_ID: &str = "orchestrator_dock";

/// The state of the dock of this run.
pub(in crate::app) struct OrchestratorDock {
    /// The setting `features.orchestrator_dock`, as the last frame read it.
    setting: bool,
    /// Whether that setting is the switch of the dock: the dock of a run follows
    /// the settings of the person, and a dock whose start named its switch, as a
    /// test does, keeps it.
    follows_setting: bool,
    /// The environment switch of the fixture, which holds for the whole run.
    environment: bool,
    /// The switch of this run after the palette turned it: `None` until that
    /// press, and then the choice of the person for this run. The setting stays
    /// as Settings holds it.
    run_switch: Option<bool>,
    /// The rectangle that the dock took in this frame, from before it drew.
    rect: Option<Rect>,
    /// The program of every conversation: `claude`, or the path that the
    /// environment names in its place.
    program: PathBuf,
    /// The conversation of each workspace, by the `local_id` that a restart
    /// keeps. A `WorkspaceId` is new at every start, so it cannot be the key.
    sessions: HashMap<String, Session>,
    /// The draft of each workspace, by the same `local_id`.
    drafts: HashMap<String, String>,
    /// The folder that keeps one conversation file for each workspace of the
    /// session that this app opened, or `None` while nothing is saved.
    storage: Option<PathBuf>,
    /// The worker that starts and stops the bridges of the conversations, in the
    /// order that the frames ask for them.
    worker: worker::Worker,
    /// The stops of the bridges that the worker did not finish, and the programs
    /// whose stop still runs there. The close of Horizon waits for them.
    stops_pending: usize,
    programs: Vec<Program>,
    /// When the close began to wait for those stops, and the bound of that wait.
    close_wait: Option<(Instant, Duration)>,
    /// The rectangles that the dock worked out in the last frame of a test, which
    /// the tests of the layout read.
    #[cfg(test)]
    last_frame: Option<LastFrame>,
}

impl Default for OrchestratorDock {
    fn default() -> Self {
        Self::new(false)
    }
}

/// The rectangles of the parts of the dock in one frame, which a test reads.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(in crate::app) struct LastFrame {
    pub(in crate::app) header: Rect,
    pub(in crate::app) body: Rect,
    pub(in crate::app) composer: Rect,
    pub(in crate::app) field: Rect,
    pub(in crate::app) send: Rect,
    pub(in crate::app) chips: Rect,
}

impl OrchestratorDock {
    /// The dock of a start whose settings hold `setting`: the dock is on when the
    /// setting is on, or when the fixture names the environment switch.
    pub(in crate::app) fn from_settings(setting: bool) -> Self {
        let mut dock = Self::from_values(
            setting,
            dock_switch(std::env::var(DOCK_SWITCH).ok().as_deref()),
            program_from_environment(std::env::var(PROGRAM_SWITCH).ok().as_deref()),
        );
        dock.follows_setting = true;
        dock
    }

    /// The dock of a start that names its values: the setting and the environment
    /// switch, which a test gives without the environment of the test process.
    fn from_values(setting: bool, environment: bool, program: PathBuf) -> Self {
        Self {
            setting,
            follows_setting: false,
            environment,
            run_switch: None,
            rect: None,
            program,
            sessions: HashMap::new(),
            drafts: HashMap::new(),
            storage: None,
            worker: worker::Worker::default(),
            stops_pending: 0,
            programs: Vec::new(),
            close_wait: None,
            #[cfg(test)]
            last_frame: None,
        }
    }

    pub(in crate::app) fn new(visible: bool) -> Self {
        Self::from_values(visible, false, program_from_environment(None))
    }

    /// The dock of a test whose program is `program`: the seam of
    /// [`PROGRAM_SWITCH`], which a test uses without the environment of the test
    /// process.
    #[cfg(test)]
    pub(in crate::app) fn with_program(visible: bool, program: PathBuf) -> Self {
        Self::from_values(visible, false, program)
    }

    /// Replaces the program of every conversation of this dock, which a test does
    /// in place of a whole dock, so the startup decision of the app keeps its
    /// stored folder.
    #[cfg(test)]
    pub(in crate::app) fn set_program(&mut self, program: PathBuf) {
        self.program = program;
    }

    /// Whether the dock draws in this frame. A run that the palette did not turn
    /// keeps the setting of Settings and the environment switch of the fixture,
    /// so a person who changes the setting sees the dock without a restart.
    pub(in crate::app) fn visible(&self) -> bool {
        self.run_switch.unwrap_or(self.setting || self.environment)
    }

    pub(in crate::app) fn toggle(&mut self) {
        self.run_switch = Some(!self.visible());
    }

    /// Reads the setting of Settings again. The switch of the palette keeps the
    /// choice of this run, so the dock that a person turned over stays so until
    /// the next start.
    pub(in crate::app) fn follow_setting(&mut self, setting: bool) {
        if self.follows_setting {
            self.setting = setting;
        }
    }

    /// The dock of a test whose switch follows the setting of Settings, as the
    /// dock of a run does, without the environment of the test process.
    #[cfg(test)]
    pub(in crate::app) fn following(setting: bool) -> Self {
        let mut dock = Self::from_values(setting, false, program_from_environment(None));
        dock.follows_setting = true;
        dock
    }

    /// The rectangle of the dock in this frame. A frame that draws no dock has
    /// none, so the canvas keeps the whole window.
    pub(in crate::app) fn frame_rect(&self) -> Option<Rect> {
        self.rect
    }

    pub(in crate::app) fn set_frame_rect(&mut self, rect: Option<Rect>) {
        self.rect = rect;
    }

    /// The draft of the workspace that `local_id` names, which is empty until the
    /// person writes in it.
    fn draft(&self, local_id: &str) -> &str {
        self.drafts.get(local_id).map_or("", String::as_str)
    }

    /// The draft of the workspace that `local_id` names, which a test writes as a
    /// person who typed in the field would.
    #[cfg(test)]
    pub(in crate::app) fn draft_mut(&mut self, local_id: &str) -> &mut String {
        self.drafts.entry(local_id.to_owned()).or_default()
    }

    /// The text of the draft of `local_id`, when it holds more than white space.
    /// The field keeps the writing of the person: the send clears it, and a
    /// message that goes out from another path leaves it alone.
    fn draft_text(&self, local_id: &str) -> Option<String> {
        let text = self.drafts.get(local_id)?.trim().to_owned();
        (!text.is_empty()).then_some(text)
    }

    /// Empties the field of `local_id`, which a message that went out does.
    fn clear_draft(&mut self, local_id: &str) {
        self.drafts.remove(local_id);
    }

    /// The conversation of `local_id`, which a message of the person creates.
    fn session_mut(&mut self, local_id: &str) -> &mut Session {
        self.sessions.entry(local_id.to_owned()).or_insert_with(Session::new)
    }

    /// Takes the request of one start of the conversation of `local_id`.
    fn begin_start(&mut self, local_id: &str) -> Option<Start> {
        self.sessions.get_mut(local_id).map(Session::begin_start)
    }

    /// Whether the start of the conversation of `local_id` waits on the worker.
    fn is_starting(&self, local_id: &str) -> bool {
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
    fn stop(&mut self, local_id: &str) -> Option<Bridge> {
        self.sessions.get_mut(local_id)?.stop()
    }

    /// Takes the bridge of every conversation, for the end of Horizon.
    fn stop_all(&mut self) -> Vec<Bridge> {
        self.sessions.values_mut().filter_map(Session::shutdown).collect()
    }

    /// Takes the conversation of `local_id`, with its draft, for a workspace
    /// that closes.
    fn forget(&mut self, local_id: &str) -> Option<Session> {
        self.drafts.remove(local_id);
        self.sessions.remove(local_id)
    }

    /// Starts the bridge of the conversation of `local_id` on the worker of the
    /// dock, and keeps the bridge of the answer to its start. A start opens a
    /// process, so a frame never waits for it, and the specification of that
    /// program is built on the worker as well: the identity of a workspace reads
    /// the file system.
    fn start_bridge(
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
    fn stop_bridge(&mut self, bridge: Bridge) {
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
    fn stop_all_bridges(&mut self) {
        let bridges = self.stop_all();
        for bridge in bridges {
            self.stop_bridge(bridge);
        }
    }

    /// Stops every bridge of the dock on the worker and forgets every
    /// conversation and draft: the board that they speak for goes, as the programs
    /// that its panels hold do.
    pub(in crate::app) fn stop_and_clear(&mut self) {
        self.stop_all_bridges();
        self.sessions.clear();
        self.drafts.clear();
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
    fn poll_worker(&mut self) {
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

    /// Keeps the text of the example that the person clicked in the draft of
    /// `local_id`, when that draft holds no text yet. The answer says whether the
    /// draft took it: then the field takes the keyboard. The click sends nothing.
    fn pick_example(&mut self, local_id: &str, text: &str) -> bool {
        let draft = self.drafts.entry(local_id.to_owned()).or_default();
        if !draft.trim().is_empty() {
            return false;
        }
        draft.clear();
        draft.push_str(text);
        true
    }

    /// Keeps the rectangles of this frame for the tests of the layout.
    #[cfg(test)]
    fn set_last_frame(&mut self, parts: layout::Parts, composer: composer::Parts) {
        self.last_frame = Some(LastFrame {
            header: parts.header,
            body: parts.body,
            composer: parts.composer,
            field: composer.field,
            send: composer.send,
            chips: composer.chips,
        });
    }

    /// The rectangles of the parts of the last frame, which a test reads to
    /// assert where they stand.
    #[cfg(test)]
    pub(in crate::app) fn last_frame(&self) -> Option<LastFrame> {
        self.last_frame
    }

    /// The conversation of `local_id`, which a test reads.
    #[cfg(test)]
    fn session(&self, local_id: &str) -> Option<&Session> {
        self.sessions.get(local_id)
    }

    /// Whether a program of the dock runs for the workspace that `local_id`
    /// names now, which is the presence of the orchestrator of that workspace:
    /// a card of it reports that its owner is here while this answers `true`.
    pub(in crate::app) fn running(&self, local_id: &str) -> bool {
        self.sessions.get(local_id).is_some_and(Session::has_bridge)
    }

    /// Whether the conversation of `local_id` runs a turn now, which the button
    /// of the sidebar reads: a message waits for that turn.
    pub(in crate::app) fn busy(&self, local_id: &str) -> bool {
        self.sessions.get(local_id).is_some_and(Session::busy)
    }

    /// The conversations that own a child program now, which a test reads.
    #[cfg(test)]
    pub(in crate::app) fn running_bridges(&self) -> usize {
        self.sessions.values().filter(|session| session.has_bridge()).count()
    }

    /// The messages of the person of the conversation of `local_id`, which a test
    /// reads: the words that the dock draws in the bubble of the person.
    #[cfg(test)]
    pub(in crate::app) fn person_messages(&self, local_id: &str) -> Vec<String> {
        self.sessions.get(local_id).map_or_else(Vec::new, |session| {
            session
                .snapshot()
                .entries
                .iter()
                .filter_map(|entry| match entry {
                    horizon_core::agent_bridge::Entry::Person { text } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        })
    }
}

/// The panel of the right edge that the canvas must stop at: the leftmost of the
/// panels that stand there, which is the dock or an open Settings panel.
pub(in crate::app) fn leftmost_panel(panels: [Option<Rect>; 2]) -> Option<Rect> {
    panels
        .into_iter()
        .flatten()
        .reduce(|left, rect| if rect.min.x < left.min.x { rect } else { left })
}

/// Whether the switch of the dock is on: the variable holds `1` and nothing else.
fn dock_switch(value: Option<&str>) -> bool {
    value == Some("1")
}

impl HorizonApp {
    /// Sends `text` to the orchestrator of the active workspace as the message of
    /// the person. It is the path of the button `+ Rent a machine`, which writes
    /// no draft: the send itself is [`HorizonApp::say_to_orchestrator`], which the
    /// composer calls too.
    ///
    /// It answers whether a message went out: a window without an active workspace
    /// sends nothing, and neither does one whose orchestrator works.
    pub(in crate::app) fn say_to_active_orchestrator(&mut self, ctx: &egui::Context, text: &str) -> bool {
        let Some(local_id) = self
            .board
            .active_workspace
            .and_then(|id| self.board.workspace(id))
            .map(|workspace| workspace.local_id.clone())
        else {
            return false;
        };
        self.say_to_orchestrator(ctx, &local_id, text)
    }
}

/// The program of a start: the value of [`PROGRAM_SWITCH`] when it names one, and
/// [`PROGRAM`] otherwise.
fn program_from_environment(value: Option<&str>) -> PathBuf {
    value
        .filter(|value| !value.is_empty())
        .map_or_else(|| PathBuf::from(PROGRAM), PathBuf::from)
}

/// The switch of the dock of a run: the setting of Settings, the environment
/// variable of the fixture, and the switch that the palette turned for this run.
#[cfg(test)]
mod switch_tests {
    use super::OrchestratorDock;

    /// A dock whose program is the default one, so no test starts a program.
    fn dock(setting: bool, environment: bool) -> OrchestratorDock {
        OrchestratorDock::from_values(setting, environment, std::path::PathBuf::from("claude"))
    }

    #[test]
    fn the_setting_turns_the_dock_on() {
        assert!(dock(true, false).visible(), "the setting is on");
        assert!(
            !dock(false, false).visible(),
            "the setting is off and no variable names it"
        );
    }

    #[test]
    fn the_environment_variable_turns_the_dock_on_without_the_setting() {
        assert!(dock(false, true).visible(), "the fixture names the variable");
        assert!(dock(true, true).visible(), "both turn it on");
    }

    /// The dock of a run, whose switch is the setting of Settings.
    fn following(setting: bool) -> OrchestratorDock {
        OrchestratorDock::following(setting)
    }

    #[test]
    fn the_setting_of_settings_follows_the_window_without_a_restart() {
        let mut dock = following(false);
        assert!(!dock.visible());

        dock.follow_setting(true);

        assert!(dock.visible(), "a person who turns the setting on sees the dock");
        dock.follow_setting(false);
        assert!(!dock.visible(), "a person who turns it off sees it go");
    }

    /// The palette turns the dock of this run over and writes no setting: a run
    /// that the palette turned keeps that choice while the setting stays as it is.
    #[test]
    fn a_dock_whose_start_named_its_switch_keeps_it() {
        let mut dock = dock(true, false);
        dock.follow_setting(false);

        assert!(
            dock.visible(),
            "the switch of a start that named it is no setting of Settings"
        );
    }

    #[test]
    fn the_palette_turns_the_dock_of_this_run_over_the_setting() {
        let mut off = following(false);
        off.toggle();
        assert!(off.visible());
        off.follow_setting(true);
        off.follow_setting(false);
        assert!(off.visible(), "the choice of the person of this run stays");
        off.toggle();
        assert!(!off.visible());

        let mut named = dock(false, true);
        named.toggle();
        assert!(
            !named.visible(),
            "the palette turns off a dock that the variable turned on"
        );
    }
}
