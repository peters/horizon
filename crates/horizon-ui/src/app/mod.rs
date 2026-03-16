mod actions;
mod attention_feed;
mod canvas;
mod panels;
mod persistence;
mod session;
mod settings;
mod sidebar;
mod util;
mod workspace;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use egui::{Context, Pos2, Rect, Vec2};
use horizon_core::{
    AgentSessionBinding, AgentSessionCatalog, Board, Config, GitWatcher, PanelId, PanelKind, PresetConfig,
    RuntimeState, WindowConfig, WorkspaceId, transcript_root_path_for_config,
};

use crate::dir_picker::DirPicker;
use crate::quick_nav::QuickNav;
use crate::theme;

const TOOLBAR_HEIGHT: f32 = 46.0;
const SIDEBAR_WIDTH: f32 = 210.0;
const PANEL_TITLEBAR_HEIGHT: f32 = 34.0;
const PANEL_PADDING: f32 = 8.0;
const PANEL_MIN_SIZE: [f32; 2] = [320.0, 220.0];
#[cfg(test)]
const PANEL_COLUMN_SPACING: f32 = 540.0;
#[cfg(test)]
const PANEL_ROW_SPACING: f32 = 360.0;
const RESIZE_HANDLE_SIZE: f32 = 18.0;
const WS_BG_PAD: f32 = 16.0;
const WS_TITLE_HEIGHT: f32 = 38.0;
const WS_EMPTY_SIZE: [f32; 2] = [304.0, 154.0];
const WS_LABEL_HEIGHT: f32 = 30.0;
const WS_LABEL_MIN_WIDTH: f32 = 110.0;
const WS_LABEL_MAX_WIDTH: f32 = 260.0;
const MINIMAP_MAX_W: f32 = 200.0;
const MINIMAP_MAX_H: f32 = 140.0;
const MINIMAP_MARGIN: f32 = 16.0;
const MINIMAP_PAD: f32 = 6.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RenameEditAction {
    #[default]
    None,
    Commit,
    Cancel,
}

enum SettingsStatus {
    None,
    LivePreview,
    Saved,
    Error(String),
}

struct SettingsEditor {
    buffer: String,
    original: String,
    status: SettingsStatus,
}

struct StartupBootstrap {
    runtime_state: RuntimeState,
    session_catalog: AgentSessionCatalog,
}

#[allow(clippy::struct_excessive_bools)]
pub struct HorizonApp {
    board: Board,
    panels_to_close: Vec<PanelId>,
    panels_to_restart: Vec<PanelId>,
    workspace_assignments: Vec<(PanelId, WorkspaceId)>,
    workspace_creates: Vec<PanelId>,
    theme_applied: bool,
    pan_offset: Vec2,
    pan_target: Option<Vec2>,
    is_panning: bool,
    panel_screen_rects: HashMap<PanelId, Rect>,
    workspace_screen_rects: Vec<(WorkspaceId, Rect)>,
    fullscreen_panel: Option<PanelId>,
    sidebar_visible: bool,
    minimap_visible: bool,
    hud_visible: bool,
    renaming_workspace: Option<WorkspaceId>,
    rename_buffer: String,
    renaming_panel: Option<PanelId>,
    panel_rename_buffer: String,
    config_path: PathBuf,
    runtime_state_path: PathBuf,
    transcript_root: Option<PathBuf>,
    template_config: Config,
    presets: Vec<PresetConfig>,
    window_config: WindowConfig,
    session_catalog: AgentSessionCatalog,
    startup_receiver: Option<Receiver<StartupBootstrap>>,
    session_catalog_refresh: Option<Receiver<horizon_core::Result<AgentSessionCatalog>>>,
    last_session_catalog_refresh: Option<Instant>,
    pending_session_rebinds: Vec<(PanelId, AgentSessionBinding)>,
    settings: Option<SettingsEditor>,
    pending_preset_pick: Option<(Option<WorkspaceId>, [f32; 2], std::time::Instant)>,
    dir_picker: Option<DirPicker>,
    quick_nav: Option<QuickNav>,
    runtime_dirty_since: Option<Instant>,
    initial_pan_done: bool,
    file_hover_pos: Option<Pos2>,
    git_watchers: HashMap<WorkspaceId, GitWatcher>,
}

