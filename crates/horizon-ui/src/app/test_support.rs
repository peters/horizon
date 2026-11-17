use crate::test_egui::DiscardTextures;
use egui::{Context, Pos2, RawInput, Rect, Vec2, ViewportId};
use horizon_core::{
    Config, HorizonHome, PanelKind, PanelState, RuntimeState, SessionStore, StartupDecision, WorkspaceState,
};
use tempfile::TempDir;

use super::HorizonApp;
use crate::block_widget::{CARD_CHROME_HEIGHT, CARD_FOOT_SPAN};
use crate::input;

pub(super) fn test_app() -> (TempDir, HorizonApp) {
    let (temp, _ctx, app) = test_app_with_config_and_startup(
        &Config::default(),
        StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        },
    );
    (temp, app)
}

pub(super) fn test_app_with_startup(startup: StartupDecision) -> (TempDir, Context, HorizonApp) {
    test_app_with_config_and_startup(&Config::default(), startup)
}

pub(super) fn test_app_with_config_and_startup(
    config: &Config,
    startup: StartupDecision,
) -> (TempDir, Context, HorizonApp) {
    let temp = tempfile::tempdir().expect("temp dir");
    let ctx = Context::default();
    let app = app_in(&ctx, temp.path(), config, startup);
    (temp, ctx, app)
}

/// The app of a test on the home of `folder`, which reads its settings from
/// `config` and starts with `startup`.
///
/// A test that starts a holder of a panel names a short folder, because the path
/// of a socket of a holder is bounded.
pub(super) fn app_in(ctx: &Context, folder: &std::path::Path, config: &Config, startup: StartupDecision) -> HorizonApp {
    // The whole suite runs under the guard in strict mode, from the start of the
    // window: a test that does work which can wait on the UI thread panics
    // instead of freezing the window of a person.
    horizon_core::ui_thread::use_strict_mode();
    let config_path = folder.join("config.json");
    let session_store = SessionStore::new(HorizonHome::from_root(folder.join(".horizon")), config_path.clone());
    let mut app = HorizonApp::new_with_egui_context(
        ctx,
        config,
        config_path,
        session_store,
        startup,
        input::ObservedKeyboardInputs::default(),
        None,
    );
    // The card sockets of a test are bound in the folder of that test: two apps of one
    // process would otherwise share the folder of the process, and the end of one would
    // remove the sockets of the other.
    app.node_cards.base = folder.join("cards");
    app
}

pub(super) fn run_app_frame(ctx: &Context, app: &mut HorizonApp) {
    run_app_frame_with_input(ctx, app, RawInput::default());
}

/// One short folder of a test, for a path that has a bound of its own: the temporary
/// folder of macOS holds 49 bytes, and the path of a Unix socket takes 100.
pub(super) fn short_temp(prefix: &str) -> TempDir {
    ["/tmp", "/var/tmp"]
        .iter()
        .find_map(|base| tempfile::Builder::new().prefix(prefix).tempdir_in(base).ok())
        .expect("a short folder of a test")
}

/// The options of a browser panel of a test: a program that never starts, so the
/// panel starts no browser and a test judges the route that the host chose.
///
/// `follows_browser_host` is the choice of the caller that the real create makes:
/// the preset picker and an agent's create follow the browser host of the
/// settings, and a saved panel or a page of a local profile does not.
pub(super) fn browser_panel_options(app: &HorizonApp, follows_browser_host: bool) -> horizon_core::PanelOptions {
    let root = app.session_store.home().root();
    horizon_core::PanelOptions {
        kind: PanelKind::Browser,
        command: Some("https://example.test/".to_owned()),
        follows_browser_host,
        browser_config: Some(horizon_core::browser::BrowserConfig {
            command: Some(root.join("no-browser").to_string_lossy().into_owned()),
            profile_root: Some(root.join("browser-profiles")),
            ..horizon_core::browser::BrowserConfig::default()
        }),
        ..horizon_core::PanelOptions::default()
    }
}

