mod actions;
mod answer_retries;
mod attention;
mod block_clicks;
mod block_links;
mod block_machine_keys;
mod block_machines;
mod block_panels;
mod block_requests;
mod block_terminals;
mod board_notice;
mod bootstrap;
mod browser_cleanup;
mod browser_close_requests;
mod browser_connectors;
mod browser_create_dispatch;
mod browser_create_failure;
mod browser_create_lifecycle;
mod browser_duplicate;
mod browser_file_chooser;
mod browser_host_io;
mod browser_kept_failures;
mod browser_manifest_stamp;
mod browser_provider_catalog;
mod browser_provider_usage;
mod browser_recovery;
mod browser_remote_create;
mod browser_remote_credentials;
mod browser_remote_slots;
mod browser_request_claims;
mod browser_requests;
mod browser_visibility_requests;
mod canvas;
mod canvas_drag;
mod canvas_gesture;
mod canvas_scroll;
mod card_fit;
mod creation_notice;
mod cwd_reads;
mod detached_viewports;
mod device_presentation;
mod device_request_pump;
mod device_requests;
mod device_reveal_wait;
pub(crate) mod device_video;
#[cfg(test)]
pub(crate) use device_video::folders_that_the_drop_queues;
pub(crate) use device_video::queue_recording_directory_delete;
mod editor_saves;
mod file_drop;
mod file_drop_highlight;
mod frame_stats;
mod keyboard_focus;
mod lifecycle;
mod machine_click;
mod machine_open;
mod minimap;
mod navigation_input;
mod notify;
mod panel_chrome;
mod panel_restarts;
mod panel_starts;
mod panels;
mod persistence;
mod press_frame;
mod reconnect;
mod remote_hosts;
mod root_chrome;
mod root_viewport;
#[cfg(test)]
mod server_join_tests;
mod server_sync;
mod session;
mod session_jobs;
mod session_kill;
mod session_manager;
mod settings;
mod settings_io;
pub(crate) mod shortcut_inventory;
pub(crate) mod shortcuts;
mod sidebar;
mod speech;
mod ssh_hosts;
mod ssh_upload;
mod startup_session;
#[cfg(test)]
mod test_support;
mod transcript_deletes;
pub(crate) mod util;
mod view;
mod work_resume;
mod workspace;
mod workspace_destination;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use egui::{Color32, Pos2, Rect, Vec2, ViewportId};
use horizon_core::{
    AgentSessionCatalog, AppShortcuts, AppearanceTheme, Board, CanvasViewState, Config, PanelId, PresetConfig,
    RemoteHostCatalog, ResolvedSession, RuntimeState, SessionLease, SessionStore, ShutdownProgress, StartupChooser,
    WindowConfig, WorkspaceId,
};

use self::browser_requests::BrowserCreateHostState;
use self::canvas::CanvasGridCache;
pub(crate) use self::device_request_pump::{BridgeApp, DeviceRequestBridge, is_device_queue_wake};
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
pub(crate) const PANEL_TITLEBAR_HEIGHT: f32 = 34.0;
pub(crate) const PANEL_PADDING: f32 = 8.0;
const PANEL_MIN_SIZE: [f32; 2] = [320.0, 220.0];
const WS_BG_PAD: f32 = 16.0;
const WS_TITLE_HEIGHT: f32 = 38.0;
const WS_EMPTY_SIZE: [f32; 2] = [304.0, 154.0];
const WS_LABEL_HEIGHT: f32 = 30.0;
const WS_LABEL_MIN_WIDTH: f32 = 110.0;
const WS_LABEL_MAX_WIDTH: f32 = 260.0;
const MINIMAP_MARGIN: f32 = 16.0;
const MINIMAP_PAD: f32 = 6.0;

/// The panels that agent panels opened for the person in this Horizon run: the
/// SSH terminals of `open_terminal` and the panels of a preset of `open_panel`.
/// Each kind has its own bound, and both hold for the whole run.
#[derive(Default)]
pub(in crate::app) struct AgentOpens {
    pub(in crate::app) terminals: block_terminals::AgentTerminals,
    pub(in crate::app) panels: block_panels::AgentPanels,
}

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
use self::settings_io::SettingsIo;

struct ActiveSession {
    session_id: String,
    lease: Option<SessionLease>,
    /// The answers of the lease writes that the session writer has not finished.
    lease_answers: Vec<Receiver<std::result::Result<(), String>>>,
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
    /// The job of the session store that runs, so the window says that it works.
    busy: Option<SessionJobLabel>,
}

