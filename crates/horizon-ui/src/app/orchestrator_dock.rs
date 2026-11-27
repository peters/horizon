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
//! the thread of a frame: both go to the coordination worker of
//! [`super::browser_host_io`], which the exit already waits for. The messages of
//! the person wait in [`session::Session`] until the bridge of their workspace is
//! up.
//!
//! The dock keeps the right edge of the window, and an open Settings panel stands
//! to its left, so the canvas ends at the left edge of the leftmost of the two.
//! Nothing here is saved: the switch, the rectangle of a frame, the drafts and
//! the conversations are the state of this run, as `sidebar_visible` and
//! `minimap_visible` are.

mod composer;
mod header;
mod layout;
mod list;
mod session;
#[cfg(test)]
mod tests;
mod text;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use egui::{Rect, Stroke, Ui, pos2};
use horizon_core::agent_bridge::{Bridge, BridgeError};

use crate::app::settings::settings_panel_default_width;
use crate::app::util::viewport_local_rect;
use crate::app::{HorizonApp, SIDEBAR_WIDTH, TOOLBAR_HEIGHT};
use crate::theme;
use crate::typeface::Weight;

use self::layout::{dock_fits, dock_rect, dock_width};
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
/// The empty state of the dock: the least horizontal padding, the title and the
/// two sentences under it.
const EMPTY_PADDING_X: f32 = 28.0;
const EMPTY_TITLE: &str = "Say what you want done.";
const EMPTY_TITLE_SIZE: f32 = 15.0;
const EMPTY_BODY: &str = "The orchestrator reads the repository, makes a plan and shows it as cards. \
                          It asks before a machine is rented.";
const EMPTY_BODY_SIZE: f32 = 13.5;
const EMPTY_BODY_LINE_HEIGHT: f32 = 1.55;
const EMPTY_GAP: f32 = 8.0;

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

/// The wake of the bridge of a conversation: the next frame of this window.
fn repaint_wake(ctx: &egui::Context) -> Arc<dyn Fn() + Send + Sync> {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}

/// The directory that the orchestrator of a workspace runs in: the directory of
/// that workspace, or the home of the person when the workspace names none.
fn program_directory(board: &horizon_core::Board, local_id: &str) -> PathBuf {
    board
        .workspace_id_by_local_id(local_id)
        .and_then(|id| board.workspace(id))
        .and_then(|workspace| workspace.cwd.clone())
        .unwrap_or_else(|| std::env::var_os("HOME").map_or_else(PathBuf::new, PathBuf::from))
}

impl HorizonApp {
    /// The rectangle that the dock takes in this frame, before it draws: the right
    /// edge of the window, from under the toolbar to its bottom. The answer is
    /// `None` while the switch is off, a panel is fullscreen, or the canvas would
    /// keep less than its least width beside the dock.
    fn orchestrator_dock_rect(&self, ui: &Ui) -> Option<Rect> {
        if !self.orchestrator_dock.visible() || self.fullscreen_panel.is_some() {
            return None;
        }
        let viewport = viewport_local_rect(ui.ctx());
        let available = ui.available_rect_before_wrap();
        let width = dock_width(viewport.width()).min(available.width());
        let sidebar_width = if self.sidebar_visible { SIDEBAR_WIDTH } else { 0.0 };
        let settings_width = self
            .settings
            .as_ref()
            .map_or(0.0, |_| settings_panel_default_width(viewport.width()));
        dock_fits(viewport.width(), sidebar_width, settings_width, width)
            .then(|| dock_rect(Rect::from_min_max(viewport.min, available.max), width))
    }

    /// Takes the rectangle of the dock for this frame and draws its frame. The
    /// rectangle is taken before any panel draws, so the canvas and the minimap of
    /// this frame read it.
    pub(in crate::app) fn render_orchestrator_dock(&mut self, ui: &mut Ui) {
        let dock_rect = self.orchestrator_dock_rect(ui);
        self.orchestrator_dock.set_frame_rect(dock_rect);
        let Some(rect) = dock_rect else {
            return;
        };
        let board = &self.board;
        let dock = &mut self.orchestrator_dock;
        let mut asked = composer::Asked::Nothing;
        egui::Panel::right(DOCK_PANEL_ID)
            .exact_size(rect.width())
            .frame(
                egui::Frame::default()
                    .fill(theme::BG_ELEVATED())
                    .stroke(Stroke::new(1.0_f32, theme::BORDER_SUBTLE())),
            )
            .show(ui, |ui| {
                ui.shrink_clip_rect(ui.max_rect());
                // The toolbar of the window draws over the top of every panel, so
                // the content of the dock starts under it.
                let content = Rect::from_min_max(
                    pos2(ui.max_rect().min.x, ui.max_rect().min.y + TOOLBAR_HEIGHT),
                    ui.max_rect().max,
                );
                let workspace = board.active_workspace.and_then(|id| board.workspace(id));
                let local_id = workspace.map(|workspace| workspace.local_id.as_str());
                let session = local_id.and_then(|local_id| dock.sessions.get(local_id));
                let snapshot = session.map(Session::snapshot);
                // A window without an active workspace has no second line.
                let state = workspace.map(|_| header::State::of(session));
                let busy = snapshot.as_ref().is_some_and(|snapshot| snapshot.busy);
                // The header and the composer are bottom and top edges of the dock,
                // so they are worked out first and the list takes the room between
                // them. A part that drew top down would move the parts under it.
                let header_height = header::height(ui, content.width(), workspace, state);
                let draft = local_id.map_or("", |local_id| dock.draft(local_id));
                let composer = composer::parts(ui, content, draft);
                let parts = layout::parts(content, header_height, composer.rect.height());
                header::render(ui, parts.header, workspace, state);
                let mut empty = String::new();
                let draft = match local_id {
                    Some(local_id) => dock.drafts.entry(local_id.to_owned()).or_default(),
                    None => &mut empty,
                };
                asked = composer::render(ui, &composer, draft, local_id.is_some(), busy);
                match snapshot {
                    Some(snapshot) if !snapshot.entries.is_empty() => list::render(
                        ui,
                        parts.body,
                        &snapshot.entries,
                        list::waits_for_text(&snapshot.entries, snapshot.busy),
                    ),
                    _ => empty_state(ui, parts.body),
                }
                #[cfg(test)]
                dock.set_last_frame(parts, composer);
            });
        let Some(local_id) = self
            .board
            .active_workspace
            .and_then(|id| self.board.workspace(id))
            .map(|workspace| workspace.local_id.clone())
        else {
            return;
        };
        match asked {
            composer::Asked::Send => self.send_orchestrator_message(ui.ctx(), &local_id),
            composer::Asked::Stop => self.stop_orchestrator_conversation(&local_id),
            composer::Asked::Nothing => {}
        }
    }

