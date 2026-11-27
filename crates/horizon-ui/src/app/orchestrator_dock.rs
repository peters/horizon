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
use std::time::Duration;

use egui::Rect;
use horizon_core::agent_bridge::{Bridge, BridgeError, ProgramSpec};

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
/// The room that the exit waits for the worker of the dock: the bound of a stop,
/// as the exit waits for the coordination worker.
const WORKER_FLUSH: Duration = Duration::from_secs(2);

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
    /// The draft is gone after this answer: the message of the person is the
    /// conversation's, and the field is empty again.
    fn take_draft(&mut self, local_id: &str) -> Option<String> {
        let text = self.drafts.get(local_id)?.trim().to_owned();
        if text.is_empty() {
            return None;
        }
        self.drafts.remove(local_id);
        Some(text)
    }

    /// The conversation of `local_id`, which a message of the person creates.
    fn session_mut(&mut self, local_id: &str) -> &mut Session {
        self.sessions.entry(local_id.to_owned()).or_insert_with(Session::new)
    }

    /// Takes the request of one start of the conversation of `local_id`.
    fn begin_start(&mut self, local_id: &str) -> Option<Start> {
        self.sessions.get_mut(local_id).map(Session::begin_start)
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
    /// dock, and keeps the bridge of the answer to `generation`. A start opens a
    /// process, so a frame never waits for it.
    fn start_bridge(
        &mut self,
        spec: ProgramSpec,
        local_id: String,
        generation: u64,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.worker.then(
            move || Bridge::start(spec, session::codec(), wake),
            move |dock, outcome| {
                if let Some(bridge) = dock.adopt(&local_id, generation, outcome) {
                    // The answer belongs to a start that the person replaced: its
                    // bridge serves nobody and is stopped here.
                    dock.worker.write(move || bridge.stop());
                }
            },
        );
    }

    /// Stops `bridge` on the worker of the dock: a stop closes the input, waits
    /// for the program and ends it, so a frame never runs it.
    fn stop_bridge(&mut self, bridge: Bridge) {
        self.worker.write(move || bridge.stop());
    }

    /// Applies, in order, the outcomes of the worker of the dock.
    fn poll_worker(&mut self) {
        for apply in self.worker.take_finished() {
            apply(self);
        }
    }

    /// Closes the worker of the dock for an exit that waits for it within its own
    /// bound, so no start, stop or write outlives the app. The entries that the
    /// exit leaves open go to their files first: a tool call that still runs
    /// reads as interrupted on the next start.
    pub(in crate::app) fn flush_worker(&mut self) {
        self.flush_conversation_appends();
        self.worker.flush(WORKER_FLUSH);
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
    /// the person, and starts the program of that conversation first when it has
    /// none. It is the path of the send of the composer in the four parts that
    /// the dock keeps: `Session::say` holds the message, `begin_start` names the
    /// start, `program_spec` builds it, and the worker of the dock runs it. A
    /// start opens a process, so no part of it runs on the thread of the frame.
    ///
    /// It answers whether a message went out: a window without an active
    /// workspace sends nothing, and neither does one whose orchestrator works.
    pub(in crate::app) fn say_to_active_orchestrator(&mut self, ctx: &egui::Context, text: &str) -> bool {
        let Some((local_id, directory)) = self
            .board
            .active_workspace
            .and_then(|id| self.board.workspace(id))
            .map(|workspace| (workspace.local_id.clone(), program_directory(workspace.cwd.as_deref())))
        else {
            return false;
        };
        if self.orchestrator_dock.busy(&local_id) {
            return false;
        }
        let program = self.orchestrator_dock.program.clone();
        let needs_bridge = {
            let session = self.orchestrator_dock.session_mut(&local_id);
            let needs_bridge = session.say(text);
            needs_bridge && !session.is_starting()
        };
        if !needs_bridge {
            return true;
        }
        let Some(request) = self.orchestrator_dock.begin_start(&local_id) else {
            return false;
        };
        let spec = session::program_spec(&program, &directory, request.session.clone(), &local_id);
        self.orchestrator_dock
            .start_bridge(spec, local_id, request.generation, repaint_wake(ctx));
        true
    }
}

/// The directory that the program of a conversation runs in: the directory of
/// its workspace, or the home of the person when the workspace names none.
fn program_directory(cwd: Option<&std::path::Path>) -> PathBuf {
    cwd.map_or_else(
        || std::env::var_os("HOME").map_or_else(PathBuf::new, PathBuf::from),
        std::path::Path::to_path_buf,
    )
}

/// The wake of a bridge of a conversation that the sidebar started: the next
/// frame of this window.
fn repaint_wake(ctx: &egui::Context) -> Arc<dyn Fn() + Send + Sync> {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
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