/// What a job of the session store works on, so the window can say that it
/// runs and refuse a second one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum SessionJobLabel {
    List,
    Create,
    Duplicate,
    TakeOver,
    Resume,
    Delete,
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
    /// Fit the workspace into this window at the next render, as Fit Workspace does.
    pending_fit: bool,
    pending_device_reveal: Option<PanelId>,
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
            pending_fit: false,
            pending_device_reveal: None,
            panel_screen_rects: HashMap::new(),
            terminal_body_screen_rects: HashMap::new(),
            panel_screen_order: Vec::new(),
        }
    }
}

/// Per-panel UI caches that survive across frames.
#[derive(Default)]
pub struct PanelRenderCaches {
    device_request_poll: Option<Instant>,
    block_request_poll: Option<Instant>,
    pending_device_reveal: Option<device_requests::PendingDeviceReveal>,
    awaiting_device_reveals: Vec<device_reveal_wait::AwaitingDeviceReveal>,
    pub(crate) terminal_grid_cache: HashMap<PanelId, TerminalGridCache>,
    pub(crate) browser_ui_state: HashMap<PanelId, crate::browser_widget::BrowserUiState>,
    pub(crate) device_ui_state: HashMap<PanelId, crate::device_widget::DeviceUiState>,
    /// The clicks and the row selection of each card. A card that has nothing
    /// to settle drops out of the map again.
    pub(crate) card_runtime: HashMap<PanelId, horizon_core::CardRuntime>,
    pub(crate) editor_preview_cache: HashMap<PanelId, MarkdownPreviewCache>,
}

