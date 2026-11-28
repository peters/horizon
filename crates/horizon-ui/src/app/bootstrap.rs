//! Application construction, initial state and startup-only resource setup.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::Instant;

use egui::Context;
use horizon_core::remote_browser_credential::CredentialWorkbench;
use horizon_core::{
    AgentSessionCatalog, AppShortcuts, Board, CanvasViewState, Config, RemoteHostCatalog, SessionStore, StartupDecision,
};

use super::ssh_hosts::SshHostsRead;
use super::{
    BrowserCreateHostState, CanvasGridCache, CanvasPanSpaceKeyState, FrameStats, HorizonApp, PanelRenderCaches,
    StartupChooserState,
    file_drop::PendingEditorDrops,
    resolve_shortcuts,
    sidebar::machines::{NoticeClock, SidebarNoticeTone},
    util,
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
    shortcuts: AppShortcuts,
    action_commands_cache: Vec<CommandEntry>,
    output_wake: horizon_core::OutputWake,
    undrawn: super::undrawn::Undrawn,
}

impl HorizonApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: &Config,
        config_path: PathBuf,
        session_store: SessionStore,
        startup: StartupDecision,
        observed_keyboard_inputs: input::ObservedKeyboardInputs,
        startup_folder: Option<PathBuf>,
    ) -> Self {
        let mut app = Self::new_with_egui_context(
            &cc.egui_ctx,
            config,
            config_path,
            session_store,
            startup,
            observed_keyboard_inputs,
            startup_folder,
        );
        // One worker for the run: a frame only writes one value into it.
        app.notifications.use_desktop_sink();
        app
    }

    pub(super) fn new_with_egui_context(
        egui_ctx: &Context,
        config: &Config,
        config_path: PathBuf,
        session_store: SessionStore,
        startup: StartupDecision,
        observed_keyboard_inputs: input::ObservedKeyboardInputs,
        startup_folder: Option<PathBuf>,
    ) -> Self {
        // The window exists: everything below this line runs on the UI thread of
        // the frame that the window draws.
        let _frame = horizon_core::ui_thread::FrameScope::enter();
        let shortcuts = resolve_shortcuts(config);
        let action_commands_cache = command_registry::action_commands(&shortcuts, util::primary_shortcut_label());
        pin_chrome_to_native_display_scale(egui_ctx);
        egui_ctx.set_fonts(configure_fonts());
        let board = Board::new();
        // The probe of `script` starts a process and waits for it, so a worker
        // resolves it once for the panels of this process.
        horizon_core::warm_script_support();
        let resolved_theme = theme::resolve_theme(config.appearance.theme, egui_ctx.system_theme());
        theme::set_theme(resolved_theme);

        // The time of the store comes from the settings worker on the first
        // frame, so the window opens without reading it.
        let config_last_mtime = None;

        let undrawn = super::undrawn::Undrawn::default();
        let bootstrap = AppBootstrap {
            config_path,
            session_store,
            observed_keyboard_inputs,
            board,
            resolved_theme,
            config_last_mtime,
            shortcuts,
            action_commands_cache,
            output_wake: undrawn.wake(egui_ctx),
            undrawn,
        };
        let mut app = Self::initial_state(config, bootstrap);
        // The folder of a `horizon <folder>` start opens after the board of the
        // last run is back, so its workspace stands beside the restored ones.
        app.startup_folder =
            startup_folder.map(|folder| super::startup_folder::StartupFolder { folder, request: None });
        // The hotkey of speech input comes with the settings of this start, so it
        // works before the first save of the window.
        app.speech.configure(config);
        app.restore_saved_session_deletion_notice();
        // The machines belong to the person and not to the session that the
        // startup decision opens, so they are read once, before any session. A
        // link to a server wakes this context when the list changes.
        app.load_machines(egui_ctx);

        match startup {
            StartupDecision::Open { session, .. } => {
                let left_out = session.retired_panels.clone();
                app.activate_persistent_session(&session);
                if let Some(notice) = super::board_notice::left_out_panels_notice(&left_out) {
                    // The panels are gone for good, so this line stays until the
                    // person closes it: a person who looks away at the start must
                    // still read what left the session.
                    app.set_sidebar_sticky_notice(notice);
                }
            }
            StartupDecision::Ephemeral { runtime_state } => app.activate_ephemeral_session(&runtime_state),
            StartupDecision::Choose(chooser) => app.startup_chooser = Some(StartupChooserState::new(chooser)),
        }

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
            shortcuts,
            action_commands_cache,
            output_wake,
            undrawn,
        }: AppBootstrap,
    ) -> Self {
        Self {
            board,
            panels_to_close: Vec::new(), panels_to_restart: Vec::new(), keyboard_focus: None,
            workspace_assignments: Vec::new(), workspace_creates: Vec::new(),
            appearance_theme: config.appearance.theme,
            resolved_theme,
            theme_applied: false, theme_settling: false,
            panel_screen_rects: HashMap::new(),
            panel_screen_order: Vec::new(),
            panel_render_order: Vec::new(),
            workspace_colors: Vec::new(),
            panel_render_caches: PanelRenderCaches::default(),
            device_claim: super::device_requests::claim::DeviceClaimState::default(),
            block_claim: super::block_requests::BlockClaimState::default(),
            block_clicks: super::block_clicks::BlockClickState::default(),
            settings_request: super::settings_request::SettingRequestState::default(),
            agent_opens: super::AgentOpens::default(),
            machine_list: empty_machine_list(session_store.home()),
            sidebar_notice: None, sidebar_notice_until: None, sidebar_notice_tone: SidebarNoticeTone::Error,
            sidebar_sticky_notice: None,
            settings_lost: None, person_browser_creates: super::creation_notice::PersonBrowserCreates::default(),
            machine_rows: super::sidebar::machines::MachineRows::default(), notice_clock: NoticeClock::default(),
            machine_watch: super::machine_watch::MachinesWatch::default(), reconnects: super::reconnect::ReconnectWaits::default(), node_cards: super::node_cards::NodeCards::default(),
            session_kills: Vec::new(), forget_machine: None, close_workspace_confirm: None, close_panel_confirm: None,
            machine_epoch: 0, machine_load_rx: None, server_sync: None, link_child: None, shared_terminals: super::shared_terminals::SharedTerminals::default(),
            canvas_grid_cache: CanvasGridCache::default(), canvas_backdrop_cache: super::canvas_backdrop::BackdropCache::default(), agent_lines: super::agent_lines::AgentLines::default(),
            frame_stats: FrameStats::default(), output_wake, undrawn, workspace_screen_rects: Vec::new(),
            fullscreen_panel: None, held_navigation_keys: Vec::new(),
            sidebar_visible: true, sidebar_drag_workspace: None,
            sidebar_list: super::sidebar::ListCache::default(), attention: super::attention::AttentionState::new(session_store.home().root()),
            notifications: super::notify::Notifications::default(), keep_running: super::keep_running::KeepRunning::default(),
            minimap_visible: true, hud_visible: false, orchestrator_dock: super::orchestrator_dock::OrchestratorDock::from_settings(config.features.orchestrator_dock),
            renaming_workspace: None,
            rename_buffer: String::new(),
            renaming_panel: None,
            panel_rename_buffer: String::new(),
            remote_browser_credentials: spawn_remote_browser_credentials(config, session_store.home()),
            keyring_import: horizon_core::remote_browser_credential::KeyringImport::new(),
            session_store,
            active_session: None,
            startup_chooser: None,
            config_path, transcript_root: None,
            template_config: config.clone(),
            shortcuts,
            presets: config.ssh_presets(&[]), ssh_hosts: SshHostsRead::default(),
            settings_hosts_refreshed: false, config_mtime_known: false,
            config_probe_pending: false, config_last_check: None, window_config: config.window.clone(),
            detached_workspaces: BTreeMap::new(),
            pending_detached_reattach: BTreeSet::new(),
            pending_detached_window_position_restore: BTreeSet::new(),
            session_catalog: AgentSessionCatalog::default(),
            session_pickup: super::session_pickup::SessionPickup::default(), startup_folder: None,
            folder_requests: super::folder_requests::FolderRequestPump::default(), startup_receiver: None,
            pending_startup_runtime_state: None,
            pending_startup_runtime_state_changed: false,
            startup_bootstrap_failure: None, busy_claude_session_ids: std::collections::HashSet::new(),
            session_catalog_refresh: super::session::SessionCatalogRefreshState::default(),
            saved_session_deletion: super::panels::SavedSessionDeletionState::default(),
            remote_hosts_overlay: None,
            remote_hosts_catalog: RemoteHostCatalog::default(),
            remote_hosts_refresh_rx: None,
            remote_hosts_refresh_in_flight: false,
            remote_hosts_last_refresh: None,
            last_panel_output_at: Some(Instant::now()), browser_create_host: BrowserCreateHostState::default(),
            settings: None, settings_io: super::settings_io::SettingsIo::new(), settings_reload_pending: false,
            named_secrets: named_secret_workbench(config), named_secret_vault: None,
            session_manager: None, session_jobs: super::session_jobs::SessionJobs::default(),
            session_manager_reload: None,
            canvas_gesture: super::canvas_gesture::CanvasGesture::default(), pending_preset_pick: None,
            press_frame: super::press_frame::PressFrame::default(),
            dir_picker: None, command_palette: None, preset_programs: super::actions::support::PresetPrograms::default(), agent_programs: super::sidebar::agents::AgentPrograms::default(), search_overlay: None,
            action_commands_cache,
            runtime_dirty_since: None,
            startup_workspace_organization_pending: false, startup_selection_restored: false,
            initial_pan_done: false, root_viewport_stabilizer: None,
            file_hover_positions: HashMap::new(), file_drop_highlight: None,
            pending_editor_drops: PendingEditorDrops::default(),
            panel_starts: super::panel_starts::PanelStarts::default(), cwd_reads: super::cwd_reads::CwdReads::default(),
            panel_restarts: super::panel_restarts::PanelRestarts::default(),
            ssh_upload_flow: None,
            ssh_upload_destinations: HashMap::new(),
            canvas_view: CanvasViewState::default(),
            pan_target: None, is_panning: false,
            middle_pan_active: false,
            canvas_pan_input_claimed: false, arranged_panel_drag: None,
            pending_space_pan_key: CanvasPanSpaceKeyState::Idle,
            observed_keyboard_inputs,
            ime_commit_normalizer: input::ImeCommitNormalizer::default(),
            frame_keyboard_events: HashMap::new(),
            terminal_keyboard_events: Vec::new(),
            terminal_body_screen_rects: HashMap::new(),
            primary_selection: PrimarySelection::new(),
            clipboard_images: crate::clipboard_image::ClipboardImages::new(),
            terminal_selection_drag: TerminalSelectionDragState::default(),
            config_last_mtime, shutdown_progress: None,
            pending_session_switch: None, exit_cleanup_complete: false, exit_code: None, welcome: super::welcome::WelcomeCache::default(),
            setup: super::setup::SetupState::new(),
            exit_save_deadline: None,
            transcript_deletes: super::transcript_deletes::PendingTranscriptDeletes::default(), speech: super::speech::SpeechUi::new(),
        }
    }
}

