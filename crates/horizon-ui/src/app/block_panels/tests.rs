//! The panel of a preset that an agent opens, judged on a test board.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::Context;
use horizon_browser_control::manifest::block::{self, Outcome};
use horizon_core::browser::manifest;
use horizon_core::{
    CanvasViewState, Config, PanelKind, PanelResume, PresetConfig, RuntimeState, StartupDecision, WindowConfig,
};

use super::*;
use crate::app::test_support::{editor_workspace_state, test_app_with_config_and_startup};

/// A preset of the settings that runs one program of this computer.
fn runnable(name: &str, alias: Option<&str>, kind: PanelKind, command: Option<&str>) -> PresetConfig {
    PresetConfig {
        name: name.into(),
        alias: alias.map(str::to_string),
        kind,
        command: command.map(str::to_string),
        args: vec!["-l".into()],
        resume: PanelResume::Fresh,
        ssh_connection: None,
    }
}

/// The settings of a person who saved one preset of every kind that a test can
/// ask for.
fn config() -> Config {
    Config {
        presets: vec![
            runnable("Task shell", Some("ts"), PanelKind::Shell, Some("/bin/sh")),
            PresetConfig {
                resume: PanelResume::Last,
                ..runnable("Claude Code", Some("cc"), PanelKind::Claude, None)
            },
            runnable("Page", None, PanelKind::Browser, None),
            runnable("Viewer", None, PanelKind::Device, None),
            runnable("Build machine", None, PanelKind::Ssh, None),
            runnable("Card", None, PanelKind::Block, None),
        ],
        ..Config::default()
    }
}

fn app() -> (tempfile::TempDir, Context, HorizonApp) {
    let runtime = RuntimeState {
        workspaces: vec![
            editor_workspace_state("first", [0.0, 0.0]),
            editor_workspace_state("second", [900.0, 0.0]),
        ],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_config_and_startup(
        &config(),
        StartupDecision::Ephemeral {
            runtime_state: Box::new(runtime),
        },
    );
    // No live agent process is needed to exercise the caller's identity.
    app.board.panels[0].kind = PanelKind::Claude;
    app.board.panels[1].kind = PanelKind::Claude;
    (temp, ctx, app)
}

/// The queue directory of a test, so no answer reaches the runtime root of the
/// person running the test.
fn queue(temp: &tempfile::TempDir) -> PathBuf {
    let root = temp.path().join("block-panel-requests");
    fs::create_dir_all(&root).expect("the queue directory");
    root
}

fn request_from(app: &HorizonApp, panel: usize, operation: block::Operation) -> block::Request {
    block::Request {
        request_id: "test".into(),
        actor: format!("horizon:{}", app.board.panels[panel].local_id),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 30_000,
        operation,
    }
}

fn request(app: &HorizonApp, operation: block::Operation) -> block::Request {
    request_from(app, 0, operation)
}

fn open_panel(preset: &str, directory: &Path, title: Option<&str>) -> block::Operation {
    block::Operation::OpenPanel {
        preset: preset.into(),
        directory: directory.display().to_string(),
        title: title.map(str::to_string),
    }
}

/// Applies one request: at once when the frame refuses it, and otherwise from the
/// answer that the coordination worker writes.
fn answer(app: &mut HorizonApp, ctx: &Context, root: &Path, operation: block::Operation) -> Outcome {
    answer_from(app, ctx, root, 0, operation)
}

/// Applies one request of the agent panel at `panel`.
fn answer_from(app: &mut HorizonApp, ctx: &Context, root: &Path, panel: usize, operation: block::Operation) -> Outcome {
    let request = request_from(app, panel, operation);
    if let Some(outcome) = app.apply_block_request_in(&request, root, ctx) {
        return outcome;
    }
    app.settle_browser_host_io();
    block::take_result_at(root, &request)
        .expect("the answer is readable")
        .expect("the worker answered")
}

fn opened(outcome: Outcome) -> String {
    match outcome {
        Outcome::Opened { panel_id } => panel_id,
        other => panic!("expected an opened panel: {other:?}"),
    }
}

fn code(outcome: &Outcome) -> &str {
    match outcome {
        Outcome::Failed { code, .. } => code,
        other => panic!("expected a refusal: {other:?}"),
    }
}

fn message(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Failed { message, .. } => message.clone(),
        other => panic!("expected a refusal: {other:?}"),
    }
}

/// The saved panel of a local id, as a restart reads it.
fn saved(app: &HorizonApp, panel_id: &str) -> horizon_core::PanelState {
    let state = RuntimeState::from_board(&app.board, WindowConfig::default(), CanvasViewState::default());
    state
        .workspaces
        .into_iter()
        .flat_map(|workspace| workspace.panels)
        .find(|panel| panel.local_id == panel_id)
        .expect("the saved panel")
}

/// A directory of a test that a panel could run in.
fn directory(temp: &tempfile::TempDir, name: &str) -> PathBuf {
    let path = temp.path().join(name);
    fs::create_dir_all(&path).expect("the directory of the test");
    path
}

