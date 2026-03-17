mod actions;
mod attention_feed;
mod canvas;
mod panels;
mod persistence;
mod session;
mod settings;
mod sidebar;
mod startup_session;
mod util;
mod workspace;
mod yaml_highlight;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use egui::{Context, Pos2, Rect, Vec2};
use horizon_core::{
    AgentSessionBinding, AgentSessionCatalog, Board, Config, GitWatcher, PanelId, PanelKind, PresetConfig,
    ResolvedSession, RuntimeState, SessionLease, SessionStore, ShutdownProgress, StartupChooser, StartupDecision,
    WindowConfig, WorkspaceId,
};

use crate::app::canvas::CanvasGridCache;
use crate::dir_picker::DirPicker;
use crate::quick_nav::QuickNav;
use crate::terminal_widget::TerminalGridCache;
use crate::{loading_spinner, theme};

const TOOLBAR_HEIGHT: f32 = 46.0;
const SIDEBAR_WIDTH: f32 = 210.0;
const PANEL_TITLEBAR_HEIGHT: f32 = 34.0;
const PANEL_PADDING: f32 = 8.0;
const PANEL_MIN_SIZE: [f32; 2] = [320.0, 220.0];
const RESIZE_HANDLE_SIZE: f32 = 18.0;
const WS_BG_PAD: f32 = 16.0;
const WS_TITLE_HEIGHT: f32 = 38.0;
const WS_EMPTY_SIZE: [f32; 2] = [304.0, 154.0];
const WS_LABEL_HEIGHT: f32 = 30.0;
const WS_LABEL_MIN_WIDTH: f32 = 110.0;
const WS_LABEL_MAX_WIDTH: f32 = 260.0;
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

struct ActiveSession {
    session_id: String,
    lease: Option<SessionLease>,
    last_lease_refresh: Option<Instant>,
    persistent: bool,
}

struct StartupChooserState {
    chooser: StartupChooser,
    selected_session_id: Option<String>,
    error: Option<String>,
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
    terminal_grid_cache: HashMap<PanelId, TerminalGridCache>,
    canvas_grid_cache: CanvasGridCache,
    workspace_screen_rects: Vec<(WorkspaceId, Rect)>,
    fullscreen_panel: Option<PanelId>,
    sidebar_visible: bool,
    minimap_visible: bool,
    hud_visible: bool,
    renaming_workspace: Option<WorkspaceId>,
    rename_buffer: String,
    renaming_panel: Option<PanelId>,
    panel_rename_buffer: String,
    session_store: SessionStore,
    active_session: Option<ActiveSession>,
    startup_chooser: Option<StartupChooserState>,
    config_path: PathBuf,
    transcript_root: Option<PathBuf>,
    template_config: Config,
    presets: Vec<PresetConfig>,
    window_config: WindowConfig,
    session_catalog: AgentSessionCatalog,
    startup_receiver: Option<Receiver<StartupBootstrap>>,
    session_catalog_refresh: Option<Receiver<horizon_core::Result<AgentSessionCatalog>>>,
    last_session_catalog_refresh: Option<Instant>,
    last_terminal_output_at: Option<Instant>,
    pending_session_rebinds: Vec<(PanelId, AgentSessionBinding)>,
    settings: Option<SettingsEditor>,
    pending_preset_pick: Option<(Option<WorkspaceId>, [f32; 2], std::time::Instant)>,
    dir_picker: Option<DirPicker>,
    quick_nav: Option<QuickNav>,
    runtime_dirty_since: Option<Instant>,
    initial_pan_done: bool,
    file_hover_pos: Option<Pos2>,
    git_watchers: HashMap<WorkspaceId, GitWatcher>,
    config_last_mtime: Option<std::time::SystemTime>,
    config_last_check: Option<Instant>,
    shutdown_progress: Option<ShutdownProgress>,
    exit_cleanup_complete: bool,
}

impl HorizonApp {
    #[allow(clippy::needless_pass_by_value)]
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: &Config,
        config_path: PathBuf,
        session_store: SessionStore,
        startup: StartupDecision,
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
        let mut board = Board::new();
        board.attention_enabled = config.features.attention_feed;

