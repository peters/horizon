//! Application construction, initial state and startup-only resource setup.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::Instant;

use egui::Context;
use horizon_core::remote_browser_credential::CredentialWorkbench;
use horizon_core::{
    AgentSessionCatalog, AppShortcuts, Board, CanvasViewState, Config, ManagedInstall, RemoteHostCatalog, SessionStore,
    StartupDecision,
};

use super::{
    BrowserCreateHostState, CanvasGridCache, CanvasPanSpaceKeyState, FrameStats, HorizonApp, PanelRenderCaches,
    StartupChooserState, resolve_shortcuts, settings, speech, util,
};
use crate::command_registry::{self, CommandEntry};
use crate::input;
use crate::primary_selection::PrimarySelection;
use crate::terminal_widget::TerminalSelectionDragState;
use crate::theme;

const FONT_INTER: &str = "inter";
const FONT_JETBRAINS_MONO: &str = "jetbrains-mono";
const FONT_NOTO_CJK: &str = "noto-sans-cjk-sc";
const FONT_NOTO_SYMBOLS: &str = "noto-sans-symbols-2";

struct AppBootstrap {
    config_path: PathBuf,
    session_store: SessionStore,
    observed_keyboard_inputs: input::ObservedKeyboardInputs,
    board: Board,
    resolved_theme: theme::ResolvedTheme,
    config_last_mtime: Option<std::time::SystemTime>,
    managed_install: Option<ManagedInstall>,
    next_surge_update_check_at: Option<Instant>,
    shortcuts: AppShortcuts,
    action_commands_cache: Vec<CommandEntry>,
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
        Self::new_with_egui_context(
            &cc.egui_ctx,
            config,
            config_path,
            session_store,
            startup,
            observed_keyboard_inputs,
        )
    }

    pub(super) fn new_with_egui_context(
        egui_ctx: &Context,
        config: &Config,
        config_path: PathBuf,
        session_store: SessionStore,
        startup: StartupDecision,
        observed_keyboard_inputs: input::ObservedKeyboardInputs,
    ) -> Self {
        let shortcuts = resolve_shortcuts(config);
        let action_commands_cache = command_registry::action_commands(&shortcuts, util::primary_shortcut_label());
        pin_chrome_to_native_display_scale(egui_ctx);
        egui_ctx.set_fonts(configure_fonts());
        let mut board = Board::new();
        board.attention_enabled = config.features.attention_feed;
        let resolved_theme = theme::resolve_theme(config.appearance.theme, egui_ctx.system_theme());
        theme::set_theme(resolved_theme);

        let config_last_mtime = std::fs::metadata(&config_path).ok().and_then(|m| m.modified().ok());
        let (managed_install, next_surge_update_check_at) = managed_install_state();

        let bootstrap = AppBootstrap {
            config_path,
            session_store,
            observed_keyboard_inputs,
            board,
            resolved_theme,
            config_last_mtime,
            managed_install,
            next_surge_update_check_at,
            shortcuts,
            action_commands_cache,
        };
        let mut app = Self::initial_state(config, bootstrap);

        match startup {
            StartupDecision::Open { session, .. } => app.activate_persistent_session(&session),
            StartupDecision::Ephemeral { runtime_state } => app.activate_ephemeral_session(&runtime_state),
            StartupDecision::Choose(chooser) => app.startup_chooser = Some(StartupChooserState::new(chooser)),
        }

        app.maybe_start_update_check();

        app
    }

    #[rustfmt::skip]
    fn initial_state(
        config: &Config,
        AppBootstrap {
            config_path,
            session_store,
            observed_keyboard_inputs,
            board,
            resolved_theme,
            config_last_mtime,
            managed_install,
            next_surge_update_check_at,
            shortcuts,
            action_commands_cache,
        }: AppBootstrap,
    ) -> Self {
        Self {
            board,
            panels_to_close: Vec::new(),
            panels_to_restart: Vec::new(),
            workspace_assignments: Vec::new(),
            workspace_creates: Vec::new(),
            appearance_theme: config.appearance.theme,
            resolved_theme,
            theme_applied: false,
            panel_screen_rects: HashMap::new(),
            panel_screen_order: Vec::new(),
            panel_render_order: Vec::new(),
            workspace_colors: Vec::new(),
            panel_render_caches: PanelRenderCaches::default(),
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
            speech: speech::SpeechSystem::from_config(&config.features.speech),
            speech_global_hotkeys: None, speech_global_hotkeys_tried: false, speech_global_hotkeys_suspended: false, speech_global_events_pending: Vec::new(),
            speech_held_bindings: Vec::new(),
            speech_engaged_profile: None,
            speech_escape_cancelled: false, speech_escape_release_pending: false,
            speech_escape_release_deadline: None,
            speech_notice: None,
            any_viewport_focused: true,
            shortcuts,
            presets: config.resolved_presets(),
            window_config: config.window.clone(),
            detached_workspaces: BTreeMap::new(),
            pending_detached_reattach: BTreeSet::new(),
            pending_detached_window_position_restore: BTreeSet::new(),
            session_catalog: AgentSessionCatalog::default(),
            startup_receiver: None,
            pending_startup_runtime_state: None,
            pending_startup_runtime_state_changed: false,
            startup_bootstrap_failure: None,
            session_catalog_refresh: None,
            remote_hosts_overlay: None,
            remote_hosts_catalog: RemoteHostCatalog::default(),
            remote_hosts_refresh_rx: None,
            remote_hosts_refresh_in_flight: false,
            remote_hosts_last_refresh: None,
            last_session_catalog_refresh: None,
            last_panel_output_at: Some(Instant::now()), browser_create_host: BrowserCreateHostState::default(),
            settings: None, remote_browser_credentials: spawn_remote_browser_credentials(config),
            speech_model_info_cache: settings::SpeechModelInfoCache::new(),
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
            startup_workspace_organization_pending: false,
            startup_selection_restored: false,
            initial_pan_done: false,
            root_viewport_stabilizer: None,
            file_hover_positions: HashMap::new(),
            file_drop_highlight: None,
            ssh_upload_flow: None,
            ssh_upload_destinations: HashMap::new(),
            canvas_view: CanvasViewState::default(),
            pan_target: None,
            is_panning: false,
            middle_pan_active: false,
            canvas_pan_input_claimed: false, arranged_panel_drag: None,
            pending_space_pan_key: CanvasPanSpaceKeyState::Idle,
            observed_keyboard_inputs,
            ime_commit_normalizer: input::ImeCommitNormalizer::default(),
            frame_keyboard_events: HashMap::new(),
            terminal_keyboard_events: Vec::new(),
            git_watchers: HashMap::new(),
            terminal_body_screen_rects: HashMap::new(),
            primary_selection: PrimarySelection::new(),
            terminal_selection_drag: TerminalSelectionDragState::default(),
            config_last_mtime,
            config_last_check: None,
            shutdown_progress: None,
            pending_session_switch: None,
            exit_cleanup_complete: false,
        }
    }
}