#[allow(clippy::struct_excessive_bools)]
pub struct HorizonApp {
    board: Board,
    panels_to_close: Vec<PanelId>,
    panels_to_restart: Vec<PanelId>,
    /// The panel whose body takes the keyboard in the frame that draws it, because
    /// a person created or revealed it.
    keyboard_focus: Option<PanelId>,
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
    panel_screen_rects: HashMap<PanelId, Rect>,
    terminal_body_screen_rects: HashMap<PanelId, Rect>,
    panel_screen_order: Vec<PanelId>,
    panel_render_order: Vec<(PanelId, usize)>,
    workspace_colors: Vec<(WorkspaceId, Color32)>,
    primary_selection: PrimarySelection,
    clipboard_images: crate::clipboard_image::ClipboardImages,
    terminal_selection_drag: TerminalSelectionDragState,
    panel_render_caches: PanelRenderCaches,
    /// The Device requests a claim took, and the answers the host owes for
    /// them. It lives here, not in the render caches: a session switch resets
    /// those, but the answers to the old board's requests must still be
    /// written, and an answer that is in flight must not lose its count.
    pub(in crate::app) device_claim: device_requests::claim::DeviceClaimState,
    /// The card requests a claim took, and the answers the host owes for them.
    /// It lives here, not in the render caches, for the same reason as
    /// `device_claim`: a session switch resets those.
    pub(in crate::app) block_claim: block_requests::BlockClaimState,
    /// The `next_click` waits this host holds open, and the last wake line to
    /// each agent panel. It lives here for the same reason as `block_claim`.
    pub(in crate::app) block_clicks: block_clicks::BlockClickState,
    /// The panels each agent panel opened for the person in this run, bounded per
    /// agent. It lives here for the same reason as `block_claim`.
    pub(in crate::app) agent_opens: AgentOpens,
    /// The machines that agents registered with this Horizon, read from its home
    /// on the coordination worker. The list belongs to the person, so it
    /// outlives a session and a workspace.
    pub(in crate::app) machine_list: horizon_core::Machines,
    /// Why the last action of the person opened nothing, as the sidebar says
    /// it, and when the line goes away again. A panel that cannot start and a
    /// machine action that opens nothing both use it.
    pub(in crate::app) sidebar_notice: Option<String>,
    pub(in crate::app) sidebar_notice_until: Option<Instant>,
    /// The tone of that line: a reason to act, or a plain answer to the person.
    pub(in crate::app) sidebar_notice_tone: crate::app::sidebar::machines::SidebarNoticeTone,
    /// The line that stays until the person closes it: the panels that an opened
    /// session left out, which a person who looks away at the start would never
    /// read otherwise. It has its own close control and no timer, and a short
    /// notice never replaces it.
    pub(in crate::app) sidebar_sticky_notice: Option<String>,
    /// The line about a setting of this computer that the server decided for
    /// another computer, read from the link on each frame. It stays in the machine
    /// section under the state line of the server until its close control ends it,
    /// and the Server tab shows it beside them.
    pub(in crate::app) settings_lost: Option<String>,
    /// The clock of the short notice, so a test moves it forward instead of
    /// waiting for the ten seconds of a notice.
    pub(in crate::app) notice_clock: sidebar::machines::NoticeClock,
    /// The rows of the sidebar's machine section, read when the list or a
    /// terminal of a machine changes.
    machine_rows: sidebar::machines::MachineRows,
    /// The machine terminals that wait for their next attempt after a lost
    /// link, with one wake at the due time and no thread of their own.
    reconnects: reconnect::ReconnectWaits,
    /// The browser panels that an action of the person opened, with the card
    /// link that opened each one, while an engine that cannot start can still
    /// end them.
    pub(in crate::app) person_browser_creates: creation_notice::PersonBrowserCreates,
    /// The tmux sessions that closes ended, waiting for the coordination
    /// worker. A test reads this list instead of running the jobs.
    pub(in crate::app) session_kills: Vec<horizon_core::SessionKill>,
    /// A machine that the person asked to forget, waiting for the confirm line.
    pub(in crate::app) forget_machine: Option<String>,
    /// A workspace whose panels the person asked to close, waiting for the line
    /// that says the tmux sessions of its machine end with them.
    pub(in crate::app) close_workspace_confirm: Option<WorkspaceId>,
    /// Counts the writes of the machine list, so a read asked for before a write
    /// never replaces it.
    machine_epoch: u64,
    /// The startup read of the machine list and of the link to its server, on
    /// its own thread.
    machine_load_rx: Option<Receiver<Result<server_sync::MachineLoad, horizon_core::MachineError>>>,
    /// The link of this Horizon to its server, when the settings name one.
    pub(in crate::app) server_sync: Option<server_sync::ServerSync>,
    canvas_grid_cache: CanvasGridCache,
    frame_stats: FrameStats,
    /// The callback that asks this window for the frame that shows the echo of a
    /// key press in a terminal panel.
    output_wake: horizon_core::OutputWake,
    workspace_screen_rects: Vec<(WorkspaceId, Rect)>,
    fullscreen_panel: Option<PanelId>,
    held_navigation_keys: Vec<horizon_core::ShortcutBinding>,
    sidebar_visible: bool,
    sidebar_drag_workspace: Option<WorkspaceId>,
    /// The rows of the sidebar's workspace list, read at most once a second.
    sidebar_list: sidebar::ListCache,
    /// Which agent panel needs the person, which one finished and which one
    /// works, from the work ledgers of the panels.
    attention: attention::AttentionState,
    /// The desktop notification of a change that the person did not see.
    notifications: notify::Notifications,
    minimap_visible: bool,
    hud_visible: bool,
    renaming_workspace: Option<WorkspaceId>,
    rename_buffer: String,
    renaming_panel: Option<PanelId>,
    panel_rename_buffer: String,
    pub(in crate::app) session_store: SessionStore,
    active_session: Option<ActiveSession>,
    startup_chooser: Option<StartupChooserState>,
    config_path: PathBuf,
    transcript_root: Option<PathBuf>,
    template_config: Config,
    shortcuts: AppShortcuts,
    /// The presets of the window: the settings, with the hosts of the ssh config
    /// that the worker of [`ssh_hosts`] read.
    presets: Vec<PresetConfig>,
    /// The hosts of the ssh config of this computer, read on a worker.
    ssh_hosts: ssh_hosts::SshHostsRead,
    /// Whether the open Settings window asked for its one read of the hosts.
    settings_hosts_refreshed: bool,
    window_config: WindowConfig,
    detached_workspaces: BTreeMap<String, DetachedWorkspaceViewportState>,
    pending_detached_reattach: BTreeSet<String>,
    pending_detached_window_position_restore: BTreeSet<String>,
    session_catalog: AgentSessionCatalog,
    startup_receiver: Option<Receiver<StartupBootstrapOutcome>>,
    pending_startup_runtime_state: Option<RuntimeState>,
    pending_startup_runtime_state_changed: bool,
    startup_bootstrap_failure: Option<StartupBootstrapFailure>,
    /// The live Claude conversations that the startup bootstrap thread read, so
    /// an answer of the person reuses them.
    busy_claude_session_ids: HashSet<String>,
    session_catalog_refresh: session::SessionCatalogRefreshState,
    saved_session_deletion: panels::SavedSessionDeletionState,
    remote_hosts_overlay: Option<RemoteHostsOverlay>,
    remote_hosts_catalog: RemoteHostCatalog,
    remote_hosts_refresh_rx: Option<Receiver<horizon_core::Result<RemoteHostCatalog>>>,
    remote_hosts_refresh_in_flight: bool,
    remote_hosts_last_refresh: Option<Instant>,
    last_panel_output_at: Option<Instant>,
    browser_create_host: BrowserCreateHostState,
    settings: Option<SettingsEditor>,
    /// Saves and reloads of the settings, which never run on the UI thread.
    settings_io: SettingsIo,
    /// A reload is in flight, so the window does not ask for another one.
    settings_reload_pending: bool,
    /// Session-only and file-backed provider credentials for remote browser targets.
    remote_browser_credentials: horizon_core::remote_browser_credential::CredentialWorkbench,
    /// The one-time import of the values that an older Horizon kept in the keyring of
    /// this system. It runs only on a click in the Settings tab "Remote".
    pub(in crate::app) keyring_import: horizon_core::remote_browser_credential::KeyringImport,
    /// The named secrets of this computer: the names of the settings and the values
    /// that its credential store holds, read on a worker thread.
    named_secrets: horizon_core::named_secrets::NamedSecretWorkbench,
    /// The same names and values, for the code that starts a panel and the code that
    /// follows the server. It holds no copy of a value: the values live in the
    /// published environment of this process.
    named_secret_vault: Option<std::sync::Arc<horizon_core::named_secrets::NamedSecretVault>>,
    session_manager: Option<RuntimeSessionManagerState>,
    canvas_gesture: canvas_gesture::CanvasGesture,
    press_frame: press_frame::PressFrame,
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
    /// Dropped editor files whose size read runs on the coordination worker.
    pending_editor_drops: file_drop::PendingEditorDrops,
    /// The panels whose process waits for the preparation that the coordination
    /// worker reads.
    panel_starts: panel_starts::PanelStarts,
    /// The reads of the working directories of the panels, which the
    /// coordination worker makes.
    cwd_reads: cwd_reads::CwdReads,
    /// The panels whose restart runs, and why each one restarts.
    panel_restarts: panel_restarts::PanelRestarts,
    /// The job of the session store that runs, and the selection and the
    /// sentence that its next list of sessions shows.
    session_jobs: session_jobs::SessionJobs,
    session_manager_reload: Option<(Option<String>, Option<String>)>,
    ssh_upload_flow: Option<ssh_upload::SshUploadFlow>,
    ssh_upload_destinations: HashMap<String, String>,
    config_last_mtime: Option<std::time::SystemTime>,
    /// Whether a frame heard the time of the store once, so the next report is a
    /// change and not the start.
    config_mtime_known: bool,
    /// Whether the settings worker reads the time of the store now.
    config_probe_pending: bool,
    config_last_check: Option<Instant>,
    shutdown_progress: Option<ShutdownProgress>,
    pending_session_switch: Option<PendingSessionSwitch>,
    exit_cleanup_complete: bool,
    /// When the exit stops waiting for the session writer.
    exit_save_deadline: Option<Instant>,
    /// Transcripts whose delete runs on the coordination worker, so a panel
    /// that comes back with the same local id keeps its own.
    transcript_deletes: transcript_deletes::PendingTranscriptDeletes,
    /// Speech input: the hotkey, the take that the worker thread runs, and the
    /// state mark of the panel of that take.
    speech: speech::SpeechUi,
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

impl HorizonApp {
    #[profiling::function]
    fn update_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        let now = Instant::now();
        self.frame_stats.record_frame(now, ctx.requested_repaint_last_pass());
        self.poll_named_secrets(ctx);
        self.remote_browser_credentials.poll();
        if self.remote_browser_credentials.is_busy() {
            // Worker answers arrive off-thread; keep frames coming until the
            // store has opened and every probe or write has reported.
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.keyring_import.poll();
        if self.keyring_import.is_busy() {
            // The import of the keyring runs its programs on a worker thread, so its
            // line arrives there too.
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.remote_browser_credentials
            .adopt_imported_values(self.keyring_import.take_changes());
        // The named secrets of a network of an older Horizon take the path of a value
        // that a person typed: the name enters the settings, and the link sends it.
        let created = self.keyring_import.take_created();
        self.adopt_created_secrets(created);
        self.queue_credential_secrets();
        if let Some(delay) = self.frame_stats.idle_refresh_after(now) {
            ctx.request_repaint_after(delay);
        }
        self.exit_on_close_request(ctx);
        // Also while the board is torn down, so every claimed request is
        // answered; a finished job wakes this context to apply its outcome.
        self.browser_create_host.io.wake_with(ctx);
        self.apply_browser_host_io();
        self.apply_settings_io(ctx);
        self.poll_machine_load();
        self.poll_server_sync();
        self.poll_ssh_hosts(ctx);

        if self.shutdown_progress.is_some() {
            self.render_shutdown_overlay(ui);
            self.poll_shutdown_progress();
            return;
        }

        let switching_session = self.poll_session_switch(ctx);
        // Before the session chooser and any session: a lease left by a crash.
        // Never in tests, which must not touch the real home.
        if switching_session {
            self.refresh_active_session_lease();
            self.poll_lease_answers();
            self.render_session_switch_overlay(ui);
            return;
        }

        if !self.prepare_frame(ui) {
            return;
        }

        if self.startup_chooser.is_some() {
            self.render_startup_chooser(ui);
            return;
        }

        let block_root_interaction = self.root_viewport_stabilization_blocks_interaction();
        let root_viewport_is_stable = self.poll_root_viewport_stabilizer(ctx);
        if block_root_interaction {
            self.suppress_root_viewport_interaction(ctx);
        }

        let (workspace_count_before, panel_count_before) = (self.board.workspaces.len(), self.board.panels.len());
        let had_panel_output = self.process_frame_inputs(ctx);
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
        self.render_active_view(ui, block_root_interaction);
        if block_root_interaction {
            Self::render_root_viewport_stabilizing_overlay(ctx);
        }
        self.finalize_frame(ctx, had_panel_output, workspace_count_before, panel_count_before);
        // Last, after every phase that can move or hide a panel this frame.
        self.restamp_browser_manifests_for_placement();
    }
}

impl eframe::App for HorizonApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        for state in self.panel_render_caches.device_ui_state.values_mut() {
            state.begin_frame();
        }
        // A close asked for the end of a machine's session on the frame that removed
        // the panel; the worker runs it here, and never on the UI thread.
        self.dispatch_session_kills();
        self.update_ui(ui);
        self.render_saved_session_deletion_notice(ui.ctx());
        self.record_root_device_presentation(ui.ctx());
        // Immediate detached viewports have finished too. Reconcile once, even
        // when startup or session-switch overlays bypass panel rendering.
        for state in self.panel_render_caches.device_ui_state.values_mut() {
            state.finish_frame(ui.ctx());
        }
        self.complete_settled_device_reveals(ui.ctx());
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::bg_for(self.resolved_theme).to_normalized_gamma_f32()
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        #[cfg(target_os = "linux")]
        {
            if self.observed_keyboard_inputs.native_context(ctx) {
                let publish_target = self.observed_keyboard_inputs.native_recipient_publisher();
                // Immediate child viewports bypass App::raw_input_hook. Publish
                // the recipient after each viewport has resolved widget focus.
                ctx.on_end_pass(
                    "native image paste recipient",
                    std::sync::Arc::new(move |ctx| {
                        let viewport = ctx.viewport_id();
                        let target = ctx.data_mut(|data| {
                            let id = egui::Id::unique(("native_image_paste_target", viewport));
                            let target = data.get_temp::<(PanelId, egui::Id)>(id);
                            data.remove::<(PanelId, egui::Id)>(id);
                            target
                        });
                        if ctx.input(|input| input.focused) {
                            let target = target
                                .filter(|(_, widget)| {
                                    ctx.memory(egui::Memory::focused)
                                        .is_none_or(|focused| focused == *widget)
                                })
                                .map(|(panel, _)| panel);
                            let scale = ctx.input(|input| input.viewport().native_pixels_per_point.unwrap_or(1.0));
                            publish_target(viewport, f64::from(scale), target);
                        }
                    }),
                );
            }
            ctx.data_mut(|data| {
                data.insert_temp(
                    egui::Id::unique("native_wayland_input"),
                    self.observed_keyboard_inputs.is_wayland_backend(),
                )
            });
            self.handle_native_image_pastes();
        }
        self.apply_clipboard_image_pastes();
        if raw_input.viewport_id == egui::ViewportId::ROOT && self.press_frame.split(&mut raw_input.events) {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
        self.filter_canvas_gesture(ctx, raw_input);
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
        self.abandon_device_reveals("Horizon is exiting");
        self.run_exit_cleanup();
        // macOS can leave Horizon running as a windowless app after eframe
        // has already torn down the viewport, so terminate explicitly.
        crate::plugin_install::exit_after_releasing_plugins(0);
    }
}

impl StartupChooserState {
    fn new(chooser: StartupChooser) -> Self {
        let selected_session_id = chooser.sessions.first().map(|session| session.session_id.clone());
        Self {
            chooser,
            selected_session_id,
            error: None,
            busy: None,
        }
    }
}
