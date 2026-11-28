//! The window side of the dock: the frame that draws it, the events that it
//! reads, and the actions that start and stop the bridge of a conversation.
//!
//! The frame takes the rectangle of the dock before any panel draws, so the
//! canvas and the minimap of the frame read it. A start and a stop go to the
//! worker of the dock and never run on this thread.
//!
//! One frame draws one conversation: the front desk, or the conversation of the
//! active workspace, as [`super::OrchestratorDock::shown`] answers. Every action
//! of the frame, such as the send and the stop, belongs to that conversation.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Rect, Stroke, Ui, pos2};
use horizon_core::agent_bridge::MAX_TEXT_BYTES;

use crate::app::block_requests::workers::WorkerAnswer;
use crate::app::settings::settings_panel_default_width;
use crate::app::util::viewport_local_rect;
use crate::app::{HorizonApp, SIDEBAR_WIDTH, TOOLBAR_HEIGHT};
use crate::theme;

use horizon_core::agent_bridge::ProposalState;
use horizon_core::workspace_list::Orchestrator;

use super::layout::{dock_fits, dock_rect, dock_width};
use super::proposal::{self, Choice};
use super::session::{Said, Session, TOO_LONG_NOTICE};
use super::worker_rows;
use super::{DOCK_PANEL_ID, FRONT_DESK, composer, empty, header, layout, list, mirror};

/// The most characters of the name of a work item in a notice of the front desk.
const WORK_ITEM_NOTICE_NAME_CHARS: usize = 40;

/// The word of the notice for a turn that ended while the dock showed it: the
/// word that [`Orchestrator::Done`] gives a turn that ended unseen.
const FINISHED_WORD: &str = "finished";

/// The notice of a press that started no work item, because no worker read the
/// folder of an item of the proposal.
const PROPOSAL_CHECK_FAILED: &str = "The work items did not start.";

/// The notice of an item whose folder was not a directory of this computer when
/// the person started the proposal.
fn folder_missing_notice(title: &str) -> String {
    format!("{title}: {}", proposal::FOLDER_MISSING)
}