        let config_last_mtime = std::fs::metadata(&config_path).ok().and_then(|m| m.modified().ok());

        let mut app = Self {
            board,
            panels_to_close: Vec::new(),
            panels_to_restart: Vec::new(),
            workspace_assignments: Vec::new(),
            workspace_creates: Vec::new(),
            theme_applied: false,
            panel_screen_rects: HashMap::new(),
            terminal_grid_cache: HashMap::new(),
            canvas_grid_cache: CanvasGridCache::default(),
            workspace_screen_rects: Vec::new(),
            fullscreen_panel: None,
            sidebar_visible: true,
            minimap_visible: true,
            hud_visible: false,
            renaming_workspace: None,
            rename_buffer: String::new(),
            renaming_panel: None,
            panel_rename_buffer: String::new(),
            session_store,
            active_session: None,
            startup_chooser: None,
            config_path,
            transcript_root: None,
            template_config: config.clone(),
            presets: config.presets.clone(),
            window_config: config.window.clone(),
            session_catalog: AgentSessionCatalog::default(),
            startup_receiver: None,
            session_catalog_refresh: None,
            last_session_catalog_refresh: None,
            last_terminal_output_at: Some(Instant::now()),
            pending_session_rebinds: Vec::new(),
            settings: None,
            pending_preset_pick: None,
            dir_picker: None,
            quick_nav: None,
            runtime_dirty_since: None,
            initial_pan_done: false,
            file_hover_pos: None,
            pan_offset: Vec2::ZERO,
            pan_target: None,
            is_panning: false,
            git_watchers: HashMap::new(),
            config_last_mtime,
            config_last_check: None,
            shutdown_progress: None,
            exit_cleanup_complete: false,
        };

        match startup {
            StartupDecision::Open { session, .. } => app.activate_persistent_session(&session),
            StartupDecision::Ephemeral { runtime_state } => app.activate_ephemeral_session(&runtime_state),
            StartupDecision::Choose(chooser) => app.startup_chooser = Some(StartupChooserState::new(chooser)),
        }

        app
    }
}

impl eframe::App for HorizonApp {
    #[profiling::function]
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.exit_on_close_request(ctx);

        if self.shutdown_progress.is_some() {
            self.render_shutdown_overlay(ctx);
            self.poll_shutdown_progress();
            return;
        }

        if !self.prepare_frame(ctx) {
            return;
        }

        if self.startup_chooser.is_some() {
            self.render_startup_chooser(ctx);
            return;
        }

        let (workspace_count_before, panel_count_before) = (self.board.workspaces.len(), self.board.panels.len());
        let had_terminal_output = self.process_frame_inputs(ctx);
        self.apply_panel_transitions();
        self.normalize_workspace_state(ctx);
        self.apply_pending_workspace_changes();
        self.render_active_view(ctx);
        self.finalize_frame(ctx, had_terminal_output, workspace_count_before, panel_count_before);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::BG.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self) {
        self.run_exit_cleanup();
        // macOS can leave Horizon running as a windowless app after eframe
        // has already torn down the viewport, so terminate explicitly.
        std::process::exit(0);
    }
}

impl HorizonApp {
    #[profiling::function]
    fn exit_on_close_request(&mut self, ctx: &Context) {
        if !ctx.input(|input| input.viewport().close_requested()) {
            return;
        }

        // Keep the viewport alive while we flush state and stop PTY-backed panels.
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.begin_shutdown();
    }

    /// Starts asynchronous terminal shutdown. State is saved immediately,
    /// and background threads join each terminal event loop. The UI shows a
    /// progress overlay until all terminals are done or the budget expires.
    #[profiling::function]
    fn begin_shutdown(&mut self) {
        if self.shutdown_progress.is_some() {
            return;
        }

        self.auto_save_runtime_state();
        self.git_watchers.clear();
        self.shutdown_progress = Some(self.board.begin_async_shutdown());
    }