#[test]
fn a_preset_opens_by_its_name_and_by_its_alias_with_the_mark_and_no_focus() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    let workspace = app.board.panels[0].workspace_id;
    let other_workspace = app.board.panels[1].workspace_id;
    let focused = app.board.panels[1].id;
    app.board.focus(focused);
    let (task, second) = (directory(&temp, "task"), directory(&temp, "second"));
    let before = app.board.panels.len();

    let panel_id = opened(answer(&mut app, &ctx, &root, open_panel("Task shell", &task, None)));
    assert_eq!(app.board.panels.len(), before + 1);
    let panel = saved(&app, &panel_id);
    assert_eq!(panel.kind, PanelKind::Shell);
    assert_eq!(panel.command.as_deref(), Some("/bin/sh"), "the command is the preset's");
    assert_eq!(panel.args, vec!["-l".to_string()], "the arguments are the preset's");
    assert_eq!(
        panel.cwd.as_deref(),
        Some(task.to_str().expect("a path")),
        "the panel runs in the folder of the request"
    );
    assert_eq!(
        panel.name, "Task shell",
        "a panel without a title is named after its preset"
    );
    assert_eq!(
        panel.opened_by.as_deref(),
        Some("Claude"),
        "the titlebar marks the panel"
    );
    let panel = app
        .board
        .panel(app.board.panel_id_by_local_id(&panel_id).expect("panel"))
        .expect("panel");
    assert_eq!(panel.workspace_id, workspace);
    assert_ne!(panel.workspace_id, other_workspace);
    assert!(panel.visible, "a new panel is visible");
    assert_eq!(
        app.board.focused,
        Some(focused),
        "opening a panel must not steal keyboard input"
    );

    let alias_id = opened(answer(&mut app, &ctx, &root, open_panel("ts", &second, None)));
    let alias = saved(&app, &alias_id);
    assert_eq!(alias.cwd.as_deref(), Some(second.to_str().expect("a path")));
    assert_eq!(alias.name, "Task shell", "an alias names the same preset");
}

#[test]
fn a_title_names_the_panel_and_a_preset_of_an_agent_starts_a_fresh_session() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    let work = directory(&temp, "fix-login");
    let title = format!("  {}  ", "fix login\nbox".repeat(8));

    let panel_id = opened(answer(
        &mut app,
        &ctx,
        &root,
        open_panel("Claude Code", &work, Some(&title)),
    ));
    let panel = saved(&app, &panel_id);
    assert_eq!(
        panel.name, "fix login",
        "the title keeps one line, trimmed to 80 characters"
    );
    assert_eq!(panel.kind, PanelKind::Claude);
    assert_eq!(
        panel.resume,
        PanelResume::Fresh,
        "a panel that an agent starts resumes no conversation"
    );
    assert_eq!(panel.opened_by.as_deref(), Some("Claude"));
}

#[test]
fn an_unknown_preset_lists_the_names_of_the_settings_and_no_command() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    let work = directory(&temp, "task");
    let before = app.board.panels.len();

    let refusal = answer(&mut app, &ctx, &root, open_panel("Task Shell", &work, None));
    assert_eq!(code(&refusal), "unknown_preset");
    let text = message(&refusal);
    assert!(text.contains("Task shell (ts)"), "the answer lists the presets: {text}");
    assert!(
        text.contains("Claude Code (cc)"),
        "the answer lists an alias too: {text}"
    );
    assert!(!text.contains("/bin/sh"), "the answer holds no command: {text}");
    assert_eq!(app.board.panels.len(), before, "a refused preset opens no panel");
}

#[test]
fn a_directory_that_is_missing_a_file_relative_or_holds_a_control_character_opens_nothing() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    let before = app.board.panels.len();
    let work = directory(&temp, "task");
    let relative = Path::new("task");
    let file = temp.path().join("file");
    fs::write(&file, b"x").expect("the file of the test");
    let controlled = PathBuf::from(format!("{}\u{7}", work.display()));

    for (asked, expected) in [
        (temp.path().join("gone"), "directory_missing"),
        (file, "directory_missing"),
        (relative.to_path_buf(), "invalid_request"),
        (controlled, "invalid_request"),
    ] {
        let refusal = answer(&mut app, &ctx, &root, open_panel("Task shell", &asked, None));
        assert_eq!(code(&refusal), expected, "{asked:?}");
    }
    assert_eq!(app.board.panels.len(), before, "a refused directory opens no panel");
}

#[test]
fn a_preset_that_opens_no_program_of_this_computer_is_refused() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    let work = directory(&temp, "task");
    let before = app.board.panels.len();

    for preset in ["Page", "Viewer", "Build machine", "Card"] {
        let refusal = answer(&mut app, &ctx, &root, open_panel(preset, &work, None));
        assert_eq!(code(&refusal), "invalid_request", "{preset}");
        assert!(
            message(&refusal).contains("no program of this computer"),
            "the refusal says why: {preset}"
        );
    }
    assert_eq!(app.board.panels.len(), before, "a refused kind opens no panel");
}