/// The wake of the bridge of a conversation: the next frame of this window.
fn repaint_wake(ctx: &egui::Context) -> Arc<dyn Fn() + Send + Sync> {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}
/// The directory that the orchestrator of a conversation runs in: the directory
/// of the workspace that `local_id` names, or the folder of the front desk under
/// the Horizon home for a conversation that belongs to no workspace. A workspace
/// that names no directory runs in the home of the person.
///
/// The folder of the front desk is made on the worker of the dock, before the
/// program starts: this function only names a path.
fn program_directory(board: &horizon_core::Board, local_id: &str) -> PathBuf {
    if local_id == FRONT_DESK {
        return horizon_core::HorizonHome::resolve().root().join(FRONT_DESK);
    }
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
        // A mirrored session of another computer, while the dock shows one, with the
        // snapshot of the mirror that the pump gave this frame.
        let shown_mirror = self
            .orchestrator_dock
            .front_desk()
            .mirror()
            .cloned()
            .map(|key| (key, Arc::clone(&self.shared_terminals.found.mirror)));
        // The worker wakes this window when an outcome is ready, so a start or a
        // stop applies without input.
        self.orchestrator_dock.worker.wake_with(ui.ctx());
        // The chip of the workers names the agent programs of the settings that this
        // computer has, and the search of them reads folders: the coordination worker
        // runs it once for this run.
        self.refresh_worker_programs();
        let board = &self.board;
        let dock = &mut self.orchestrator_dock;
        // The conversation of this frame: the front desk, or the workspace of the
        // board that the dock shows.
        let local_id = dock.front_desk().key(board).to_owned();
        let form = match board.active_workspace.and_then(|id| board.workspace(id)) {
            Some(workspace) if local_id != FRONT_DESK => header::Form::Workspace(workspace),
            _ => header::Form::FrontDesk,
        };
        let shows_front_desk = matches!(form, header::Form::FrontDesk);
        // The front desk has no chip of place, so its composer has no chip at all.
        let composer_form = if shows_front_desk {
            composer::FRONT_DESK
        } else {
            composer::WORKSPACE
        };
        let mut asked = composer::Asked::Nothing;
        let mut picked = None;
        let mut pressed: Option<list::Press> = None;
        // The chip of the workers of this conversation, built before the frame draws
        // so that the head of the composer knows the value that the field takes.
        let mut workers_chip = (!shows_front_desk).then(|| dock.workers_chip(&local_id, &self.template_config.presets));
        let workers_value = workers_chip
            .as_ref()
            .map_or(super::workers_menu::NONE, super::workers_menu::WorkersChip::value)
            .to_owned();
        let mut back = false;
        // The reference rows of this frame read the state of the workspaces that the
        // front desk started, the proposals that came from a file and the rows of the
        // workers.
        let refs = list::Refs::build(board, dock, &local_id);
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
                if let Some((key, view)) = &shown_mirror {
                    mirror::render(ui, content, view, key);
                    return;
                }
                let session = dock.sessions.get(&local_id);
                let snapshot = session.map(Session::snapshot);
                // The front desk names its own job in the second line, so the
                // state of its conversation stands in the chip alone.
                let state = header::State::of(session);
                let busy = snapshot.as_ref().is_some_and(|snapshot| snapshot.busy);
                // The header and the composer are bottom and top edges of the dock,
                // so they are worked out first and the list takes the room between
                // them. A part that drew top down would move the parts under it.
                let header_height = header::height(ui, content.width(), &form, state);
                let composer = composer::parts(ui, content, dock.draft(&local_id), composer_form, &workers_value);
                let parts = layout::parts(content, header_height, composer.rect.height());
                back = header::render(ui, parts.header, &form, state);
                #[cfg(test)]
                dock.set_last_header(parts.header, &form);
                let draft = dock.drafts.entry(local_id.clone()).or_default();
                asked = composer::render(ui, &composer, draft, true, busy, composer_form, workers_chip.as_mut());
                match snapshot {
                    Some(snapshot) if !snapshot.entries.is_empty() => {
                        pressed = list::render(
                            ui,
                            parts.body,
                            &local_id,
                            &snapshot.entries,
                            list::waits_for_text(&snapshot.entries, snapshot.busy),
                            &refs,
                            &|panel| board.panel_id_by_local_id(panel).is_some(),
                        );
                    }
                    _ => picked = empty::render(ui, parts.body, empty::Form::of(shows_front_desk)),
                }
                #[cfg(test)]
                dock.set_last_frame(parts, composer);
            });
        if back {
            self.orchestrator_dock.front_desk_mut().show();
        }
        if let Some(text) = picked
            && self.orchestrator_dock.pick_example(&local_id, text)
        {
            ui.ctx()
                .memory_mut(|memory| memory.request_focus(egui::Id::unique(composer::FIELD_ID)));
        }
        self.apply_dock_presses(ui.ctx(), &local_id, pressed, workers_chip.as_ref());
        match asked {
            composer::Asked::Send => self.send_orchestrator_message(ui.ctx(), &local_id),
            composer::Asked::Stop => self.stop_orchestrator_conversation(&local_id),
            composer::Asked::Nothing => {}
        }
    }

    /// Applies what the person pressed in one frame of the dock: a row of the list,
    /// which shows a panel, a workspace or a request for a worker, and a row of the
    /// chip of the workers, which adds or removes one program.
    fn apply_dock_presses(
        &mut self,
        ctx: &egui::Context,
        local_id: &str,
        pressed: Option<list::Press>,
        workers_chip: Option<&super::workers_menu::WorkersChip>,
    ) {
        match pressed {
            // A press on the reference row of a card or of a worker brings that panel
            // into view, as the row of the sidebar does. A row whose panel left the
            // board takes no press, so no press names a panel that is gone.
            Some(list::Press::Card(panel)) => {
                if let Some(id) = self.board.panel_id_by_local_id(&panel) {
                    self.reveal_selected_panel(ctx, id);
                }
            }
            Some(list::Press::WorkItem(work_item)) => self.show_work_item(&work_item),
            Some(list::Press::Proposal { id, choice }) => self.press_proposal(ctx, &id, choice),
            Some(list::Press::WorkerAsk { id, choice }) => {
                let answer = match choice {
                    worker_rows::Choice::Start => WorkerAnswer::Start,
                    worker_rows::Choice::Always => WorkerAnswer::Always,
                    worker_rows::Choice::NotNow => WorkerAnswer::NotNow,
                };
                self.press_worker_ask(ctx, local_id, &id, answer);
            }
            None => {}
        }
        // The record of the conversation keeps the program that the person added to
        // the chip or removed from it.
        if let Some(chip) = workers_chip
            && let Some(index) = chip.toggled()
            && let Some(row) = chip.rows().get(index)
        {
            self.orchestrator_dock.toggle_worker_program(local_id, &row.program);
            self.mark_runtime_dirty();
            ctx.request_repaint();
        }
    }

    /// Asks the coordination worker which agent programs of the settings this
    /// computer has, once for this run, as the group `Agents` of the sidebar does.
    /// The answer reaches the chip of the workers of every conversation.
    fn refresh_worker_programs(&mut self) {
        if !self.orchestrator_dock.ask_worker_programs() {
            return;
        }
        let presets = self.template_config.presets.clone();
        self.browser_create_host.io.then(
            move || super::workers_menu::search(&presets),
            |app, missing| app.orchestrator_dock.adopt_worker_programs(missing),
        );
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
    pub(in crate::app) fn poll_orchestrator_dock(&mut self, ctx: &egui::Context) {
        for session in self.orchestrator_dock.sessions.values_mut() {
            session.poll();
        }
        self.orchestrator_dock.poll_worker();
        // A line of Horizon that waited for the end of a turn goes out on this pass,
        // so the program reads it as the first line of its next turn. A report of a
        // worker takes the same queue, so it goes out after the line of the wake and
        // keeps the order of the reports.
        self.orchestrator_dock.flush_system_messages();
        self.orchestrator_dock.flush_worker_reports();
        // The check of the folders of a proposal runs on the worker, and the work
        // items of a proposal start on the frame that reads its answer.
        self.apply_proposal_start(ctx);
        // The start and the end of a turn and the start and the stop of a bridge are
        // the news of the rows of the sidebar, and of the notices of the front desk.
        // This pass reads them, so the sidebar needs no frame of its own: the wake of
        // a bridge already draws this one.
        if self.orchestrator_dock.note_turn_changes() {
            self.sidebar_list.count_inbox_change();
        }
        self.note_work_item_states();
        let board = &self.board;
        self.front_desk_messages
            .retain(|work_item| board.workspace_id_by_local_id(work_item).is_some());
        // The dock shows the front desk until a workspace row or a change of the
        // active workspace shows that workspace.
        self.orchestrator_dock
            .front_desk_mut()
            .follow_active_workspace(&self.board);
        // A workspace that closes takes its conversation with it: no program of
        // the dock speaks for a workspace that the board no longer holds.
        self.forget_workspaces_the_board_left();
        // A worker whose panel left the board leaves the record of its conversation,
        // and a conversation that left takes its chip and its workers with it.
        let board = &self.board;
        self.orchestrator_dock.retain_worker_conversations(|local_id| {
            local_id == FRONT_DESK || board.workspace_id_by_local_id(local_id).is_some()
        });
        self.orchestrator_dock
            .retain_worker_panels(|panel| board.panel_id_by_local_id(panel).is_some());
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

    /// Shows the workspace of the work item `work_item`, and makes it the active
    /// one, as a press on its row of the sidebar does. A work item that the board no
    /// longer holds shows nothing.
    fn show_work_item(&mut self, work_item: &str) {
        let Some(workspace) = self.board.workspace_id_by_local_id(work_item) else {
            return;
        };
        self.board.focus_workspace(workspace);
        self.orchestrator_dock.front_desk_mut().show_workspace();
    }

    /// Applies the press of the person on the row of a proposal. A start checks the
    /// folders on the worker of the dock first; a decline decides at once.
    pub(in crate::app) fn press_proposal(&mut self, ctx: &egui::Context, id: &str, choice: Choice) {
        match choice {
            Choice::Start => {
                let Some(items) = self.orchestrator_dock.open_proposal(id) else {
                    return;
                };
                if !self.orchestrator_dock.begin_proposal_start(id, &items) {
                    // No worker took the check, so no folder was read and no work
                    // item starts. The proposal stays open and the person reads
                    // that the press started nothing.
                    self.orchestrator_dock.session_notice(FRONT_DESK, PROPOSAL_CHECK_FAILED);
                }
            }
            Choice::Decline => self.decline_proposal(id),
        }
        ctx.request_repaint();
    }

    /// Applies the answer of the check of the folders of a proposal: one workspace
    /// for each item, its folder when the check found it, and the task of the item
    /// as the first message of the orchestrator of that workspace.
    ///
    /// The active workspace and the conversation that the dock shows do not change,
    /// so a press on the row of a proposal leaves the person where they were.
    fn apply_proposal_start(&mut self, ctx: &egui::Context) {
        let Some((id, found)) = self.orchestrator_dock.take_proposal_check() else {
            return;
        };
        let Some(items) = self.orchestrator_dock.open_proposal(&id) else {
            return;
        };
        let front_desk_shown = self.orchestrator_dock.front_desk().shows(&self.board);
        let mut started: Vec<(String, String)> = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let workspace = self.board.create_workspace(&item.title);
            // A new workspace starts with the grid arrangement, and an arranged
            // workspace gives every tile the size of the preset: a card of the
            // orchestrator would then hide its own text behind `More below`, as a
            // real run showed. The panels of a work item are cards that take the
            // size of their content, so the work item starts with no arrangement,
            // as the workspace of a machine does.
            self.board.clear_workspace_layout(workspace);
            let Some(local_id) = self
                .board
                .workspace(workspace)
                .map(|workspace| workspace.local_id.clone())
            else {
                continue;
            };
            match (&item.folder, found.get(index).copied().unwrap_or(false)) {
                (Some(folder), true) => {
                    self.board
                        .set_workspace_directory(workspace, Some(std::path::Path::new(folder)));
                }
                (Some(_), false) => {
                    // The folder of a proposal is text of a program, and it is a
                    // directory only when the worker found one. The work item starts
                    // in the folder of this Horizon, and the person reads why.
                    self.orchestrator_dock
                        .session_notice(FRONT_DESK, &folder_missing_notice(&item.title));
                }
                (None, _) => {}
            }
            started.push((local_id, item.task.clone()));
        }
        for (local_id, task) in &started {
            if self.say_to_orchestrator(ctx, local_id, task) {
                continue;
            }
            tracing::warn!(%local_id, "the task of a work item of a proposal did not reach its orchestrator");
        }
        if started.is_empty() {
            return;
        }
        if !self.orchestrator_dock.proposal_state(&id, ProposalState::Started) {
            return;
        }
        self.orchestrator_dock
            .session_notice(FRONT_DESK, &proposal::started_notice(started.len()));
        let names: Vec<(String, String)> = started
            .iter()
            .map(|(local_id, task)| {
                let name = self
                    .board
                    .workspace_id_by_local_id(local_id)
                    .and_then(|workspace| self.board.workspace(workspace))
                    .map_or_else(String::new, |workspace| workspace.name.clone());
                let _ = task;
                (local_id.clone(), name)
            })
            .collect();
        for (local_id, name) in names {
            self.orchestrator_dock.work_item_row(&local_id, &name);
        }
        self.orchestrator_dock
            .front_desk_system_message(&proposal::bridge_line(&id, Choice::Start));
        // The conversation that the dock shows stays the front desk, and the active
        // workspace stays the one of the frame before the press.
        self.orchestrator_dock
            .front_desk_mut()
            .follow_active_workspace(&self.board);
        if front_desk_shown {
            self.orchestrator_dock.front_desk_mut().show();
        }
        self.mark_runtime_dirty();
    }

    /// Applies a press on `Not now`: the proposal is not started, the person reads
    /// it, and the program of the front desk reads the same line.
    fn decline_proposal(&mut self, id: &str) {
        if !self.orchestrator_dock.proposal_state(id, ProposalState::Declined) {
            return;
        }
        self.orchestrator_dock
            .session_notice(FRONT_DESK, proposal::NOT_STARTED_NOTICE);
        self.orchestrator_dock
            .front_desk_system_message(&proposal::bridge_line(id, Choice::Decline));
        self.mark_runtime_dirty();
    }

    /// Writes one notice in the conversation of the front desk for each work item
    /// that goes into a state that the person waits for. Horizon writes the notice:
    /// it is no turn of a program.
    fn note_work_item_states(&mut self) {
        let work_items = self.orchestrator_dock.work_items();
        let board = &self.board;
        self.orchestrator_dock
            .retain_work_items(|work_item| board.workspace_id_by_local_id(work_item).is_some());
        for (work_item, name) in work_items {
            let Some(workspace) = board.workspace_id_by_local_id(&work_item) else {
                continue;
            };
            let state = self.workspace_orchestrator(workspace);
            let Some(before) = self.orchestrator_dock.observe_work_item(&work_item, state) else {
                continue;
            };
            // A turn that ends while the dock shows its conversation leaves no word
            // on the row, because the person saw it end. The front desk did not see
            // it, so the end of that turn is still one `finished` notice there.
            let seen_end = before == Orchestrator::Working && state == Orchestrator::Quiet;
            let word = match state.notice_word() {
                Some(word) => word,
                None if seen_end => FINISHED_WORD,
                None => continue,
            };
            let name: String = name.chars().take(WORK_ITEM_NOTICE_NAME_CHARS).collect();
            self.orchestrator_dock
                .session_notice(FRONT_DESK, &format!("{name} {word}"));
        }
    }

    /// Forgets every conversation whose workspace the board no longer holds: its
    /// program ends on the worker, and its stored conversation leaves the session
    /// folder. A workspace leaves the board when its last panel closes, and the
    /// dock of that workspace has no control left to stop it with.
    ///
    /// The front desk belongs to no workspace, so no board ever leaves it: its
    /// reserved key is never one of the keys that this pass forgets.
    fn forget_workspaces_the_board_left(&mut self) {
        let gone: Vec<String> = self
            .orchestrator_dock
            .sessions
            .keys()
            .filter(|local_id| {
                local_id.as_str() != FRONT_DESK && self.board.workspace_id_by_local_id(local_id).is_none()
            })
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
        self.say_to_orchestrator_marked(ctx, local_id, text, false)
    }

    /// The same send, with the mark of the front desk on the message: the front desk
    /// passes a message of the person to the orchestrator of a work item, and the
    /// bubble of that message carries the caption `via the front desk`.
    pub(in crate::app) fn say_to_orchestrator_marked(
        &mut self,
        ctx: &egui::Context,
        local_id: &str,
        text: &str,
        via_front_desk: bool,
    ) -> bool {
        if !super::OrchestratorDock::is_conversation(&self.board, local_id) || self.orchestrator_dock.busy(local_id) {
            return false;
        }
        if text.len() > MAX_TEXT_BYTES {
            // The program would refuse that text, so the dock begins no turn: the
            // field keeps the writing of the person, and one notice names why.
            self.orchestrator_dock.session_mut(local_id).notice(TOO_LONG_NOTICE);
            return false;
        }
        let directory = program_directory(&self.board, local_id);
        let said = self
            .orchestrator_dock
            .session_mut(local_id)
            .say_via(text, via_front_desk);
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