    #[profiling::function]
    fn poll_shutdown_progress(&mut self) {
        const MAX_SHUTDOWN_WAIT: Duration = Duration::from_secs(3);

        let Some(progress) = &self.shutdown_progress else {
            return;
        };

        if progress.is_complete() || progress.started_at().elapsed() > MAX_SHUTDOWN_WAIT {
            self.exit_cleanup_complete = true;
            self.release_active_session_lease();
            std::process::exit(0);
        }
    }

    #[profiling::function]
    fn render_shutdown_overlay(&self, ctx: &Context) {
        let Some(progress) = &self.shutdown_progress else {
            return;
        };
        let completed = progress.terminals_completed();
        let total = progress.terminal_count();

        egui::CentralPanel::default().show(ctx, |ui| {
            if total > 0 {
                loading_spinner::show_with_detail(
                    ui,
                    egui::Id::new("shutdown_spinner"),
                    "Closing Horizon\u{2026}",
                    &format!("{completed} / {total} terminals shut down"),
                );
            } else {
                loading_spinner::show(ui, egui::Id::new("shutdown_spinner"), Some("Closing Horizon\u{2026}"));
            }
        });
    }

    /// Synchronous fallback for the `on_exit` eframe callback.
    #[profiling::function]
    fn run_exit_cleanup(&mut self) {
        if self.exit_cleanup_complete {
            return;
        }

        self.exit_cleanup_complete = true;
        self.auto_save_runtime_state();
        self.board.shutdown_terminal_panels();
        self.git_watchers.clear();
        self.release_active_session_lease();
    }

    #[profiling::function]
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

        if self.startup_chooser.is_none() && !self.initial_pan_done {
            self.seed_initial_pan(ctx);
        }

