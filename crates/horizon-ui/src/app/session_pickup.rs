//! **Pick up where you left off**: the sessions that the agent programs of this
//! computer already keep.
//!
//! The first start page shows a section of these rows under its tiles, and the
//! Menu of the titlebar opens the full list in a window. Both read one list that
//! `horizon_core::session_pickup` builds on a worker: the stores of the agent
//! programs are files of other programs, so a frame never reads, stats or parses
//! one of them.
//!
//! A press of `Open` makes the workspace of the folder of the session, when the
//! board has none, and opens the panel of the preset of that program with the
//! working directory of the session and the resume of exactly that session. The
//! panel then holds that session id as its binding from the first frame, which is
//! the rule of `docs/session-recovery.md`.
//!
//! A row that another program has open now opens only after a second
//! confirmation: two programs in one conversation is a known hazard.

pub(in crate::app) mod rows;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Context, RichText, Vec2};
use horizon_core::{AgentSessionKey, PanelKind, PickupFolder, PickupList, PickupProgram, PickupRow};

use self::rows::{PickupPress, ROW_GAP, ROW_HEIGHT, folder_header_height};
use super::HorizonApp;
use crate::theme;

/// The words that open the section of the first start page.
pub(in crate::app) const SECTION_TITLE: &str = "Pick up where you left off";
/// The words of the control that opens the full list from the first start page.
pub(in crate::app) const SHOW_ALL: &str = "Show all";
/// The title of the window that holds the full list.
pub(in crate::app) const WINDOW_TITLE: &str = "Recent agent sessions";
/// The row of the Menu of the titlebar that opens the full list.
pub(in crate::app) const MENU_TITLE: &str = "Recent agent sessions";
/// The line of the Menu row.
pub(in crate::app) const MENU_DETAIL: &str = "Open a conversation that an agent already has on this computer";
/// The line of the window when the stores answer nothing.
pub(in crate::app) const EMPTY_LINE: &str =
    "No conversation of an agent program is on this computer. Open an agent panel, and its session stands here.";

/// What one read of the session stores needs: the home folder that holds them,
/// the programs of the settings, and the sessions that an open panel already has.
pub(in crate::app) struct PickupRequest {
    /// The home folder of this computer, whose stores the pickup reads.
    pub home: std::path::PathBuf,
    /// The agent programs of the presets of the settings.
    pub programs: Vec<PickupProgram>,
    /// The sessions of the open panels of this board, which the pickup leaves out.
    pub open_panels: HashSet<AgentSessionKey>,
}

/// The pickup of this window: its list, the read that fills it, and the window
/// that shows it.
#[derive(Default)]
pub(in crate::app) struct SessionPickup {
    list: PickupList,
    /// The read that a worker runs, so no frame reads a session store.
    pending: Option<Receiver<PickupList>>,
    /// Whether this window asked for its one read at the start.
    asked: bool,
    /// Whether the window of the full list shows.
    window_open: bool,
    /// The row that waits for the second confirmation because another program
    /// has its session open now.
    confirm: Option<AgentSessionKey>,
    /// Why a press opened nothing, in the words of the window.
    notice: Option<String>,
}

#[cfg(test)]
impl SessionPickup {
    /// A pickup that already holds `list`, so a test shows rows without a read of
    /// the stores of this computer.
    pub(super) fn with_list(list: PickupList) -> Self {
        Self {
            list,
            asked: true,
            ..Self::default()
        }
    }
}

impl SessionPickup {
    /// Whether the window of the full list shows.
    pub(in crate::app) fn window_open(&self) -> bool {
        self.window_open
    }

    /// The rows of the first start page, or an empty list when this window knows
    /// of no session.
    pub(in crate::app) fn first_page(&self) -> Vec<PickupFolder> {
        self.list.first_page()
    }

    /// Whether the row of `kind` and `session_id` waits for a second confirmation.
    pub(in crate::app) fn is_confirmed(&self, kind: PanelKind, session_id: &str) -> bool {
        self.confirm
            .as_ref()
            .is_some_and(|key| *key == AgentSessionKey::new(kind, session_id))
    }