#[test]
fn a_ninth_panel_inside_the_window_is_rate_limited_and_the_window_decides_the_wait() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    let work = directory(&temp, "task");
    for _ in 0..block_preset::MAX_PANELS_PER_OWNER {
        opened(answer(&mut app, &ctx, &root, open_panel("Task shell", &work, None)));
    }
    let refusal = answer(&mut app, &ctx, &root, open_panel("Task shell", &work, None));
    assert_eq!(code(&refusal), "rate_limited");
    assert!(message(&refusal).contains("seconds"), "the answer names the wait");

    // Another agent panel of the same workspace has its own allowance.
    let other = answer_from(&mut app, &ctx, &root, 1, open_panel("Task shell", &work, None));
    assert!(matches!(other, Outcome::Opened { .. }), "{other:?}");
}

#[test]
fn the_clock_of_the_window_gives_the_allowance_back() {
    let start = Instant::now();
    let mut panels = AgentPanels::default();
    for _ in 0..block_preset::MAX_PANELS_PER_OWNER {
        assert_eq!(panels.wait_seconds("horizon:agent", start), None);
        panels.record("horizon:agent", start);
    }
    assert_eq!(
        panels.wait_seconds("horizon:agent", start),
        Some(600),
        "the wait is the rest of the ten minutes"
    );
    assert_eq!(
        panels.wait_seconds("horizon:other", start),
        None,
        "one agent panel does not wait for the panels of another"
    );
    let late = start + Duration::from_mins(10) + Duration::from_secs(1);
    assert_eq!(
        panels.wait_seconds("horizon:agent", late),
        None,
        "the window has passed"
    );
    panels.record("horizon:agent", late);
    assert_eq!(
        panels.wait_seconds("horizon:agent", late),
        None,
        "a record inside the window keeps the allowance of the ones that left it"
    );
}

#[test]
fn the_panel_of_an_agent_of_another_workspace_lands_in_that_workspace() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    let work = directory(&temp, "task");
    let second_workspace = app.board.create_workspace("elsewhere");
    app.board.panels[1].workspace_id = second_workspace;
    let first = app.board.panels[0].workspace_id;

    let panel_id = opened(answer_from(
        &mut app,
        &ctx,
        &root,
        1,
        open_panel("Task shell", &work, None),
    ));
    let panel = app
        .board
        .panel(app.board.panel_id_by_local_id(&panel_id).expect("panel"))
        .expect("panel");
    assert_eq!(panel.workspace_id, second_workspace);
    assert_ne!(panel.workspace_id, first);
}

#[test]
fn another_host_a_gone_caller_and_an_expired_deadline_are_refused() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    let work = directory(&temp, "task");
    let before = app.board.panels.len();

    let mut wrong_host = request(&app, open_panel("Task shell", &work, None));
    wrong_host.host_instance = "another-host".into();
    assert_eq!(
        code(&app.apply_block_request_in(&wrong_host, &root, &ctx).expect("an answer")),
        "wrong_host"
    );

    let mut gone = request(&app, open_panel("Task shell", &work, None));
    gone.actor = "horizon:gone".into();
    assert_eq!(
        code(&app.apply_block_request_in(&gone, &root, &ctx).expect("an answer")),
        "caller_unavailable"
    );

    let mut expired = request(&app, open_panel("Task shell", &work, None));
    expired.deadline_at_millis = manifest::now_millis() - 1;
    assert_eq!(
        code(&app.apply_block_request_in(&expired, &root, &ctx).expect("an answer")),
        "request_expired"
    );
    assert_eq!(app.board.panels.len(), before, "a refused request opens no panel");
}

#[test]
fn the_directory_is_read_on_the_worker_and_the_frame_opens_nothing() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    let work = directory(&temp, "task");
    let before = app.board.panels.len();
    let request = request(&app, open_panel("Task shell", &work, None));

    assert!(
        app.apply_block_request_in(&request, &root, &ctx).is_none(),
        "the frame answers nothing: the directory of the request is read on the worker"
    );
    assert_eq!(app.board.panels.len(), before, "no frame opens the panel");
    assert!(app.block_claims_outstanding(), "the host owes the caller an answer");

    app.settle_browser_host_io();
    let panel_id = opened(
        block::take_result_at(&root, &request)
            .expect("readable")
            .expect("the worker answered"),
    );
    assert!(app.board.panel_id_by_local_id(&panel_id).is_some());
    assert_eq!(
        app.board.panels.len(),
        before + 1,
        "the frame that applies the answer opens the panel"
    );
}

#[test]
fn the_mark_and_the_directory_are_saved_with_the_panel() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    let work = directory(&temp, "task");
    let panel_id = opened(answer(&mut app, &ctx, &root, open_panel("Task shell", &work, None)));

    let panel = saved(&app, &panel_id);
    assert_eq!(panel.opened_by.as_deref(), Some("Claude"));
    assert_eq!(panel.cwd.as_deref(), Some(work.to_str().expect("a path")));
    let restored = panel.to_panel_options(&horizon_core::browser::BrowserConfig::default());
    assert_eq!(
        restored.opened_by,
        Some("Claude".to_string()),
        "a restored panel keeps its mark"
    );
    assert_eq!(
        restored.cwd.as_deref(),
        Some(work.as_path()),
        "a restored panel keeps its folder"
    );
}