impl HorizonApp {
    #[allow(clippy::needless_pass_by_value)]
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: &Config,
        config_path: PathBuf,
        runtime_state_path: PathBuf,
        runtime_state: RuntimeState,
    ) -> Self {
        let mut fonts = egui::FontDefinitions::default();

        fonts.font_data.insert(
            "inter".to_owned(),
            egui::FontData::from_static(include_bytes!("../../assets/fonts/InterVariable.ttf")).into(),
        );
        fonts.font_data.insert(
            "jetbrains-mono".to_owned(),
            egui::FontData::from_static(include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf")).into(),
        );

        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "inter".to_owned());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(0, "jetbrains-mono".to_owned());

        cc.egui_ctx.set_fonts(fonts);
        let transcript_root = transcript_root_path_for_config(&config_path);
        let startup_receiver = Self::runtime_state_needs_session_bootstrap(&runtime_state)
            .then(|| Self::spawn_startup_bootstrap(runtime_state.clone()));
        let board = if startup_receiver.is_some() {
            Board::new()
        } else {
            Board::from_runtime_state_with_transcripts(&runtime_state, transcript_root.as_deref()).unwrap_or_else(
                |error| {
                    tracing::error!("failed to restore runtime state: {error}");
                    Board::new()
                },
            )
        };

        Self {
            board,
            panels_to_close: Vec::new(),
            panels_to_restart: Vec::new(),
            workspace_assignments: Vec::new(),
            workspace_creates: Vec::new(),
            theme_applied: false,
            panel_screen_rects: HashMap::new(),
            workspace_screen_rects: Vec::new(),
            fullscreen_panel: None,
            sidebar_visible: true,
            minimap_visible: true,
            hud_visible: false,
            renaming_workspace: None,
            rename_buffer: String::new(),
            renaming_panel: None,
            panel_rename_buffer: String::new(),
            config_path,
            runtime_state_path,
            transcript_root,
            template_config: config.clone(),
            presets: config.presets.clone(),
            window_config: runtime_state.window_or(&config.window).clone(),
            session_catalog: AgentSessionCatalog::default(),
            startup_receiver,
            session_catalog_refresh: None,
            last_session_catalog_refresh: None,
            pending_session_rebinds: Vec::new(),
            settings: None,
            pending_preset_pick: None,
            dir_picker: None,
            quick_nav: None,
            runtime_dirty_since: Some(
                Instant::now()
                    .checked_sub(Duration::from_secs(1))
                    .unwrap_or_else(Instant::now),
            ),
            initial_pan_done: false,
            file_hover_pos: None,
            pan_offset: runtime_state
                .pan_offset
                .map_or(Vec2::ZERO, |offset| Vec2::new(offset[0], offset[1])),
            pan_target: None,
            is_panning: false,
            git_watchers: HashMap::new(),
        }
    }
}

impl eframe::App for HorizonApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        if !self.prepare_frame(ctx) {
            return;
        }

        let (workspace_count_before, panel_count_before) = (self.board.workspaces.len(), self.board.panels.len());
        let had_terminal_output = self.process_frame_inputs(ctx);
        self.apply_panel_transitions();
        self.normalize_workspace_state();
        self.apply_pending_workspace_changes();
        self.render_active_view(ctx);
        self.finalize_frame(ctx, had_terminal_output, workspace_count_before, panel_count_before);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self) {
        self.auto_save_runtime_state();
        self.board.shutdown_agent_panels();
    }
}

impl HorizonApp {
    fn prepare_frame(&mut self, ctx: &Context) -> bool {
        if !self.theme_applied {
            theme::apply(ctx);
            self.theme_applied = true;
        }

        if !self.poll_startup_bootstrap() {
            self.render_loading_view(ctx);
            ctx.request_repaint_after(Duration::from_millis(16));
            return false;
        }

        if !self.initial_pan_done {
            self.seed_initial_pan(ctx);
        }

        true
    }