    /// Starts the read of the stores, unless one runs.
    fn ask(&mut self, request: PickupRequest) -> bool {
        if self.pending.is_some() {
            return false;
        }
        self.asked = true;
        let PickupRequest {
            home,
            programs,
            open_panels,
        } = request;
        let now = horizon_core::current_unix_millis();
        let (sender, receiver) = std::sync::mpsc::channel();
        // The stores of the agent programs are files of other programs, and a
        // person's home is private to them: this read runs on its own worker.
        let spawned = std::thread::Builder::new()
            .name("agent-session-pickup".to_string())
            .spawn(move || {
                let list = horizon_core::session_pickup::load(&home, &programs, &open_panels, now);
                let _ = sender.send(list);
            });
        match spawned {
            Ok(_) => {
                self.pending = Some(receiver);
                true
            }
            Err(error) => {
                tracing::warn!("the read of the agent sessions did not start: {error}");
                false
            }
        }
    }

    /// Applies the answer of the worker, and answers whether the pickup changed.
    fn poll(&mut self) -> bool {
        let Some(receiver) = self.pending.as_ref() else {
            return false;
        };
        match receiver.try_recv() {
            Ok(list) => {
                self.pending = None;
                self.list = list;
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                tracing::warn!("the read of the agent sessions ended without an answer");
                false
            }
        }
    }
}

impl HorizonApp {
    /// Reads the sessions of the agent programs once per window, and applies the
    /// answer of the worker.
    pub(in crate::app) fn poll_session_pickup(&mut self, ctx: &Context) -> bool {
        if !self.session_pickup.asked && self.request_session_pickup() {
            ctx.request_repaint();
        }
        if self.session_pickup.pending.is_none() {
            return false;
        }
        // The answer arrives off the frame: keep the frames coming while it runs.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        self.session_pickup.poll()
    }

    /// Asks a worker for a read of the session stores of this computer.
    ///
    /// A read runs alone: a press that asks again while one runs keeps the
    /// answer of the read that is already there.
    fn request_session_pickup(&mut self) -> bool {
        let request = PickupRequest {
            home: self.session_store.home().root().to_path_buf(),
            programs: PickupProgram::from_presets(&self.presets),
            open_panels: self
                .board
                .panels
                .iter()
                .filter_map(|panel| {
                    horizon_core::Panel::session_id(panel).map(|id| AgentSessionKey::new(panel.kind, id))
                })
                .collect(),
        };
        self.session_pickup.ask(request)
    }

    /// Opens the window of the full list, with a fresh read of the stores.
    pub(in crate::app) fn show_recent_agent_sessions(&mut self) {
        self.session_pickup.window_open = true;
        self.session_pickup.notice = None;
        self.session_pickup.confirm = None;
        self.request_session_pickup();
    }

    /// Closes the window of the full list.
    pub(in crate::app) fn close_recent_agent_sessions(&mut self) {
        self.session_pickup.window_open = false;
        self.session_pickup.confirm = None;
    }

