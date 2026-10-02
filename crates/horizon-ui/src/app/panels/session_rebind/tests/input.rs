use super::*;

#[test]
fn picker_releases_input_when_its_owner_stops_rendering() {
    let ctx = Context::default();
    frame(&ctx, Vec::new(), true);
    assert_eq!(session_picker_panel(&ctx), Some(PanelId(1)));
    for _ in 0..2 {
        let _ = ctx.run_ui(RawInput::default(), |_| {}).discard_textures();
    }
    assert!(session_picker_panel(&ctx).is_none());
}

#[test]
fn picker_blocks_root_shortcuts_and_restores_them_after_expiry() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = Context::default();
    let workspace = app.board.create_workspace("recovery");
    let panel = app
        .board
        .create_panel(
            horizon_core::PanelOptions {
                kind: PanelKind::Editor,
                ..Default::default()
            },
            workspace,
        )
        .expect("editor panel");
    app.shortcuts.toggle_settings = horizon_core::ShortcutBinding::parse("F1").expect("shortcut");
    app.shortcuts.command_palette = horizon_core::ShortcutBinding::parse("F2").expect("shortcut");
    for key in [Key::F1, Key::F2] {
        let _ = ctx
            .run_ui(RawInput::default(), |ui| {
                open_session_picker(&ui.button("Resume"), panel, Vec::new());
            })
            .discard_textures();
        let _ = ctx
            .run_ui(
                RawInput {
                    events: vec![Event::Key {
                        key,
                        physical_key: Some(key),
                        pressed: true,
                        repeat: false,
                        modifiers: Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |_| {
                    app.process_frame_inputs(&ctx);
                },
            )
            .discard_textures();
        assert!(app.settings.is_none());
        assert!(app.command_palette.is_none());
    }
    for _ in 0..2 {
        let _ = ctx.run_ui(RawInput::default(), |_| {}).discard_textures();
    }
    let _ = ctx
        .run_ui(
            RawInput {
                events: vec![Event::Key {
                    key: Key::F1,
                    physical_key: Some(Key::F1),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                }],
                ..Default::default()
            },
            |_| {
                app.process_frame_inputs(&ctx);
            },
        )
        .discard_textures();
    assert!(app.settings.is_some());
}

#[test]
fn picker_pointer_input_bypasses_canvas_gesture_reservation() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = Context::default();
    let workspace = app.board.create_workspace("recovery");
    let panel = app
        .board
        .create_panel(
            horizon_core::PanelOptions {
                kind: PanelKind::Editor,
                ..Default::default()
            },
            workspace,
        )
        .expect("editor panel");
    app.root_viewport_stabilizer = None;
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0))),
        ..Default::default()
    };
    let _ = ctx
        .run_ui(input(), |ui| {
            open_session_picker(&ui.button("Resume"), panel, Vec::new());
        })
        .discard_textures();
    let pos = Pos2::new(500.0, 400.0);
    let events = vec![
        Event::PointerMoved(pos),
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::CTRL,
        },
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::CTRL,
        },
    ];
    let mut raw = RawInput {
        events: events.clone(),
        ..input()
    };
    app.filter_canvas_gesture(&ctx, &mut raw);
    assert_eq!(raw.events, events);
    assert!(app.canvas_gesture.take_completed().is_none());
    for _ in 0..2 {
        let _ = ctx.run_ui(input(), |_| {}).discard_textures();
    }
    raw.events = events.clone();
    app.filter_canvas_gesture(&ctx, &mut raw);
    assert_ne!(raw.events, events, "canvas reservation resumes after picker expiry");
}

#[test]
fn picker_clicks_do_not_focus_underlying_panels() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = Context::default();
    let workspace = app.board.create_workspace("recovery");
    let mut create = |position| {
        app.board
            .create_panel(
                horizon_core::PanelOptions {
                    kind: PanelKind::Editor,
                    position: Some(position),
                    size: Some([300.0, 200.0]),
                    ..Default::default()
                },
                workspace,
            )
            .expect("editor panel")
    };
    let owner = create([20.0, 100.0]);
    let other = create([600.0, 100.0]);
    app.board.focus(owner);
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0))),
        ..Default::default()
    };
    let _ = ctx
        .run_ui(input(), |ui| {
            open_session_picker(&ui.button("Resume"), owner, Vec::new());
        })
        .discard_textures();
    let pos = app
        .visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None)
        .into_iter()
        .find(|(id, _)| *id == other)
        .expect("other panel geometry")
        .1
        .screen_rect
        .center();
    let click = || RawInput {
        events: vec![Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }],
        ..input()
    };
    let _ = ctx
        .run_ui(click(), |_| {
            app.process_frame_inputs(&ctx);
        })
        .discard_textures();
    assert_eq!(app.board.focused, Some(owner));
    for _ in 0..2 {
        let _ = ctx.run_ui(input(), |_| {}).discard_textures();
    }
    let _ = ctx
        .run_ui(click(), |_| {
            app.process_frame_inputs(&ctx);
        })
        .discard_textures();
    assert_eq!(app.board.focused, Some(other));
}
