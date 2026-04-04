mod actions;
mod attention_feed;
mod canvas;
mod detached_viewports;
mod file_drop;
mod file_drop_highlight;
mod frame_stats;
mod lifecycle;
mod minimap;
mod panel_chrome;
mod panels;
mod persistence;
mod remote_hosts;
mod root_chrome;
mod session;
mod session_manager;
mod settings;
pub(crate) mod shortcuts;
mod sidebar;
mod ssh_upload;
mod startup_session;
mod updates;
pub(crate) mod util;
mod view;
mod workspace;
mod yaml_highlight;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use egui::{Context, Pos2, Rect, Vec2, ViewportId};
use horizon_core::{
    AgentSessionBinding, AgentSessionCatalog, AppShortcuts, Board, CanvasViewState, Config, GitWatcher, ManagedInstall,
    PanelId, PresetConfig, RemoteHostCatalog, ResolvedSession, RuntimeState, SessionLease, SessionStore,
    ShutdownProgress, StartupChooser, StartupDecision, WindowConfig, WorkspaceId,
};

use self::canvas::CanvasGridCache;
use super::command_palette::CommandPalette;
use super::command_registry::CommandEntry;
use super::dir_picker::DirPicker;
use super::editor_widget::MarkdownPreviewCache;
use super::input;
use super::primary_selection::PrimarySelection;
use super::remote_hosts_overlay::RemoteHostsOverlay;
use super::search_overlay::SearchOverlay;
use super::terminal_widget::TerminalGridCache;
use super::theme;

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
const FONT_INTER: &str = "inter";
const FONT_JETBRAINS_MONO: &str = "jetbrains-mono";
const FONT_NOTO_CJK: &str = "noto-sans-cjk-sc";
const FONT_NOTO_SYMBOLS: &str = "noto-sans-symbols-2";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RenameEditAction {
    #[default]
    None,
    Commit,
    Cancel,
}

#[derive(Clone, Default)]
enum CanvasPanSpaceKeyState {
    #[default]
    Idle,
    Pending(Vec<input::TerminalInputEvent>),
    Consumed,
}

use self::frame_stats::FrameStats;
use self::session_manager::RuntimeSessionManagerState;
use self::settings::SettingsEditor;
use self::updates::{AvailableUpdate, UpdateCheckMessage};

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

#[derive(Clone, Default)]
struct DetachedCanvasInteractionState {
    is_panning: bool,
    middle_pan_active: bool,
    canvas_pan_input_claimed: bool,
    pending_space_pan_key: CanvasPanSpaceKeyState,
}

#[derive(Clone, Default)]
struct DetachedWorkspaceViewportState {
    window: WindowConfig,
    canvas_view: CanvasViewState,
    pan_target: Option<Vec2>,
    interaction: DetachedCanvasInteractionState,
    initial_fit_pending: bool,
    panel_screen_rects: HashMap<PanelId, Rect>,
    terminal_body_screen_rects: HashMap<PanelId, Rect>,
    panel_screen_order: Vec<PanelId>,
}

