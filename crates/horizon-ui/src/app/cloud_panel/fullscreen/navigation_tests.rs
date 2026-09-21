use std::time::{Duration, Instant};

use egui::{Context, Event, Key, Modifiers};
use horizon_core::cloud_panel::CloudGroup;
use horizon_core::{PanelOptions, RuntimeState, StartupDecision};

use crate::app::HorizonApp;
use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app_with_startup};

fn frame(ctx: &Context, app: &mut HorizonApp, events: Vec<Event>) {
    let mut input = raw_input([1400.0, 900.0], None);
    input.events = events;
    run_app_frame_with_input(ctx, app, input);
}

fn key_event(key: Key, pressed: bool, repeat: bool) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat,
        modifiers: Modifiers::NONE,
    }
}

fn assert_palette_dismissal_preserves_terminal(ctx: &Context, app: &mut HorizonApp, received: &std::path::Path) {
    let fullscreen = app.fullscreen_panel;
    app.open_command_palette();
    frame(ctx, app, Vec::new());
    frame(ctx, app, vec![key_event(Key::Escape, true, false)]);
    frame(ctx, app, vec![key_event(Key::Escape, true, true)]);
    frame(ctx, app, vec![key_event(Key::Escape, false, false)]);
    assert!(app.command_palette.is_none());
    assert_eq!(app.fullscreen_panel, fullscreen);
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        std::fs::read(received).unwrap().is_empty(),
        "palette Escape reached the PTY"
    );
}

#[test]
fn escape_closes_the_command_palette_before_leaving_either_fullscreen_level() {
    for nested_panel in [false, true] {
        let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        app.root_viewport_stabilizer = None;
        let workspace = app.board.create_workspace("palette navigation");
        let panel = app
            .board
            .create_panel(
                PanelOptions {
                    command: Some("/bin/sh".into()),
                    ..PanelOptions::default()
                },
                workspace,
            )
            .unwrap();
        let mut cloud = CloudGroup::new(
            1,
            "Fixture".into(),
            app.board.workspace(workspace).unwrap().local_id.clone(),
            temp.path().into(),
            [0.0, 0.0],
        );
        cloud.panels.push(app.board.panel(panel).unwrap().local_id.clone());
        app.cloud_prototype.groups.0.push(cloud);
        app.board.cloud_groups = app.cloud_prototype.groups.clone();
        app.cloud_prototype.root = Some(temp.path().join("clouds"));
        app.cloud_prototype.ready = true;
        frame(&ctx, &mut app, Vec::new());
        app.toggle_cloud_fullscreen(&ctx, 1);
        app.fullscreen_panel = nested_panel.then_some(panel);
        app.open_command_palette();
        frame(&ctx, &mut app, Vec::new());
        assert!(app.command_palette.is_some());

        frame(&ctx, &mut app, vec![key_event(Key::Escape, true, false)]);
        assert!(app.command_palette.is_none(), "Escape must close the palette first");
        assert!(app.cloud_prototype.fullscreen.is_some());
        assert_eq!(app.fullscreen_panel, nested_panel.then_some(panel));

        frame(&ctx, &mut app, vec![key_event(Key::Escape, true, true)]);
        assert!(app.cloud_prototype.fullscreen.is_some(), "held Escape left the cloud");
        assert_eq!(app.fullscreen_panel, nested_panel.then_some(panel));
        frame(&ctx, &mut app, vec![key_event(Key::Escape, false, false)]);
        frame(&ctx, &mut app, vec![key_event(Key::Escape, true, false)]);
        assert!(app.fullscreen_panel.is_none());
        assert_eq!(app.cloud_prototype.fullscreen.is_some(), nested_panel);
    }
}