/// The machine list before its read answers: this home keeps no machine yet.
fn empty_machine_list(home: &horizon_core::HorizonHome) -> horizon_core::Machines {
    horizon_core::Machines::with_records(horizon_core::MachineRegistry::from_home(home), Vec::new())
}

/// The named secrets of this start. The startup read of the machine list opens the
/// secrets file on its worker thread and replaces this with the names that the file
/// answered, so the window never waits for a disk read here.
fn named_secret_workbench(config: &Config) -> horizon_core::named_secrets::NamedSecretWorkbench {
    horizon_core::named_secrets::NamedSecretWorkbench::new(None, &config.secrets.secret_names())
}

fn spawn_remote_browser_credentials(config: &Config, home: &horizon_core::HorizonHome) -> CredentialWorkbench {
    let mut workbench = CredentialWorkbench::spawn_file(home.secrets_path());
    workbench.load_environment_bindings(&config.browser.remote);
    workbench
}

pub(crate) fn configure_fonts() -> egui::FontDefinitions {
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
    // The two faces of the interface itself, hinted for even stems at the screen
    // sizes that the interface has.
    for name in [FONT_INTER, FONT_JETBRAINS_MONO] {
        let hinted = fonts.font_data.remove(name).map(|font| hinted((*font).clone()).into());
        if let Some(font) = hinted {
            fonts.font_data.insert(name.to_owned(), font);
        }
    }
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

/// The rasterising of the two faces that the interface draws its own text in.
///
/// Both faces are hinted true-type faces that the interface draws at 9 to 14
/// screen points, where an unhinted stem can land anywhere between two pixels
/// and the stem beside it lands somewhere else: that is the uneven weight that
/// a person reads as a blurry line. Normal hinting grid-fits a stem to a pixel.
///
/// The sub-pixel bins of the rasteriser are the other half, and they are the
/// larger half. With the bins on, the toolkit draws a glyph at the quarter
/// pixel that its pen position falls on, so one letter of a run is a picture
/// with its stem in one column and the next letter is a picture with the same
/// stem in two columns at half ink: the grey double that a reader sees as a
/// soft stem. The toolkit then rounds the ink of every glyph to a whole device
/// pixel anyway, so the bins only buy a picture that no longer matches the
/// place it is drawn. With the bins off, one glyph has one picture at any pen
/// position, and a stem is the column that the hinter made of it.
///
/// The horizontal fit of the hinter is off for both faces, so that every glyph
/// keeps the ink and the advance that the unhinted face has. This rasteriser
/// makes the ink of Inter lighter and wider with the fit on: the stem of an
/// `l` at 12.5 points is two columns of a quarter ink instead of one column
/// and two fading edges, while the advances do not change at all. On `JetBrains Mono` the
/// fit changes neither the ink nor the advance, so the terminal keeps the same
/// cells with the fit off and one rule covers both faces.
fn hinted(font: egui::FontData) -> egui::FontData {
    font.tweak(egui::epaint::text::FontTweak {
        hinting: Some(true),
        hinting_target: egui::epaint::text::HintingTarget::Smooth(egui::epaint::text::SmoothHinting {
            light: false,
            symmetric_rendering: true,
            preserve_linear_metrics: true,
        }),
        subpixel_binning: Some(false),
        ..egui::epaint::text::FontTweak::default()
    })
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
    use crate::app::test_support::{
        editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup,
    };
    use crate::test_egui::DiscardTextures;
    use horizon_core::{CanvasViewState, RuntimeState, StartupDecision};

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

    /// The stems of one line must be equally thick at 9 to 14 screen points,
    /// which is the hinting that the interface asks of its own two faces.
    #[test]
    fn the_two_faces_of_the_interface_are_hinted_for_even_stems() {
        let fonts = configure_fonts();
        let smooth = |name: &str| {
            let tweak = &fonts.font_data[name].tweak;
            assert_eq!(tweak.hinting, Some(true), "{name} hints its stems");
            let egui::epaint::text::HintingTarget::Smooth(smooth) = tweak.hinting_target else {
                panic!("{name} should be hinted for anti-aliased text");
            };
            smooth
        };
        for name in [FONT_INTER, FONT_JETBRAINS_MONO] {
            let face = smooth(name);
            assert!(!face.light, "{name} fits its stems to the pixel");
            assert!(
                face.preserve_linear_metrics,
                "{name} keeps the ink and the advance of the unhinted face: the horizontal fit makes its stems lighter and wider"
            );
            assert_eq!(
                fonts.font_data[name].tweak.subpixel_binning,
                Some(false),
                "{name} draws one picture of a glyph at any pen position"
            );
        }
        assert_eq!(
            fonts.font_data[FONT_NOTO_CJK].tweak.hinting, None,
            "a fallback face keeps the default hinting"
        );
    }

    /// One glyph has one picture at any pen position, so its stem cannot land
    /// in one column for one letter and in two columns at half ink for the
    /// next: the grey double of a sub-pixel bin.
    #[test]
    fn a_glyph_has_one_picture_at_any_pen_position() {
        let ctx = egui::Context::default();
        ctx.set_fonts(configure_fonts());
        let mut reports = Vec::new();
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                for font in [egui::FontId::monospace(13.0), egui::FontId::proportional(12.5)] {
                    let galley = ui
                        .painter()
                        .layout_no_wrap("llllilil".to_owned(), font.clone(), egui::Color32::WHITE);
                    let glyphs = &galley.rows[0].row.glyphs;
                    let pictures: Vec<([u16; 2], [u16; 2])> = glyphs
                        .iter()
                        .filter(|glyph| glyph.chr == 'l')
                        .map(|glyph| (glyph.uv_rect.min, glyph.uv_rect.max))
                        .collect();
                    reports.push((font, pictures));
                }
            })
            .discard_textures();
        for (font, pictures) in reports {
            assert!(pictures.len() >= 3, "{} draws several l", font.family);
            let first = pictures[0];
            assert!(
                pictures.iter().all(|picture| *picture == first),
                "the pen position of {} must not change its picture: {pictures:?}",
                font.family
            );
        }
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
            runtime_state: Box::new(RuntimeState {
                canvas_view: Some(CanvasViewState::default()),
                workspaces: vec![editor_workspace_state("alpha", [0.0, 0.0])],
                ..RuntimeState::default()
            }),
        });
        app.root_viewport_stabilizer = None;
        let viewport = raw_input([1600.0, 1000.0], None);
        let _ = run_app_frame_with_input(&ctx, &mut app, viewport.clone());

        let before = app.canvas_view.zoom;
        let mut zoom_in = viewport.clone();
        zoom_in.events.push(Event::Key {
            key: Key::Equals,
            physical_key: Some(Key::Equals),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                ctrl: !cfg!(target_os = "macos"),
                mac_cmd: cfg!(target_os = "macos"),
                command: true,
                ..egui::Modifiers::NONE
            },
        });
        let _ = run_app_frame_with_input(&ctx, &mut app, zoom_in);
        let _ = run_app_frame_with_input(&ctx, &mut app, viewport);

        assert!(app.canvas_view.zoom > before);
        assert!(!ctx.options(|options| options.zoom_with_keyboard));
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
