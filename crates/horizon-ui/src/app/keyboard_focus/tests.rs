//! The keyboard that a person's action gives to one panel.
//!
//! A frame draws the panel that the person asked for and gives its body the focus
//! of the window, so the next key reaches its program. A terminal that an agent
//! opens never asks, so the keyboard stays where the person had it.

use egui::{Context, Id, RawInput};
use horizon_core::browser::manifest::block;
use horizon_core::{PanelId, PanelKind, PanelOptions, RuntimeState, StartupDecision, WorkspaceId};
use tempfile::TempDir;

use crate::app::HorizonApp;
use crate::app::test_support::{editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup};
use crate::test_egui::DiscardTextures;

/// One app with one command panel that runs `cat`, so its program stays.
fn app_with_one_panel() -> (TempDir, Context, HorizonApp, PanelId, WorkspaceId) {
    let runtime = RuntimeState {
        workspaces: vec![editor_workspace_state("first", [0.0, 0.0])],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    app.theme_applied = true;
    app.initial_pan_done = true;
    app.root_viewport_stabilizer = None;
    let workspace = app.board.ensure_workspace();
    let panel = app
        .create_panel_with_options(
            PanelOptions {
                kind: PanelKind::Command,
                command: Some("/bin/sh".to_owned()),
                args: vec!["-c".to_owned(), "cat".to_owned()],
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("the panel of the test");
    app.settle_panel_starts();
    (temp, ctx, app, panel, workspace)
}

/// One frame of the whole window, with `events` as its input.
fn frame(ctx: &Context, app: &mut HorizonApp, time: f64, events: Vec<egui::Event>) {
    let mut input: RawInput = raw_input([1400.0, 900.0], None);
    input.time = Some(time);
    input.events = events;
    let _ = run_app_frame_with_input(ctx, app, input);
}

/// One frame in which a widget of the window holds the keyboard, as a field of
/// the sidebar does while a person types in it.
fn frame_while_another_widget_holds_the_keyboard(ctx: &Context, app: &mut HorizonApp, time: f64) {
    let mut input: RawInput = raw_input([1400.0, 900.0], None);
    input.time = Some(time);
    let held = Id::unique("a field of the sidebar of the test");
    let _ = ctx
        .run_ui(input, |ui| {
            ui.memory_mut(|memory| memory.request_focus(held));
            app.render_active_view(ui, true);
        })
        .discard_textures();
}

/// Whether the program of `panel` holds the keyboard of the window.
fn has_the_keyboard(app: &HorizonApp, panel: PanelId) -> bool {
    app.board
        .panel(panel)
        .and_then(horizon_core::Panel::terminal)
        .is_some_and(horizon_core::Terminal::has_keyboard_focus)
}

/// K1: a person's panel that is created or revealed takes the keyboard, even when
/// another widget of the window held it.
#[test]
fn a_person_action_that_reveals_a_panel_gives_it_the_keyboard() {
    let (_temp, ctx, mut app, first, workspace) = app_with_one_panel();
    frame(&ctx, &mut app, 0.0, Vec::new());
    assert!(
        has_the_keyboard(&app, first),
        "the only panel of the window holds the keyboard"
    );

    let second = app
        .create_panel_with_options(
            PanelOptions {
                kind: PanelKind::Command,
                command: Some("/bin/sh".to_owned()),
                args: vec!["-c".to_owned(), "cat".to_owned()],
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("the second panel of the test");
    app.settle_panel_starts();
    frame_while_another_widget_holds_the_keyboard(&ctx, &mut app, 0.016);
    assert!(
        !has_the_keyboard(&app, second),
        "a panel that no action revealed takes no keyboard"
    );

    app.reveal_new_panel(&ctx, workspace, second);
    frame_while_another_widget_holds_the_keyboard(&ctx, &mut app, 0.032);

    assert!(has_the_keyboard(&app, second), "the revealed panel takes the keyboard");
    assert!(
        !has_the_keyboard(&app, first),
        "the panel that the person left gives the keyboard away"
    );
    assert!(
        !app.keyboard_focus_requested(second),
        "the frame that drew the panel answered the request"
    );
}

/// K2: a terminal that an agent opens keeps the keyboard where the person had
/// it.
#[test]
fn a_terminal_that_an_agent_opens_does_not_take_the_keyboard() {
    use horizon_browser_control::manifest::block_terminal::TerminalRoute;
    use horizon_core::browser::manifest;

    let (_temp, ctx, mut app, person, _workspace) = app_with_one_panel();
    app.board.panels[0].kind = PanelKind::Claude;
    frame(&ctx, &mut app, 0.0, Vec::new());
    assert!(
        has_the_keyboard(&app, person),
        "the panel of the person holds the keyboard"
    );

    let request = block::Request {
        request_id: "test".into(),
        actor: format!("horizon:{}", app.board.panels[0].local_id),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation: block::Operation::OpenTerminal {
            ssh: Some(TerminalRoute {
                host: "lab-1.invalid".into(),
                user: Some("deploy".into()),
                port: Some(2222),
            }),
            remote_command: None,
            title: None,
            key: None,
            machine: None,
        },
    };
    let _ = app.apply_block_request(&request, &ctx);

    frame(&ctx, &mut app, 0.016, Vec::new());

    assert!(
        has_the_keyboard(&app, person),
        "the terminal of the agent leaves the keyboard with the person"
    );
    let opened = app
        .board
        .panels
        .iter()
        .find(|panel| panel.opened_by().is_some())
        .map(|panel| panel.id)
        .expect("the terminal of the agent");
    assert!(
        !has_the_keyboard(&app, opened),
        "the terminal of the agent takes no keyboard"
    );
    assert!(
        app.board.focused == Some(person),
        "the board keeps its focus on the panel of the person"
    );
}