/// Runs one frame of the window and answers the visuals that its panels drew with.
///
/// The root ui of a frame takes the style of the theme that was active when the
/// frame began, and every widget of the frame inherits it, so this is the style of
/// the fields and the check boxes of the frame.
pub(super) fn run_app_frame_visuals(ctx: &Context, app: &mut HorizonApp) -> egui::Visuals {
    let _frame = horizon_core::ui_thread::FrameScope::enter();
    let mut frame = eframe::Frame::_new_kittest();
    let mut visuals = None;
    let _ = ctx
        .run_ui(RawInput::default(), |ui| {
            eframe::App::ui(app, ui, &mut frame);
            visuals = Some(ui.style().visuals.clone());
        })
        .discard_textures();
    visuals.expect("one frame of the window")
}

pub(super) fn run_app_frame_with_input(ctx: &Context, app: &mut HorizonApp, input: RawInput) -> egui::FullOutput {
    let _frame = horizon_core::ui_thread::FrameScope::enter();
    let mut frame = eframe::Frame::_new_kittest();
    ctx.run_ui(input, |ui| {
        eframe::App::ui(app, ui, &mut frame);
    })
    .discard_textures()
}

#[derive(Debug)]
struct TestDroppedFile(std::path::PathBuf);

impl egui::DroppedFile for TestDroppedFile {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|error| error.to_string())
    }
}

pub(super) fn dropped_file(path: impl Into<std::path::PathBuf>) -> egui::DroppedFileHandle {
    std::sync::Arc::new(TestDroppedFile(path.into()))
}

pub(super) fn raw_input(size: [f32; 2], position: Option<[f32; 2]>) -> RawInput {
    let inner_rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(size[0], size[1]));
    let mut input = RawInput {
        screen_rect: Some(inner_rect),
        ..RawInput::default()
    };
    let viewport = input.viewports.entry(ViewportId::ROOT).or_default();
    viewport.inner_rect = Some(inner_rect);
    viewport.outer_rect =
        position.map(|position| Rect::from_min_size(Pos2::new(position[0], position[1]), egui::vec2(size[0], size[1])));
    input
}

/// The modifiers of a primary shortcut press on this system, as a real press
/// reports them: Cmd on macOS, Ctrl on Linux.
///
/// A test that feeds a modified canvas click from `egui::Modifiers::CTRL` names
/// a press that no system sends: macOS reports Cmd as `command` and its Control
/// key as `ctrl` alone.
pub(super) fn primary_modifier_press() -> egui::Modifiers {
    egui::Modifiers {
        ctrl: !cfg!(target_os = "macos"),
        mac_cmd: cfg!(target_os = "macos"),
        command: true,
        ..egui::Modifiers::NONE
    }
}

/// The id of a real panel: 36 characters, as the app gives it.
pub(super) const HELD_PANEL_ID: &str = "9bc6f6c5-175a-4f16-a55a-1619bb1a93b1";
/// The line that the stand-in program of a holder prints.
pub(super) const HELD_PROGRAM_MARKER: &str = "held marker";

/// The arguments of the stand-in program of a holder: it prints the marker and
/// reads its input until it ends.
pub(super) fn held_program_args() -> Vec<String> {
    vec!["-c".to_owned(), format!("printf '{HELD_PROGRAM_MARKER}\\n'; exec cat")]
}

/// The launch that the holder of a test owns, which prints the marker.
pub(super) fn held_program_launch(local_id: &str) -> horizon_core::panel_holder::Launch {
    horizon_core::panel_holder::Launch {
        local_id: local_id.to_owned(),
        program: "/bin/sh".to_owned(),
        args: held_program_args(),
        cwd: Some(std::path::PathBuf::from("/tmp")),
        env: std::collections::HashMap::new(),
        rows: 24,
        cols: 80,
        cell_width: 8,
        cell_height: 17,
    }
}

/// The settings of a test whose agent programs run in holders.
pub(super) fn keep_agents_config() -> Config {
    let mut config = Config::default();
    config.features.keep_agents_through_restart = true;
    config
}