#[test]
fn fullscreen_navigation_never_reaches_the_pty_but_fresh_escape_does() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    let ready = temp.path().join("ready");
    let received = temp.path().join("pty-input");
    let workspace = app.board.create_workspace("navigation isolation");
    let panel = app
        .board
        .create_panel(
            PanelOptions {
                command: Some("/bin/sh".into()),
                args: vec![
                    "-c".into(),
                    "stty raw -echo; printf '\\033[>3u'; : > \"$1\"; cat > \"$2\"".into(),
                    "capture".into(),
                    ready.to_string_lossy().into(),
                    received.to_string_lossy().into(),
                ],
                ..PanelOptions::default()
            },
            workspace,
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() || !received.exists() {
        assert!(Instant::now() < deadline, "PTY capture did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app
        .board
        .panel(panel)
        .unwrap()
        .terminal()
        .unwrap()
        .mode()
        .contains(alacritty_terminal::term::TermMode::REPORT_EVENT_TYPES)
    {
        app.board.process_output();
        assert!(Instant::now() < deadline, "Kitty release reporting did not activate");
        std::thread::sleep(Duration::from_millis(10));
    }
    app.board.focus(panel);
    let local_workspace = app.board.workspace(workspace).unwrap().local_id.clone();
    let local_panel = app.board.panel(panel).unwrap().local_id.clone();
    let mut cloud = CloudGroup::new(1, "Fixture".into(), local_workspace, temp.path().into(), [0.0, 0.0]);
    cloud.panels.push(local_panel);
    app.cloud_prototype.groups.0.push(cloud);
    app.board.cloud_groups = app.cloud_prototype.groups.clone();
    app.cloud_prototype.root = Some(temp.path().join("clouds"));
    app.cloud_prototype.ready = true;
    frame(&ctx, &mut app, Vec::new());
    app.toggle_cloud_fullscreen(&ctx, 1);
    frame(&ctx, &mut app, vec![key_event(Key::F11, true, false)]);
    assert_eq!(app.fullscreen_panel, Some(panel));
    frame(&ctx, &mut app, vec![key_event(Key::F11, true, true)]);
    assert_eq!(app.fullscreen_panel, Some(panel), "held toggle must not oscillate");
    frame(&ctx, &mut app, vec![key_event(Key::F11, false, false)]);

    assert_palette_dismissal_preserves_terminal(&ctx, &mut app, &received);

    frame(&ctx, &mut app, vec![key_event(Key::Escape, true, false)]);
    assert!(app.fullscreen_panel.is_none());
    assert!(app.cloud_prototype.fullscreen.is_some());
    frame(&ctx, &mut app, vec![key_event(Key::Escape, true, true)]);
    assert!(
        app.cloud_prototype.fullscreen.is_some(),
        "held Escape must not exit two levels"
    );
    let mut unfocused = raw_input([1400.0, 900.0], None);
    unfocused.focused = false;
    unfocused.events = vec![Event::WindowFocused(false)];
    unfocused.viewports.entry(egui::ViewportId::ROOT).or_default().focused = Some(false);
    run_app_frame_with_input(&ctx, &mut app, unfocused);
    frame(
        &ctx,
        &mut app,
        vec![Event::WindowFocused(true), key_event(Key::Escape, false, false)],
    );
    frame(&ctx, &mut app, vec![key_event(Key::Escape, true, false)]);
    assert!(app.cloud_prototype.fullscreen.is_none());
    frame(&ctx, &mut app, vec![key_event(Key::Escape, true, true)]);
    frame(&ctx, &mut app, vec![key_event(Key::Escape, false, false)]);
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        std::fs::read(&received).unwrap().is_empty(),
        "navigation reached the PTY"
    );

    frame(&ctx, &mut app, vec![key_event(Key::Escape, true, false)]);
    frame(&ctx, &mut app, vec![key_event(Key::Escape, false, false)]);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if std::fs::read(&received).unwrap() == b"\x1b[27u\x1b[27;1:3u" {
            break;
        }
        assert!(Instant::now() < deadline, "fresh terminal Escape was swallowed");
        std::thread::sleep(Duration::from_millis(10));
    }
}