    /// Draws the window of the full list, the rows grouped by folder.
    pub(in crate::app) fn render_recent_agent_sessions(&mut self, ctx: &Context) {
        if !self.session_pickup.window_open {
            return;
        }
        let id = egui::Id::unique("recent_agent_sessions");
        let mut painted = None;
        let mut close = false;
        let shown = egui::Modal::new(id)
            .area(egui::Modal::default_area(id).order(egui::Order::Tooltip))
            .frame(
                egui::Frame::popup(&ctx.global_style())
                    .corner_radius(16.0)
                    .inner_margin(18.0)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER_STRONG())),
            )
            .show(ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 120.0).clamp(360.0, 620.0));
                ui.label(RichText::new(WINDOW_TITLE).size(20.0).strong().color(theme::FG()));
                let folders = self.session_pickup.list.folders().to_vec();
                ui.label(
                    RichText::new(summary_line(&self.session_pickup.list, &folders))
                        .size(12.0)
                        .color(theme::FG_SOFT()),
                );
                ui.add_space(12.0);
                if let Some(notice) = self.session_pickup.notice.clone() {
                    ui.label(RichText::new(notice).size(12.0).color(theme::PALETTE_RED()));
                    ui.add_space(8.0);
                }
                if folders.is_empty() {
                    ui.label(RichText::new(EMPTY_LINE).size(12.5).color(theme::FG_SOFT()));
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                    return;
                }
                let height = window_body_height(&folders);
                egui::ScrollArea::vertical()
                    .id_salt("recent_agent_sessions_scroll")
                    .max_height(height.min((ctx.content_rect().height() - 220.0).max(120.0)))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        painted = self.paint_pickup_groups(ui, &folders);
                    });
                ui.add_space(10.0);
                ui.label(
                    RichText::new("A session that another program has open asks once more before it opens here.")
                        .size(11.5)
                        .color(theme::FG_SOFT()),
                );
            });
        let press = painted;
        if shown.should_close() {
            close = true;
        }
        if let Some(press) = press {
            self.apply_pickup_press(ctx, &press);
        }
        if close {
            self.close_recent_agent_sessions();
        }
    }

    /// Draws every folder of `folders` with its rows, and answers the press of the
    /// person.
    fn paint_pickup_groups(&self, ui: &mut egui::Ui, folders: &[PickupFolder]) -> Option<PickupPress> {
        for (folder_index, folder) in folders.iter().enumerate() {
            let width = ui.available_width();
            let (header, _) = ui.allocate_exact_size(Vec2::new(width, folder_header_height()), egui::Sense::hover());
            if let Some(choice) = rows::paint_folder_header(ui, header, folder, folder_index) {
                return Some(choice);
            }
            for (row_index, row) in folder.rows.iter().enumerate() {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), egui::Sense::hover());
                let confirming = self.session_pickup.is_confirmed(row.kind, &row.session_id);
                if let Some(choice) = rows::paint_row(ui, rect, folder_index, row_index, row, None, confirming) {
                    return Some(choice);
                }
                ui.add_space(ROW_GAP);
            }
            ui.add_space(8.0);
        }
        None
    }

    /// Opens the rows that the person pressed.
    pub(in crate::app) fn apply_pickup_press(&mut self, ctx: &Context, press: &PickupPress) {
        let folders = self.session_pickup.list.folders().to_vec();
        match press {
            PickupPress::ShowAll => self.show_recent_agent_sessions(),
            PickupPress::Open { folder, row } => {
                let Some(folder) = folders.get(*folder) else {
                    return;
                };
                let Some(row) = folder.rows.get(*row) else {
                    return;
                };
                if !row.state.opens() {
                    self.session_pickup.notice = Some(format!(
                        "This session does not open: {}.",
                        row.state.says().unwrap_or("the state of the session is unknown")
                    ));
                    return;
                }
                if row.state.asks_first() && !self.session_pickup.is_confirmed(row.kind, &row.session_id) {
                    self.session_pickup.confirm = Some(AgentSessionKey::new(row.kind, row.session_id.clone()));
                    self.session_pickup.notice = Some(format!(
                        "{} has this conversation open now. Press Open anyway to open it here as well.",
                        row.kind.display_name()
                    ));
                    return;
                }
                let folder_path = folder.path.clone();
                let opened = self.open_agent_session(ctx, &folder_path, row);
                if opened {
                    self.session_pickup.notice = None;
                }
            }
            PickupPress::OpenAll { folder } => {
                let Some(folder) = folders.get(*folder) else {
                    return;
                };
                let folder_path = folder.path.clone();
                for row in open_all_rows(folder) {
                    self.open_agent_session(ctx, &folder_path, &row);
                }
            }
        }
    }

    /// Opens the workspace of `folder` and the panel of `row` in it.
    ///
    /// This is the one open of the pickup: the first start page, the window and
    /// the command line of a folder all come here.
    pub(in crate::app) fn open_agent_session(&mut self, ctx: &Context, folder: &Path, row: &PickupRow) -> bool {
        let Some(preset) = self.pickup_preset(row.kind) else {
            self.session_pickup.notice = Some(format!(
                "The settings hold no preset of {}. Add one in Settings, Presets.",
                row.kind.display_name()
            ));
            return false;
        };
        let options = row.panel_options(folder, &preset, &self.template_config.browser);
        let workspace_id = self.ensure_workspace_of_folder(ctx, folder);
        match self.create_panel_with_options(options, workspace_id) {
            Ok(panel_id) => {
                self.reveal_new_panel(ctx, workspace_id, panel_id);
                self.session_pickup.confirm = None;
                // The session now has an open panel, so it leaves the list.
                self.request_session_pickup();
                true
            }
            Err(error) => {
                self.session_pickup.notice = Some(format!("The panel did not open: {error}"));
                false
            }
        }
    }

    /// The preset of the settings that starts `kind`, in the order of the
    /// settings.
    fn pickup_preset(&self, kind: PanelKind) -> Option<horizon_core::PresetConfig> {
        self.presets
            .iter()
            .find(|preset| preset.kind == kind && horizon_core::session_pickup::reads_sessions(kind))
            .cloned()
    }

    /// The workspace of `folder`, made with the name of the folder when the board
    /// has none, and brought into the view.
    ///
    /// A workspace "of a folder" is a workspace whose working directory names that
    /// folder, compared by [`horizon_core::same_folder`].
    pub(in crate::app) fn ensure_workspace_of_folder(
        &mut self,
        ctx: &Context,
        folder: &Path,
    ) -> horizon_core::WorkspaceId {
        if let Some(workspace_id) = self.workspace_of_folder(folder) {
            self.board.focus_workspace(workspace_id);
            self.focus_workspace_visible(ctx, workspace_id, false);
            return workspace_id;
        }
        let name = folder_name(folder);
        let workspace_id = self.create_workspace_visible(ctx, &name);
        if let Some(workspace) = self.board.workspace_mut(workspace_id) {
            workspace.cwd = Some(folder.to_path_buf());
        }
        self.board.focus_workspace(workspace_id);
        workspace_id
    }

    /// The workspace of `folder`, when the board has one.
    #[must_use]
    pub(in crate::app) fn workspace_of_folder(&self, folder: &Path) -> Option<horizon_core::WorkspaceId> {
        self.board
            .workspaces
            .iter()
            .find(|workspace| {
                workspace
                    .cwd
                    .as_deref()
                    .is_some_and(|cwd| horizon_core::same_folder(cwd, folder))
            })
            .map(|workspace| workspace.id)
    }
}