impl DetachedWorkspaceViewportState {
    fn new(window: WindowConfig) -> Self {
        Self {
            window,
            canvas_view: CanvasViewState::default(),
            pan_target: None,
            interaction: DetachedCanvasInteractionState::default(),
            initial_fit_pending: true,
            panel_screen_rects: HashMap::new(),
            terminal_body_screen_rects: HashMap::new(),
            panel_screen_order: Vec::new(),
        }
    }
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
    middle_pan_active: bool,
    canvas_pan_input_claimed: bool,
    pending_space_pan_key: CanvasPanSpaceKeyState,
    observed_keyboard_inputs: input::ObservedKeyboardInputs,
    frame_keyboard_events: HashMap<ViewportId, Vec<input::FrameKeyEvent>>,
    terminal_keyboard_events: Vec<input::TerminalInputEvent>,
    panel_screen_rects: HashMap<PanelId, Rect>,
    terminal_body_screen_rects: HashMap<PanelId, Rect>,
    panel_screen_order: Vec<PanelId>,
    primary_selection: PrimarySelection,
    terminal_grid_cache: HashMap<PanelId, TerminalGridCache>,
    editor_preview_cache: HashMap<PanelId, MarkdownPreviewCache>,
    canvas_grid_cache: CanvasGridCache,
    frame_stats: FrameStats,
    workspace_screen_rects: Vec<(WorkspaceId, Rect)>,
    fullscreen_panel: Option<PanelId>,
    sidebar_visible: bool,
    sidebar_drag_workspace: Option<WorkspaceId>,
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
    shortcuts: AppShortcuts,
    presets: Vec<PresetConfig>,
    window_config: WindowConfig,
    detached_workspaces: BTreeMap<String, DetachedWorkspaceViewportState>,
    pending_detached_reattach: BTreeSet<String>,
    pending_detached_window_position_restore: BTreeSet<String>,
    session_catalog: AgentSessionCatalog,
    startup_receiver: Option<Receiver<StartupBootstrap>>,
    session_catalog_refresh: Option<Receiver<horizon_core::Result<AgentSessionCatalog>>>,
    remote_hosts_overlay: Option<RemoteHostsOverlay>,
    remote_hosts_catalog: RemoteHostCatalog,
    remote_hosts_refresh_rx: Option<Receiver<horizon_core::Result<RemoteHostCatalog>>>,
    remote_hosts_refresh_in_flight: bool,
    remote_hosts_last_refresh: Option<Instant>,
    last_session_catalog_refresh: Option<Instant>,
    last_terminal_output_at: Option<Instant>,
    pending_session_rebinds: Vec<(PanelId, AgentSessionBinding)>,
    settings: Option<SettingsEditor>,
    session_manager: Option<RuntimeSessionManagerState>,
    managed_install: Option<ManagedInstall>,
    surge_update_check_rx: Option<Receiver<UpdateCheckMessage>>,
    surge_available_update: Option<AvailableUpdate>,
    next_surge_update_check_at: Option<Instant>,
    pending_preset_pick: Option<(Option<WorkspaceId>, [f32; 2], std::time::Instant)>,
    dir_picker: Option<DirPicker>,
    command_palette: Option<CommandPalette>,
    search_overlay: Option<SearchOverlay>,
    action_commands_cache: Vec<CommandEntry>,
    runtime_dirty_since: Option<Instant>,
    initial_pan_done: bool,
    file_hover_positions: HashMap<ViewportId, Pos2>,
    file_drop_highlight: Option<file_drop::FileDropHighlight>,
    ssh_upload_flow: Option<ssh_upload::SshUploadFlow>,
    ssh_upload_destinations: HashMap<String, String>,
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
        observed_keyboard_inputs: input::ObservedKeyboardInputs,
    ) -> Self {
        let shortcuts = resolve_shortcuts(config);
        let action_commands_cache =
            super::command_registry::action_commands(&shortcuts, self::util::primary_shortcut_label());
        cc.egui_ctx.set_fonts(configure_fonts());
        let mut board = Board::new();
        board.attention_enabled = config.features.attention_feed;

        let config_last_mtime = std::fs::metadata(&config_path).ok().and_then(|m| m.modified().ok());
        let (managed_install, next_surge_update_check_at) = managed_install_state();

        let mut app = Self {
            board,
            panels_to_close: Vec::new(),
            panels_to_restart: Vec::new(),
            workspace_assignments: Vec::new(),
            workspace_creates: Vec::new(),
            theme_applied: false,
            panel_screen_rects: HashMap::new(),
            panel_screen_order: Vec::new(),
            terminal_grid_cache: HashMap::new(),
            editor_preview_cache: HashMap::new(),
            canvas_grid_cache: CanvasGridCache::default(),
            frame_stats: FrameStats::default(),
            workspace_screen_rects: Vec::new(),
            fullscreen_panel: None,
            sidebar_visible: true,
            sidebar_drag_workspace: None,
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
            shortcuts,
            presets: config.resolved_presets(),
            window_config: config.window.clone(),
            detached_workspaces: BTreeMap::new(),
            pending_detached_reattach: BTreeSet::new(),
            pending_detached_window_position_restore: BTreeSet::new(),
            session_catalog: AgentSessionCatalog::default(),
            startup_receiver: None,
            session_catalog_refresh: None,
            remote_hosts_overlay: None,
            remote_hosts_catalog: RemoteHostCatalog::default(),
            remote_hosts_refresh_rx: None,
            remote_hosts_refresh_in_flight: false,
            remote_hosts_last_refresh: None,
            last_session_catalog_refresh: None,
            last_terminal_output_at: Some(Instant::now()),
            pending_session_rebinds: Vec::new(),
            settings: None,
            session_manager: None,
            managed_install,
            surge_update_check_rx: None,
            surge_available_update: None,
            next_surge_update_check_at,
            pending_preset_pick: None,
            dir_picker: None,
            command_palette: None,
            search_overlay: None,
            action_commands_cache,
            runtime_dirty_since: None,
            initial_pan_done: false,
            file_hover_positions: HashMap::new(),
            file_drop_highlight: None,
            ssh_upload_flow: None,
            ssh_upload_destinations: HashMap::new(),
            canvas_view: CanvasViewState::default(),
            pan_target: None,
            is_panning: false,
            middle_pan_active: false,
            canvas_pan_input_claimed: false,
            pending_space_pan_key: CanvasPanSpaceKeyState::Idle,
            observed_keyboard_inputs,
            frame_keyboard_events: HashMap::new(),
            terminal_keyboard_events: Vec::new(),
            git_watchers: HashMap::new(),
            terminal_body_screen_rects: HashMap::new(),
            primary_selection: PrimarySelection::new(),
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

        app.maybe_start_update_check();

        app
    }
}

