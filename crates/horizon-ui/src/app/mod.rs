mod actions;
mod attention_feed;
mod bootstrap;
mod browser_requests;
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
mod remote_environments;
mod remote_hosts;
mod root_chrome;
mod root_viewport;
mod session;
mod session_manager;
mod settings;
pub(crate) mod shortcut_inventory;
pub(crate) mod shortcuts;
mod sidebar;
pub(crate) mod speech;
mod ssh_upload;
mod startup_session;
#[cfg(test)]
mod test_support;
mod updates;
pub(crate) mod util;
mod view;
mod workspace;
mod yaml_highlight;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use egui::{Color32, Pos2, Rect, Vec2, ViewportId};
use horizon_core::{
    AgentSessionCatalog, AppShortcuts, AppearanceTheme, Board, CanvasViewState, Config, GitWatcher, ManagedInstall,
    PanelId, PresetConfig, RemoteHostCatalog, ResolvedSession, RuntimeState, SessionLease, SessionStore,
    ShortcutBinding, ShutdownProgress, StartupChooser, WindowConfig, WorkspaceId,
};

use self::browser_requests::BrowserCreateHostState;
use self::canvas::CanvasGridCache;
use super::command_palette::CommandPalette;
use super::command_registry::CommandEntry;
use super::dir_picker::DirPicker;
use super::editor_widget::MarkdownPreviewCache;
use super::input;
use super::primary_selection::PrimarySelection;
use super::remote_hosts_overlay::RemoteHostsOverlay;
use super::search_overlay::SearchOverlay;
use super::terminal_widget::{TerminalGridCache, TerminalSelectionDragState};
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
use self::panels::ArrangedPanelDrag;
use self::session::{StartupBootstrapFailure, StartupBootstrapOutcome};
use self::session_manager::RuntimeSessionManagerState;
use self::settings::SettingsEditor;
use self::updates::{AvailableUpdate, UpdateCheckMessage};

struct ActiveSession {
    session_id: String,
    lease: Option<SessionLease>,
    last_lease_refresh: Option<Instant>,
    persistent: bool,
}

struct PendingSessionSwitch {
    shutdown_progress: ShutdownProgress,
    /// `None` after a timed-out browser teardown aborts the switch. The
    /// progress remains as a profile-lock guard until the driver exits.
    target: Option<ResolvedSession>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HeldSpeechBinding {
    binding: ShortcutBinding,
    release_deadline: Option<Instant>,
}

impl HeldSpeechBinding {
    const fn new(binding: ShortcutBinding) -> Self {
        Self {
            binding,
            release_deadline: None,
        }
    }
}

/// Transient dictation feedback shown bottom-center: outcomes that would
/// otherwise be invisible (ignored presses, empty transcripts, errors).
struct SpeechNotice {
    message: String,
    error: bool,
    shown_at: Instant,
}

/// Per-panel UI caches that survive across frames.
#[derive(Default)]
pub struct PanelRenderCaches {
    pub(crate) terminal_grid_cache: HashMap<PanelId, TerminalGridCache>,
    pub(crate) browser_ui_state: HashMap<PanelId, crate::browser_widget::BrowserUiState>,
    pub(crate) editor_preview_cache: HashMap<PanelId, MarkdownPreviewCache>,
}

#[allow(clippy::struct_excessive_bools)]
pub struct HorizonApp {
    board: Board,
    panels_to_close: Vec<PanelId>,
    panels_to_restart: Vec<PanelId>,
    workspace_assignments: Vec<(PanelId, WorkspaceId)>,
    workspace_creates: Vec<PanelId>,
    appearance_theme: AppearanceTheme,
    resolved_theme: theme::ResolvedTheme,
    theme_applied: bool,
    canvas_view: CanvasViewState,
    pan_target: Option<Vec2>,
    is_panning: bool,
    middle_pan_active: bool,
    canvas_pan_input_claimed: bool,
    arranged_panel_drag: Option<ArrangedPanelDrag>,
    pending_space_pan_key: CanvasPanSpaceKeyState,
    observed_keyboard_inputs: input::ObservedKeyboardInputs,
    ime_commit_normalizer: input::ImeCommitNormalizer,
    frame_keyboard_events: HashMap<ViewportId, Vec<input::FrameKeyEvent>>,
    terminal_keyboard_events: Vec<input::TerminalInputEvent>,
    speech: Option<speech::SpeechSystem>,
    speech_global_hotkeys: Option<horizon_cursor::GlobalHotkeys>,
    speech_global_hotkeys_tried: bool,
    speech_global_hotkeys_suspended: bool,
    speech_global_events_pending: Vec<horizon_cursor::HotkeyEvent>,
    /// Every push-to-talk chord currently held (used to keep their key
    /// events, repeats, and releases out of the terminal input stream —
    /// multiple profile keys can be down simultaneously).
    speech_held_bindings: Vec<HeldSpeechBinding>,
    /// The profile whose chord press started the active hold-mode recording;
    /// releases only stop a recording when set (a no-op press must not stop a
    /// mic-button recording).
    speech_engaged_profile: Option<usize>,
    /// Escape was consumed to cancel a recording this frame.
    speech_escape_cancelled: bool,
    /// The cancel-Escape's release has not yet been seen; it must also be
    /// kept out of the terminal stream (kitty release reporting).
    speech_escape_release_pending: bool,
    speech_escape_release_deadline: Option<Instant>,
    speech_notice: Option<SpeechNotice>,
    /// Whether any Horizon viewport (root or detached) reported focus this
    /// frame; evaluated at end of frame to cancel unattended recordings.
    any_viewport_focused: bool,
    panel_screen_rects: HashMap<PanelId, Rect>,
    terminal_body_screen_rects: HashMap<PanelId, Rect>,
    panel_screen_order: Vec<PanelId>,
    panel_render_order: Vec<(PanelId, usize)>,
    workspace_colors: Vec<(WorkspaceId, Color32)>,
    primary_selection: PrimarySelection,
    terminal_selection_drag: TerminalSelectionDragState,
    panel_render_caches: PanelRenderCaches,
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
    startup_receiver: Option<Receiver<StartupBootstrapOutcome>>,
    pending_startup_runtime_state: Option<RuntimeState>,
    pending_startup_runtime_state_changed: bool,
    startup_bootstrap_failure: Option<StartupBootstrapFailure>,
    session_catalog_refresh: Option<Receiver<horizon_core::Result<AgentSessionCatalog>>>,
    remote_hosts_overlay: Option<RemoteHostsOverlay>,
    remote_hosts_catalog: RemoteHostCatalog,
    remote_hosts_refresh_rx: Option<Receiver<horizon_core::Result<RemoteHostCatalog>>>,
    remote_hosts_refresh_in_flight: bool,
    remote_hosts_last_refresh: Option<Instant>,
    last_session_catalog_refresh: Option<Instant>,
    last_panel_output_at: Option<Instant>,
    browser_create_host: BrowserCreateHostState,
    settings: Option<SettingsEditor>,
    speech_model_info_cache: settings::SpeechModelInfoCache,
    session_manager: Option<RuntimeSessionManagerState>,
    remote_environments: remote_environments::RemoteEnvironments,
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
    startup_workspace_organization_pending: bool,
    startup_selection_restored: bool,
    initial_pan_done: bool,
    root_viewport_stabilizer: Option<root_viewport::RootViewportStabilizer>,
    file_hover_positions: HashMap<ViewportId, Pos2>,
    file_drop_highlight: Option<file_drop::FileDropHighlight>,
    ssh_upload_flow: Option<ssh_upload::SshUploadFlow>,
    ssh_upload_destinations: HashMap<String, String>,
    git_watchers: HashMap<WorkspaceId, GitWatcher>,
    config_last_mtime: Option<std::time::SystemTime>,
    config_last_check: Option<Instant>,
    shutdown_progress: Option<ShutdownProgress>,
    pending_session_switch: Option<PendingSessionSwitch>,
    exit_cleanup_complete: bool,
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        let now = Instant::now();
        self.frame_stats.record_frame(now);
        if let Some(delay) = self.frame_stats.idle_refresh_after(now) {
            ctx.request_repaint_after(delay);
        }
        self.exit_on_close_request(ctx);