/// The saved panel of an agent that continues its work after a restart, whose
/// program the holder of a test runs.
pub(super) fn held_panel_state(local_id: &str) -> PanelState {
    PanelState {
        local_id: local_id.to_owned(),
        name: "Codex".to_owned(),
        kind: PanelKind::Codex,
        command: Some("/bin/sh".to_owned()),
        args: held_program_args(),
        resume: horizon_core::PanelResume::Fresh,
        work_resume: horizon_core::agent_work::ResumePolicy {
            enabled: true,
            ..horizon_core::agent_work::ResumePolicy::default()
        },
        rows: 24,
        cols: 80,
        ..PanelState::default()
    }
}

/// Whether a program of `pid` still runs. Signal 0 reads the process table and
/// sends nothing to the process.
pub(super) fn program_runs(pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub(super) fn editor_panel_state(local_id: &str, position: [f32; 2]) -> PanelState {
    PanelState {
        local_id: local_id.to_string(),
        name: format!("{local_id} notes"),
        kind: PanelKind::Editor,
        position: Some(position),
        size: Some([320.0, 220.0]),
        ..PanelState::default()
    }
}

pub(super) fn editor_workspace_state(local_id: &str, position: [f32; 2]) -> WorkspaceState {
    WorkspaceState {
        local_id: local_id.to_string(),
        name: local_id.to_string(),
        position: Some(position),
        panels: vec![editor_panel_state(
            &format!("{local_id}-panel"),
            [position[0] + 20.0, position[1] + 60.0],
        )],
        ..WorkspaceState::default()
    }
}

/// Waits until the session writer wrote every lease of the active session, so a
/// test can read the lease file on disk.
pub(super) fn wait_for_lease_writes(app: &mut HorizonApp) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        app.poll_lease_answers();
        let pending = app
            .active_session
            .as_ref()
            .is_some_and(|session| !session.lease_answers.is_empty());
        if !pending {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the session writer wrote no lease"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Waits until the session writer saved every queued board, so a test can read the session
/// files on disk.
pub(super) fn wait_for_session_saves(app: &HorizonApp) {
    assert!(
        app.session_store
            .flush_runtime_states(std::time::Duration::from_secs(30)),
        "the session writer did not save the board"
    );
}

/// The ink of the text `text` that a frame drew.
pub(super) fn drawn_text(output: &egui::FullOutput, text: &str) -> Rect {
    output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::epaint::Shape::Text(shape) if shape.galley.text() == text => {
                Some(Rect::from_min_size(shape.pos, shape.galley.size()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("the frame drew no {text}"))
}

/// Whether the frame drew the words `text`.
pub(super) fn drew_text(output: &egui::FullOutput, text: &str) -> bool {
    output
        .shapes
        .iter()
        .any(|clipped| matches!(&clipped.shape, egui::epaint::Shape::Text(shape) if shape.galley.text() == text))
}

/// The card panel that `card` names, as the canvas draws it this frame.
pub(super) fn card_panel(app: &HorizonApp, ctx: &Context, card: &str) -> Rect {
    let panel = app
        .board
        .panel(app.board.panel_id_by_local_id(card).expect("the card"))
        .expect("the card");
    let min = app.canvas_to_screen(
        app.canvas_rect(ctx),
        Pos2::new(panel.layout.position[0], panel.layout.position[1]),
    );
    Rect::from_min_size(min, app.canvas_size_to_screen(Vec2::from(panel.layout.size)))
}

/// The room that the body of a card panel draws in: below its titlebar and above
/// the footer that stays at its bottom.
pub(super) fn card_body(panel: Rect, zoom: f32) -> Rect {
    Rect::from_min_max(
        Pos2::new(panel.min.x, panel.min.y + (CARD_CHROME_HEIGHT - CARD_FOOT_SPAN) * zoom),
        Pos2::new(panel.max.x, panel.max.y - CARD_FOOT_SPAN * zoom),
    )
}
