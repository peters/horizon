//! The window side of the dock: the frame that draws it, the events that it
//! reads, and the actions that start and stop the bridge of a conversation.
//!
//! The frame takes the rectangle of the dock before any panel draws, so the
//! canvas and the minimap of the frame read it. A start and a stop go to the
//! worker of the dock and never run on this thread.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Rect, Stroke, Ui, pos2};
use horizon_core::agent_bridge::MAX_TEXT_BYTES;

use crate::app::settings::settings_panel_default_width;
use crate::app::util::viewport_local_rect;
use crate::app::{HorizonApp, SIDEBAR_WIDTH, TOOLBAR_HEIGHT};
use crate::theme;

use super::layout::{dock_fits, dock_rect, dock_width};
use super::session::{Said, Session, TOO_LONG_NOTICE};
use super::{DOCK_PANEL_ID, composer, empty, header, layout, list};

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
        // The worker wakes this window when an outcome is ready, so a start or a
        // stop applies without input.
        self.orchestrator_dock.worker.wake_with(ui.ctx());
        let board = &self.board;
        let dock = &mut self.orchestrator_dock;
        let mut asked = composer::Asked::Nothing;
        let mut picked = None;
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
                        local_id.unwrap_or_default(),
                        &snapshot.entries,
                        list::waits_for_text(&snapshot.entries, snapshot.busy),
                    ),
                    _ => picked = empty::render(ui, parts.body),
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
        if let Some(text) = picked
            && self.orchestrator_dock.pick_example(&local_id, text)
        {
            ui.ctx()
                .memory_mut(|memory| memory.request_focus(egui::Id::unique(composer::FIELD_ID)));
        }
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
    ///
    /// The same pass keeps the stored side in step: it loads the conversation of
    /// a workspace that a saved board brings back, it keeps the record of each
    /// session in the board, and it queues one write for each conversation whose
    /// entries the file does not hold yet. Each of those jobs runs on the worker
    /// of the dock, and its outcome applies on a later frame.
    pub(in crate::app) fn poll_orchestrator_dock(&mut self) {
        for session in self.orchestrator_dock.sessions.values_mut() {
            session.poll();
        }
        self.orchestrator_dock.poll_worker();
        // A workspace that closes takes its conversation with it: no program of
        // the dock speaks for a workspace that the board no longer holds.
        self.forget_workspaces_the_board_left();
        let program = self.orchestrator_dock.program_name().to_owned();
        self.orchestrator_dock.restore_conversations(&self.board);
        if self
            .orchestrator_dock
            .sync_conversation_records(&mut self.board, &program)
        {
            self.mark_runtime_dirty();
        }
        self.orchestrator_dock.queue_conversation_appends();
    }

    /// Forgets every conversation whose workspace the board no longer holds: its
    /// program ends on the worker, and its stored conversation leaves the session
    /// folder. A workspace leaves the board when its last panel closes, and the
    /// dock of that workspace has no control left to stop it with.
    fn forget_workspaces_the_board_left(&mut self) {
        let gone: Vec<String> = self
            .orchestrator_dock
            .sessions
            .keys()
            .filter(|local_id| self.board.workspace_id_by_local_id(local_id).is_none())
            .cloned()
            .collect();
        for local_id in gone {
            self.forget_orchestrator_conversation(&local_id);
        }
    }

    /// Sends `text` to the orchestrator of the workspace that `local_id` names, as
    /// the message of the person, and starts the program of that conversation when
    /// it has none. It is the one send of the dock: the composer and the button
    /// `+ Rent a machine` of the sidebar both call it, and no part of it runs a
    /// process on the thread of the frame.
    ///
    /// It answers whether a message went out: a workspace that the board does not
    /// hold sends nothing, a conversation that works sends nothing, and a text
    /// above [`MAX_TEXT_BYTES`] bytes sends nothing and keeps the field of the
    /// person.
    pub(in crate::app) fn say_to_orchestrator(&mut self, ctx: &egui::Context, local_id: &str, text: &str) -> bool {
        if self.board.workspace_id_by_local_id(local_id).is_none() || self.orchestrator_dock.busy(local_id) {
            return false;
        }
        if text.len() > MAX_TEXT_BYTES {
            // The program would refuse that text, so the dock begins no turn: the
            // field keeps the writing of the person, and one notice names why.
            self.orchestrator_dock.session_mut(local_id).notice(TOO_LONG_NOTICE);
            return false;
        }
        let directory = program_directory(&self.board, local_id);
        let said = self.orchestrator_dock.session_mut(local_id).say(text);
        let (needs_bridge, stopped) = match said {
            Said::Taken => (false, None),
            Said::Start => (true, None),
            // The program of this conversation took no message: it ends on the
            // worker, and a new program carries what waits.
            Said::StartAfter(bridge) => (true, Some(bridge)),
        };
        if let Some(bridge) = stopped {
            self.orchestrator_dock.stop_bridge(bridge);
        }
        if !needs_bridge || self.orchestrator_dock.is_starting(local_id) {
            return true;
        }
        let Some(start) = self.orchestrator_dock.begin_start(local_id) else {
            return false;
        };
        let program = self.orchestrator_dock.program.clone();
        self.orchestrator_dock
            .start_bridge(program, directory, local_id, start, repaint_wake(ctx));
        true
    }

    /// Sends the draft of the workspace that `local_id` names, and starts the
    /// program of that conversation when it has none. A message that does not go
    /// out keeps the draft, so the person shortens it instead of writing it again.
    fn send_orchestrator_message(&mut self, ctx: &egui::Context, local_id: &str) {
        let Some(text) = self.orchestrator_dock.draft_text(local_id) else {
            return;
        };
        if self.say_to_orchestrator(ctx, local_id, &text) {
            self.orchestrator_dock.clear_draft(local_id);
        }
    }

    /// Writes `text` in the draft of `local_id` and sends it, as a person who
    /// typed it and pressed Enter would. A test that reads the stored side of
    /// the dock uses it without the keyboard.
    #[cfg(test)]
    pub(in crate::app) fn send_orchestrator_text(&mut self, ctx: &egui::Context, local_id: &str, text: &str) {
        self.orchestrator_dock.draft_mut(local_id).push_str(text);
        self.send_orchestrator_message(ctx, local_id);
    }

    /// Stops the bridge of the workspace that `local_id` names, on the worker of
    /// the dock, and leaves the notice of the person.
    pub(in crate::app) fn stop_orchestrator_conversation(&mut self, local_id: &str) {
        if let Some(bridge) = self.orchestrator_dock.stop(local_id) {
            self.orchestrator_dock.stop_bridge(bridge);
        }
    }

    /// Stops every bridge of the dock, and waits for their stops within the bound
    /// of the exit.
    pub(in crate::app) fn stop_orchestrator_bridges(&mut self) {
        self.orchestrator_dock.stop_all_bridges();
    }

    /// Drops the conversation and the draft of a workspace that closes, stops
    /// the bridge of that conversation on the worker of the dock, and removes the
    /// stored conversation of that workspace from its session folder.
    pub(in crate::app) fn forget_orchestrator_conversation(&mut self, local_id: &str) {
        self.orchestrator_dock.delete_conversation(local_id);
        let Some(mut session) = self.orchestrator_dock.forget(local_id) else {
            return;
        };
        if let Some(bridge) = session.shutdown() {
            self.orchestrator_dock.stop_bridge(bridge);
        }
    }
}