fn managed_install_state() -> (Option<ManagedInstall>, Option<Instant>) {
    let managed_install = std::env::current_exe()
        .ok()
        .and_then(|current_exe| ManagedInstall::discover(&current_exe));
    let next_surge_update_check_at = managed_install
        .as_ref()
        .filter(|install| install.uses_stable_channel() && install.uses_github_releases())
        .map(|_| Instant::now());
    (managed_install, next_surge_update_check_at)
}

fn configure_fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();

    insert_font_data(
        &mut fonts,
        FONT_INTER,
        include_bytes!("../../assets/fonts/InterVariable.ttf"),
    );
    insert_font_data(
        &mut fonts,
        FONT_JETBRAINS_MONO,
        include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf"),
    );
    // Keep JetBrains Mono as the metrics source for the terminal grid, then
    // fall back to broader Unicode coverage for glyphs it does not contain.
    insert_font_data(
        &mut fonts,
        FONT_NOTO_CJK,
        include_bytes!("../../assets/fonts/NotoSansCJKsc-Regular.otf"),
    );
    insert_font_data(
        &mut fonts,
        FONT_NOTO_SYMBOLS,
        include_bytes!("../../assets/fonts/NotoSansSymbols2-Regular.ttf"),
    );

    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, FONT_INTER.to_owned());

    let monospace = fonts.families.entry(egui::FontFamily::Monospace).or_default();
    monospace.insert(0, FONT_JETBRAINS_MONO.to_owned());
    monospace.insert(1, FONT_NOTO_CJK.to_owned());
    monospace.insert(2, FONT_NOTO_SYMBOLS.to_owned());

    fonts
}

fn insert_font_data(fonts: &mut egui::FontDefinitions, name: &str, bytes: &'static [u8]) {
    fonts
        .font_data
        .insert(name.to_owned(), egui::FontData::from_static(bytes).into());
}

fn resolve_shortcuts(config: &Config) -> AppShortcuts {
    match config.shortcuts.resolve() {
        Ok(shortcuts) => shortcuts,
        Err(error) => {
            tracing::error!("invalid shortcut config loaded at runtime: {error}");
            AppShortcuts::default()
        }
    }
}

impl eframe::App for HorizonApp {
    #[profiling::function]
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        self.frame_stats.record_frame(now);
        if let Some(delay) = self.frame_stats.idle_refresh_after(now) {
            ctx.request_repaint_after(delay);
        }
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

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let viewport_id = raw_input.viewport_id;
        let frame_keyboard_events = self.observed_keyboard_inputs.take_frame_key_events(raw_input);
        if frame_keyboard_events.is_empty() {
            self.frame_keyboard_events.remove(&viewport_id);
        } else {
            self.frame_keyboard_events.insert(viewport_id, frame_keyboard_events);
        }
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

#[cfg(test)]
mod tests {
    use egui::FontFamily;

    use super::{FONT_INTER, FONT_JETBRAINS_MONO, FONT_NOTO_CJK, FONT_NOTO_SYMBOLS, configure_fonts};

    #[test]
    fn configure_fonts_registers_terminal_fallback_stack() {
        let fonts = configure_fonts();
        let proportional = fonts
            .families
            .get(&FontFamily::Proportional)
            .expect("proportional font family");
        let monospace = fonts
            .families
            .get(&FontFamily::Monospace)
            .expect("monospace font family");

        assert_eq!(proportional.first().map(String::as_str), Some(FONT_INTER));
        assert_eq!(monospace.first().map(String::as_str), Some(FONT_JETBRAINS_MONO));
        assert_eq!(monospace.get(1).map(String::as_str), Some(FONT_NOTO_CJK));
        assert_eq!(monospace.get(2).map(String::as_str), Some(FONT_NOTO_SYMBOLS));
        assert!(fonts.font_data.contains_key(FONT_NOTO_CJK));
        assert!(fonts.font_data.contains_key(FONT_NOTO_SYMBOLS));
    }
}