fn spawn_remote_browser_credentials(config: &Config) -> CredentialWorkbench {
    let mut workbench = CredentialWorkbench::spawn_platform();
    workbench.load_environment_bindings(&config.browser.remote);
    workbench
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

    let proportional = fonts.families.entry(egui::FontFamily::Proportional).or_default();
    proportional.insert(0, FONT_INTER.to_owned());
    proportional.insert(1, FONT_NOTO_CJK.to_owned());
    proportional.insert(2, FONT_NOTO_SYMBOLS.to_owned());

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

/// Keep root chrome at the OS display scale.
///
/// Horizon owns Ctrl/Cmd +/-, 0, and Ctrl+scroll for canvas zoom. egui's
/// default `zoom_with_keyboard` uses those same chords to change
/// `Context::zoom_factor`, which scales the sidebar, toolbar, and minimap
/// independently of the canvas. That GUI zoom does not follow a later
/// desktop resolution or DPI change, so chrome stays oversized on a large
/// display. Pin zoom to 1.0 and leave pixel density to `native_pixels_per_point`.
pub(super) fn pin_chrome_to_native_display_scale(ctx: &Context) {
    let zoom_with_keyboard = ctx.options(|options| options.zoom_with_keyboard);
    if zoom_with_keyboard {
        ctx.options_mut(|options| options.zoom_with_keyboard = false);
    }
    if (ctx.zoom_factor() - 1.0).abs() > f32::EPSILON {
        ctx.set_zoom_factor(1.0);
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, FontFamily, Key, Modifiers, RawInput};

    use super::{FONT_INTER, FONT_JETBRAINS_MONO, FONT_NOTO_CJK, FONT_NOTO_SYMBOLS, configure_fonts};
    use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};
    use crate::command_registry::CommandId;
    use crate::test_egui::DiscardTextures;
    use horizon_core::{RuntimeState, StartupDecision};

    #[test]
    fn configure_fonts_registers_ui_and_terminal_fallback_stacks() {
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
        assert_eq!(proportional.get(1).map(String::as_str), Some(FONT_NOTO_CJK));
        assert_eq!(proportional.get(2).map(String::as_str), Some(FONT_NOTO_SYMBOLS));
        assert_eq!(monospace.first().map(String::as_str), Some(FONT_JETBRAINS_MONO));
        assert_eq!(monospace.get(1).map(String::as_str), Some(FONT_NOTO_CJK));
        assert_eq!(monospace.get(2).map(String::as_str), Some(FONT_NOTO_SYMBOLS));
        assert!(fonts.font_data.contains_key(FONT_NOTO_CJK));
        assert!(fonts.font_data.contains_key(FONT_NOTO_SYMBOLS));
    }

    #[test]
    fn egui_keyboard_zoom_scales_the_whole_ui_by_default() {
        let ctx = egui::Context::default();
        assert!(ctx.options(|options| options.zoom_with_keyboard));
        assert!((ctx.zoom_factor() - 1.0).abs() <= f32::EPSILON);

        let mut input = RawInput::default();
        input.events.push(command_key(Key::Plus));
        let _ = ctx.run_ui(input, |_| {}).discard_textures();
        let _ = ctx.run_ui(RawInput::default(), |_| {}).discard_textures();

        assert!((ctx.zoom_factor() - 1.1).abs() < 0.001);
    }

    #[test]
    fn horizon_pins_chrome_to_native_display_scale() {
        let (_temp, ctx, _app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });

        assert!(!ctx.options(|options| options.zoom_with_keyboard));
        assert!((ctx.zoom_factor() - 1.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn canvas_zoom_shortcuts_do_not_scale_root_chrome() {
        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        let viewport = raw_input([1600.0, 1000.0], None);
        let _ = run_app_frame_with_input(&ctx, &mut app, viewport.clone());

        let mut zoom_in = viewport.clone();
        zoom_in.events.push(command_key(Key::Plus));
        let _ = run_app_frame_with_input(&ctx, &mut app, zoom_in);
        let _ = run_app_frame_with_input(&ctx, &mut app, viewport);

        assert!(!ctx.options(|options| options.zoom_with_keyboard));
        assert!((ctx.zoom_factor() - 1.0).abs() <= f32::EPSILON);

        let before = app.canvas_view.zoom;
        app.execute_command(&ctx, &CommandId::ZoomIn);
        assert!(app.canvas_view.zoom > before);
        assert!((ctx.zoom_factor() - 1.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn prepare_frame_repins_chrome_if_gui_zoom_drifts() {
        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        ctx.options_mut(|options| {
            options.zoom_with_keyboard = true;
            options.zoom_factor = 1.5;
        });

        let viewport = raw_input([1600.0, 1000.0], None);
        let _ = run_app_frame_with_input(&ctx, &mut app, viewport.clone());
        let _ = run_app_frame_with_input(&ctx, &mut app, viewport);

        assert!(!ctx.options(|options| options.zoom_with_keyboard));
        assert!((ctx.zoom_factor() - 1.0).abs() <= f32::EPSILON);
    }

    fn command_key(key: Key) -> Event {
        Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }
    }
}