    fn seed_initial_pan(&mut self, ctx: &Context) {
        self.initial_pan_done = true;
        if let Some(workspace_id) = self.leftmost_workspace_id() {
            self.board.focus_workspace(workspace_id);
            if let Some((min, max)) = self.board.workspace_bounds(workspace_id) {
                let pos = Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT);
                let size = Vec2::new(
                    max[0] - min[0] + 2.0 * WS_BG_PAD,
                    max[1] - min[1] + 2.0 * WS_BG_PAD + WS_TITLE_HEIGHT,
                );
                let canvas_rect = Self::canvas_rect(ctx, self.sidebar_visible);
                self.pan_offset = Vec2::new(40.0 - pos.x, canvas_rect.height() * 0.5 - (pos.y + size.y * 0.5));
            }
        }
    }

    fn process_frame_inputs(&mut self, ctx: &Context) -> bool {
        self.handle_fullscreen_toggle(ctx);
        self.handle_shortcuts(ctx);
        self.handle_file_drop(ctx);
        let had_terminal_output = self.board.process_output();

        for panel_id in self.board.exited_panels() {
            self.panels_to_close.push(panel_id);
        }

        self.animate_pan(ctx);
        self.maybe_refresh_session_catalog();
        self.poll_git_watchers();

        had_terminal_output
    }

    fn poll_git_watchers(&mut self) {
        // Collect which workspaces need watchers (have GitChanges panels).
        let mut workspaces_needing_watchers: HashMap<WorkspaceId, Option<std::path::PathBuf>> = HashMap::new();
        for panel in &self.board.panels {
            if panel.kind == PanelKind::GitChanges {
                let cwd = panel
                    .launch_cwd
                    .clone()
                    .or_else(|| self.board.workspace(panel.workspace_id).and_then(|ws| ws.cwd.clone()));
                workspaces_needing_watchers.entry(panel.workspace_id).or_insert(cwd);
            }
        }

        // Start watchers for workspaces that need them.
        for (workspace_id, cwd) in &workspaces_needing_watchers {
            if !self.git_watchers.contains_key(workspace_id)
                && let Some(path) = cwd
            {
                tracing::info!(workspace = workspace_id.0, path = %path.display(), "starting git watcher");
                self.git_watchers.insert(*workspace_id, GitWatcher::start(path.clone()));
            }
        }

        // Poll existing watchers and push updates to panels.
        let updates: Vec<(WorkspaceId, std::sync::Arc<horizon_core::GitStatus>)> = self
            .git_watchers
            .iter()
            .filter_map(|(ws_id, watcher)| watcher.try_recv().map(|status| (*ws_id, status)))
            .collect();

        for (workspace_id, status) in updates {
            for panel in &mut self.board.panels {
                if panel.workspace_id == workspace_id
                    && panel.kind == PanelKind::GitChanges
                    && let Some(viewer) = panel.content.git_changes_mut()
                {
                    viewer.update(std::sync::Arc::clone(&status));
                }
            }
        }

        // Remove watchers for workspaces that no longer have GitChanges panels.
        self.git_watchers
            .retain(|ws_id, _| workspaces_needing_watchers.contains_key(ws_id));
    }

    fn apply_panel_transitions(&mut self) {
        let panels_to_close = std::mem::take(&mut self.panels_to_close);
        for panel_id in panels_to_close {
            self.close_panel(panel_id);
            self.panel_screen_rects.remove(&panel_id);
            if self.renaming_panel == Some(panel_id) {
                self.clear_panel_rename();
            }
        }
        let panels_to_restart = std::mem::take(&mut self.panels_to_restart);
        for panel_id in panels_to_restart {
            if let Err(error) = self.board.restart_panel(panel_id) {
                tracing::error!(panel_id = panel_id.0, %error, "failed to restart panel");
            }
        }
    }

    fn normalize_workspace_state(&mut self) {
        self.board.remove_empty_workspaces();
        if self.board.workspaces.is_empty() {
            self.reset_view();
        }
        if self
            .renaming_workspace
            .is_some_and(|workspace_id| self.board.workspace(workspace_id).is_none())
        {
            self.clear_workspace_rename();
        }
        if self
            .renaming_panel
            .is_some_and(|panel_id| self.board.panel(panel_id).is_none())
        {
            self.clear_panel_rename();
        }
    }

    fn apply_pending_workspace_changes(&mut self) {
        for panel_id in self.workspace_creates.drain(..) {
            let name = format!("Workspace {}", self.board.workspaces.len() + 1);
            let workspace_id = self.board.create_workspace(&name);
            self.board.assign_panel_to_workspace(panel_id, workspace_id);
        }
        for (panel_id, workspace_id) in self.workspace_assignments.drain(..) {
            self.board.assign_panel_to_workspace(panel_id, workspace_id);
        }
        self.apply_pending_session_rebinds();
    }

    fn render_active_view(&mut self, ctx: &Context) {
        if self.fullscreen_panel.is_some() {
            self.render_fullscreen_panel(ctx);
        } else if self.settings.is_some() {
            self.render_toolbar(ctx);
            self.render_sidebar(ctx);
            self.render_settings(ctx);
        } else {
            self.handle_canvas_pan(ctx);
            self.render_toolbar(ctx);
            self.render_sidebar(ctx);
            self.render_canvas(ctx);
            self.render_workspace_backgrounds(ctx);
            self.handle_canvas_double_click(ctx);
            self.render_panels(ctx);
            self.render_preset_picker(ctx);
            self.render_minimap(ctx);
            let minimap_height = if self.minimap_visible && !self.board.workspaces.is_empty() {
                MINIMAP_MAX_H + MINIMAP_PAD * 2.0
            } else {
                0.0
            };
            attention_feed::render_attention_feed(ctx, &self.board, minimap_height);
            self.render_canvas_hud(ctx);
        }
    }

    fn finalize_frame(
        &mut self,
        ctx: &Context,
        had_terminal_output: bool,
        workspace_count_before: usize,
        panel_count_before: usize,
    ) {
        self.render_dir_picker(ctx);
        self.render_quick_nav(ctx);
        self.sync_window_config(ctx);

        if self.board.workspaces.len() != workspace_count_before || self.board.panels.len() != panel_count_before {
            self.auto_save_runtime_state();
        }
        self.flush_runtime_if_dirty();

        let has_live_terminals = !self.board.panels.is_empty();
        let animating = self.pan_target.is_some();
        if animating {
            ctx.request_repaint();
        } else if has_live_terminals {
            // Poll for new PTY output. When a terminal just produced output
            // we check again soon (16ms) to keep up with streaming content.
            // When idle we back off to 100ms to save CPU — egui still
            // repaints instantly on any user interaction (mouse, keyboard).
            let poll = if had_terminal_output {
                Duration::from_millis(16)
            } else {
                Duration::from_millis(100)
            };
            ctx.request_repaint_after(poll);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PANEL_MIN_SIZE;
    use super::util::{clamp_panel_size, default_panel_canvas_pos, format_grid_position};
    use egui::{Pos2, Vec2};

    #[test]
    fn default_panel_positions_tile_in_rows() {
        assert_eq!(default_panel_canvas_pos(0), Pos2::new(120.0, 120.0));
        assert_eq!(default_panel_canvas_pos(1), Pos2::new(660.0, 120.0));
        assert_eq!(default_panel_canvas_pos(3), Pos2::new(120.0, 480.0));
    }

    #[test]
    fn panel_size_is_clamped_to_minimums() {
        let clamped = clamp_panel_size(Vec2::new(100.0, 120.0));

        assert!((clamped.x - PANEL_MIN_SIZE[0]).abs() <= f32::EPSILON);
        assert!(clamped.y >= PANEL_MIN_SIZE[1]);
    }

    #[test]
    fn grid_positions_are_rounded_for_display() {
        assert_eq!(format_grid_position(Pos2::new(12.4, -7.6)), "12, -8");
        assert_eq!(format_grid_position(Pos2::new(-3.5, 2.5)), "-4, 3");
    }
}