        true
    }

    #[profiling::function]
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

    #[profiling::function]
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
        self.poll_config_reload();

        had_terminal_output
    }

    #[profiling::function]
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

    #[profiling::function]
    fn poll_config_reload(&mut self) {
        // Skip while settings editor is open (it manages its own save/reload).
        if self.settings.is_some() {
            return;
        }

        // Check at most every 2 seconds.
        let now = Instant::now();
        if self
            .config_last_check
            .is_some_and(|t| now.duration_since(t) < Duration::from_secs(2))
        {
            return;
        }
        self.config_last_check = Some(now);

        let current_mtime = std::fs::metadata(&self.config_path)
            .ok()
            .and_then(|m| m.modified().ok());

        if current_mtime == self.config_last_mtime {
            return;
        }
        self.config_last_mtime = current_mtime;

        if let Ok(config) = Config::load(Some(&self.config_path)) {
            tracing::info!("config file changed, reloading presets");
            self.template_config = config.clone();
            self.presets.clone_from(&config.presets);
            self.board.sync_workspace_metadata(&config);
        }
    }

    #[profiling::function]
    fn apply_panel_transitions(&mut self) {
        let panels_to_close = std::mem::take(&mut self.panels_to_close);
        for panel_id in panels_to_close {
            self.close_panel(panel_id);
            self.panel_screen_rects.remove(&panel_id);
            self.terminal_grid_cache.remove(&panel_id);
            if self.renaming_panel == Some(panel_id) {
                self.clear_panel_rename();
            }
        }
        let panels_to_restart = std::mem::take(&mut self.panels_to_restart);
        for panel_id in panels_to_restart {
            if let Err(error) = self.board.restart_panel(panel_id) {
                tracing::error!(panel_id = panel_id.0, %error, "failed to restart panel");
            } else {
                self.terminal_grid_cache.remove(&panel_id);
            }
        }
    }

    #[profiling::function]
    fn normalize_workspace_state(&mut self, ctx: &Context) {
        let count_before = self.board.workspaces.len();
        self.board.remove_empty_workspaces();
        let count_after = self.board.workspaces.len();
        if self.board.workspaces.is_empty() {
            self.reset_view();
        } else if count_after < count_before && count_after == 1 {
            let workspace_id = self.board.workspaces[0].id;
            self.board.focus_workspace(workspace_id);
            if let Some((min, max)) = self.board.workspace_bounds(workspace_id) {
                let pos = Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT);
                let size = Vec2::new(
                    max[0] - min[0] + 2.0 * WS_BG_PAD,
                    max[1] - min[1] + 2.0 * WS_BG_PAD + WS_TITLE_HEIGHT,
                );
                self.pan_to_canvas_pos_aligned(ctx, pos, size, true);
            }
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

    #[profiling::function]
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

    #[profiling::function]
    fn render_active_view(&mut self, ctx: &Context) {
        if self.fullscreen_panel.is_some() {
            self.render_fullscreen_panel(ctx);
            return;
        }

        // Settings side panel renders first so egui reserves the space
        // before the canvas `CentralPanel` claims the remainder.
        if self.settings.is_some() {
            self.render_settings(ctx);
        }

        let workspace_bounds = self.board.workspace_bounds_map();
        self.handle_canvas_pan(ctx);
        self.render_toolbar(ctx);
        self.render_sidebar(ctx);
        self.render_canvas(ctx);
        let overlay_zones = self.overlay_exclusion_zones(ctx);
        self.render_workspace_backgrounds(ctx, &workspace_bounds, &overlay_zones);
        self.handle_canvas_double_click(ctx);
        self.render_panels(ctx);
        self.render_preset_picker(ctx);
        let minimap_height = self.render_minimap(ctx, &workspace_bounds);
        if self.template_config.features.attention_feed {
            let feed_result =
                attention_feed::render_attention_feed(ctx, &self.board, minimap_height, &self.template_config.overlays);
            for attention_id in feed_result.dismissed_ids {
                let _ = self.board.dismiss_attention(attention_id);
            }
            if let Some(panel_id) = feed_result.focus_panel {
                self.board.focus(panel_id);
                if let Some(ws_id) = self.board.panel(panel_id).map(|p| p.workspace_id)
                    && let Some((min, max)) = self.board.workspace_bounds(ws_id)
                {
                    let pos = egui::Pos2::new(min[0] - WS_BG_PAD, min[1] - WS_BG_PAD - WS_TITLE_HEIGHT);
                    let size = Vec2::new(
                        max[0] - min[0] + 2.0 * WS_BG_PAD,
                        max[1] - min[1] + 2.0 * WS_BG_PAD + WS_TITLE_HEIGHT,
                    );
                    self.pan_to_canvas_pos_aligned(ctx, pos, size, true);
                }
            }
        }
        self.render_canvas_hud(ctx);
    }

    #[profiling::function]
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
        self.refresh_active_session_lease();

        if self.board.workspaces.len() != workspace_count_before || self.board.panels.len() != panel_count_before {
            self.auto_save_runtime_state();
        }
        self.flush_runtime_if_dirty();

        let has_live_terminals = !self.board.panels.is_empty();
        let animating = self.pan_target.is_some();
        if animating {
            ctx.request_repaint();
        } else if has_live_terminals {
            // Keep streaming terminals responsive, but progressively back off
            // once the board has been quiet for a while to reduce idle CPU.
            let now = Instant::now();
            let poll = if had_terminal_output {
                self.last_terminal_output_at = Some(now);
                Duration::from_millis(16)
            } else {
                let idle_for = self
                    .last_terminal_output_at
                    .map_or(Duration::MAX, |last_output| now.saturating_duration_since(last_output));

                if idle_for < Duration::from_secs(1) {
                    Duration::from_millis(100)
                } else if idle_for < Duration::from_secs(5) {
                    Duration::from_millis(250)
                } else if idle_for < Duration::from_secs(30) {
                    Duration::from_millis(500)
                } else {
                    Duration::from_secs(1)
                }
            };
            ctx.request_repaint_after(poll);
        }
    }
}

impl StartupChooserState {
    fn new(chooser: StartupChooser) -> Self {
        let selected_session_id = chooser.sessions.first().map(|session| session.session_id.clone());
        Self {
            chooser,
            selected_session_id,
            error: None,
        }
    }
}