        if self.shutdown_progress.is_some() {
            self.render_shutdown_overlay(ui);
            self.poll_shutdown_progress();
            return;
        }

        if self.poll_session_switch(ctx) {
            self.refresh_active_session_lease();
            self.render_session_switch_overlay(ui);
            return;
        }

        if !self.prepare_frame(ui) {
            self.poll_speech_runtime(ctx, Vec::new());
            self.render_speech_notice(ctx);
            return;
        }

        if self.startup_chooser.is_some() {
            self.poll_speech_runtime(ctx, Vec::new());
            self.render_startup_chooser(ui);
            self.render_speech_notice(ctx);
            return;
        }

        let inventory_interaction = self.render_remote_environments(ctx);
        let block_root_interaction = self.root_viewport_stabilization_blocks_interaction();
        let root_viewport_is_stable = self.poll_root_viewport_stabilizer(ctx);
        if block_root_interaction {
            self.suppress_root_viewport_interaction(ctx);
        }

        let (workspace_count_before, panel_count_before) = (self.board.workspaces.len(), self.board.panels.len());
        let had_panel_output = self.process_frame_inputs(ctx, inventory_interaction.is_some());
        self.apply_panel_transitions();
        self.normalize_workspace_state(ctx);
        self.apply_pending_workspace_changes();
        // The restored board must be normalized and include queued changes
        // before its one-shot layout and initial viewport are finalized.
        if root_viewport_is_stable {
            let organization_was_requested = self.startup_workspace_organization_pending;
            let aligned_leftmost_workspace = self.apply_startup_workspace_organization(ctx);
            if !self.initial_pan_done {
                let preserve_restored_selection = organization_was_requested && self.startup_selection_restored;
                self.seed_initial_pan(ctx, aligned_leftmost_workspace, preserve_restored_selection);
            }
        }
        self.render_active_view(ui, block_root_interaction || inventory_interaction.is_some());
        if block_root_interaction {
            Self::render_root_viewport_stabilizing_overlay(ctx);
        }
        self.render_speech_notice(ctx);
        self.finalize_frame(ctx, had_panel_output, workspace_count_before, panel_count_before);
        // Last, after every phase that can move or hide a panel this frame.
        self.restamp_browser_manifests_for_placement();
        if let Some(input) = inventory_interaction {
            Self::restore_remote_environment_input(ctx, input);
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::bg_for(self.resolved_theme).to_normalized_gamma_f32()
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let viewport_id = raw_input.viewport_id;
        self.ime_commit_normalizer.normalize(viewport_id, &mut raw_input.events);
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