/// The rows that `Open all` of `folder` opens.
///
/// A row that another program has open now waits for its own confirmation, and a
/// row that cannot open stays in the list: one press of a folder never opens a
/// conversation twice and never opens a row that has no program.
#[must_use]
pub(in crate::app) fn open_all_rows(folder: &PickupFolder) -> Vec<PickupRow> {
    let count = folder.open_all_count();
    folder
        .rows
        .iter()
        .filter(|row| row.state.opens() && !row.state.asks_first())
        .take(count)
        .cloned()
        .collect()
}

/// The name of the workspace of `folder`: the name of the folder itself.
#[must_use]
pub(in crate::app) fn folder_name(folder: &Path) -> String {
    folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| folder.display().to_string())
}

/// The line under the title of the window.
fn summary_line(list: &PickupList, folders: &[PickupFolder]) -> String {
    let rows = folders.iter().map(|folder| folder.rows.len()).sum::<usize>();
    let folders = folders.len();
    let sessions = if rows == 1 { "session" } else { "sessions" };
    let places = if folders == 1 { "folder" } else { "folders" };
    if list.is_empty() {
        return "Newest first".to_string();
    }
    format!("{rows} {sessions} in {folders} {places} · Newest first")
}

/// The height of the rows of the window, so a short list needs no scrollbar.
fn window_body_height(folders: &[PickupFolder]) -> f32 {
    folders
        .iter()
        .map(|folder| {
            #[allow(clippy::cast_precision_loss)]
            let rows = folder.rows.len() as f32;
            folder_header_height() + rows * (ROW_HEIGHT + ROW_GAP) + 8.0
        })
        .sum::<f32>()
        + 8.0
}
