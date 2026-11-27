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
//! Nothing here is saved: the switch, the rectangle of a frame, the drafts and
//! the conversations are the state of this run, as `sidebar_visible` and
//! `minimap_visible` are.

mod composer;
mod empty;
mod header;
mod layout;
mod list;
mod session;
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

/// The environment switch of this part. `1` turns the dock on; a setting replaces
/// the variable in a later part.
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
    /// Whether the switch of the experiment is on.
    visible: bool,
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
    /// The dock of a start, as the environment switch names it.
    pub(in crate::app) fn from_environment() -> Self {
        Self::from_values(
            dock_switch(std::env::var(DOCK_SWITCH).ok().as_deref()),
            program_from_environment(std::env::var(PROGRAM_SWITCH).ok().as_deref()),
        )
    }

    /// The dock of a start that names its two values.
    fn from_values(visible: bool, program: PathBuf) -> Self {
        Self {
            visible,
            rect: None,
            program,
            sessions: HashMap::new(),
            drafts: HashMap::new(),
            worker: worker::Worker::default(),
            #[cfg(test)]
            last_frame: None,
        }
    }

    pub(in crate::app) fn new(visible: bool) -> Self {
        Self::from_values(visible, program_from_environment(None))
    }

    /// The dock of a test whose program is `program`: the seam of
    /// [`PROGRAM_SWITCH`], which a test uses without the environment of the test
    /// process.
    #[cfg(test)]
    pub(in crate::app) fn with_program(visible: bool, program: PathBuf) -> Self {
        Self::from_values(visible, program)
    }

    pub(in crate::app) fn visible(&self) -> bool {
        self.visible
    }

    pub(in crate::app) fn toggle(&mut self) {
        self.visible = !self.visible;
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
    /// bound, so no start or stop outlives the app.
    pub(in crate::app) fn flush_worker(&mut self) {
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

    /// The conversations that own a child program now, which a test reads.
    #[cfg(test)]
    pub(in crate::app) fn running_bridges(&self) -> usize {
        self.sessions.values().filter(|session| session.has_bridge()).count()
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

/// The program of a start: the value of [`PROGRAM_SWITCH`] when it names one, and
/// [`PROGRAM`] otherwise.
fn program_from_environment(value: Option<&str>) -> PathBuf {
    value
        .filter(|value| !value.is_empty())
        .map_or_else(|| PathBuf::from(PROGRAM), PathBuf::from)
}