    /// Reads every bridge of the dock, and applies its events to the conversation
    /// of its workspace. A frame calls it, and so does the pass of a window that
    /// draws no frame: a conversation that nobody shows keeps its events. A
    /// second pass of one frame reads no event twice, because a read takes them
    /// from the channel.
    pub(in crate::app) fn poll_orchestrator_dock(&mut self) {
        for session in self.orchestrator_dock.sessions.values_mut() {
            session.poll();
        }
    }

    /// Sends the draft of the workspace that `local_id` names, and starts the
    /// bridge of that conversation first when it has none.
    fn send_orchestrator_message(&mut self, ctx: &egui::Context, local_id: &str) {
        let Some(text) = self.orchestrator_dock.take_draft(local_id) else {
            return;
        };
        let directory = program_directory(&self.board, local_id);
        let program = self.orchestrator_dock.program.clone();
        let needs_bridge = {
            let session = self.orchestrator_dock.session_mut(local_id);
            let needs_bridge = session.say(&text);
            needs_bridge && !session.is_starting()
        };
        if !needs_bridge {
            return;
        }
        let Some(request) = self.orchestrator_dock.begin_start(local_id) else {
            return;
        };
        let spec = session::program_spec(&program, &directory, request.session.clone(), local_id);
        let wake = repaint_wake(ctx);
        let generation = request.generation;
        let local_id = local_id.to_owned();
        // A start is a process, so it never runs on the thread of this frame.
        self.browser_create_host.io.then(
            move || Bridge::start(spec, session::codec(), wake),
            move |app: &mut HorizonApp, outcome| {
                if let Some(bridge) = app.orchestrator_dock.adopt(&local_id, generation, outcome) {
                    app.browser_create_host.io.write(move || bridge.stop());
                }
            },
        );
    }

    /// Stops the bridge of the workspace that `local_id` names, on the
    /// coordination worker, and leaves the notice of the person.
    pub(in crate::app) fn stop_orchestrator_conversation(&mut self, local_id: &str) {
        let Some(bridge) = self.orchestrator_dock.stop(local_id) else {
            return;
        };
        self.browser_create_host.io.write(move || bridge.stop());
    }

    /// Stops every bridge of the dock, which the exit waits for within its bound.
    pub(in crate::app) fn stop_orchestrator_bridges(&mut self) {
        for bridge in self.orchestrator_dock.stop_all() {
            self.browser_create_host.io.write(move || bridge.stop());
        }
    }

    /// Drops the conversation and the draft of a workspace that closes, and stops
    /// the bridge of that conversation on the worker.
    pub(in crate::app) fn forget_orchestrator_conversation(&mut self, local_id: &str) {
        let Some(mut session) = self.orchestrator_dock.forget(local_id) else {
            return;
        };
        if let Some(bridge) = session.shutdown() {
            self.browser_create_host.io.write(move || bridge.stop());
        }
    }
}

/// The empty state of the dock: how the orchestrator works, centred in the `body`
/// between the header and the composer.
fn empty_state(ui: &Ui, body: Rect) {
    let text_width = body.width() - 2.0 * EMPTY_PADDING_X;
    if text_width <= 0.0 || body.height() <= 0.0 {
        return;
    }
    let title = text::line(ui, EMPTY_TITLE, EMPTY_TITLE_SIZE, Weight::Bold, theme::FG(), text_width);
    let body_text = text::paragraph(
        ui,
        EMPTY_BODY,
        EMPTY_BODY_SIZE,
        Weight::Regular,
        theme::FG_SOFT(),
        text_width,
        EMPTY_BODY_LINE_HEIGHT,
    );
    let title_height = title.size().y;
    let height = title_height + EMPTY_GAP + body_text.size().y;
    let left = body.min.x + EMPTY_PADDING_X;
    let top = body.center().y - height / 2.0;
    if top < body.min.y || top + height > body.max.y {
        return;
    }
    let painter = ui.painter();
    painter.galley(pos2(left, top), title, theme::FG());
    painter.galley(pos2(left, top + title_height + EMPTY_GAP), body_text, theme::FG_SOFT());
}
