mod actions;
mod attention_feed;
mod canvas;
mod detached_viewports;
mod lifecycle;
mod panel_chrome;
mod panels;
mod persistence;
mod session;
mod settings;
mod sidebar;
mod startup_session;
mod util;
mod view;
mod workspace;
mod yaml_highlight;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use egui::{Context, Pos2, Rect, Vec2};
use horizon_core::{
    AgentSessionBinding, AgentSessionCatalog, Board, CanvasViewState, Config, GitWatcher, PanelId, PresetConfig,
    ResolvedSession, RuntimeState, SessionLease, SessionStore, ShutdownProgress, StartupChooser, StartupDecision,
    WindowConfig, WorkspaceId,
};

use crate::app::canvas::CanvasGridCache;
use crate::dir_picker::DirPicker;
use crate::quick_nav::QuickNav;
use crate::terminal_widget::TerminalGridCache;
use crate::theme;

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
    canvas_view: CanvasViewState,
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
    detached_workspaces: BTreeMap<String, WindowConfig>,
    pending_detached_reattach: BTreeSet<String>,
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
            detached_workspaces: BTreeMap::new(),
            pending_detached_reattach: BTreeSet::new(),
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
            canvas_view: CanvasViewState::default(),
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
