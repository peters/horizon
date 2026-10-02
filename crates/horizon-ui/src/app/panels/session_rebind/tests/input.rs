use super::*;

#[test]
fn speech_press_gate_follows_the_focused_picker_viewport() {
    let (_temp, app) = crate::app::test_support::test_app();
    let ctx = Context::default();
    let detached = egui::ViewportId::from_hash_of("speech-picker");
    let mut initial = RawInput::default();
    initial.viewports.insert(
        detached,
        egui::ViewportInfo {
            focused: Some(true),
            ..Default::default()
        },
    );
    let _ = ctx
        .run_ui(initial, |_| {
            assert!(focused_session_picker_panel(&ctx).is_none());
            assert!(!app.speech_text_surface_active(&ctx).0);
        })
        .discard_textures();
    for viewport in [egui::ViewportId::ROOT, detached] {
        let mut input = RawInput {
            viewport_id: viewport,
            ..Default::default()
        };
        input.viewports.insert(viewport, egui::ViewportInfo::default());
        let _ = ctx
            .run_ui(input, |ui| {
                open_session_picker(&ui.button("Resume"), PanelId(1), Vec::new());
            })
            .discard_textures();
        let mut root_input = RawInput::default();
        root_input.viewports.insert(
            detached,
            egui::ViewportInfo {
                focused: Some(viewport == detached),
                ..Default::default()
            },
        );
        let _ = ctx
            .run_ui(root_input, |_| {
                assert_eq!(focused_session_picker_panel(&ctx), Some(PanelId(1)));
                assert!(app.speech_text_surface_active(&ctx).0);
            })
            .discard_textures();
    }
}

#[cfg(feature = "speech")]
#[test]
fn picker_blocks_capture_start_and_still_processes_an_engaged_hold_release() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = Context::default();
    let workspace = app.board.create_workspace("speech picker");
    let panel = app
        .board
        .create_panel(
            horizon_core::PanelOptions {
                kind: PanelKind::Editor,
                ..Default::default()
            },
            workspace,
        )
        .expect("panel");
    app.board.focus(panel);
    let (speech, channels) = crate::app::speech::SpeechSystem::with_test_bindings(&["F1"]);
    app.speech = Some(speech);
    let _ = ctx
        .run_ui(RawInput::default(), |ui| {
            open_session_picker(&ui.button("Resume"), panel, Vec::new());
        })
        .discard_textures();
    let input = |pressed| {
        let mut input = RawInput {
            events: vec![Event::Key {
                key: Key::F1,
                physical_key: Some(Key::F1),
                pressed,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            ..Default::default()
        };
        input.viewports.get_mut(&egui::ViewportId::ROOT).expect("root").focused = Some(true);
        input
    };
    let _ = ctx
        .run_ui(input(true), |ui| {
            app.handle_speech_input(&ctx);
            open_session_picker(&ui.button("Resume"), panel, Vec::new());
        })
        .discard_textures();
    assert!(!channels.capture_start_requested());
    assert!(app.speech.as_ref().expect("speech").recording_sink().is_none());
    app.speech
        .as_mut()
        .expect("speech")
        .start(crate::app::speech::SpeechSink::Panel(panel), 0);
    app.speech_engaged_profile = Some(0);
    assert!(channels.capture_start_requested());
    let _ = ctx
        .run_ui(input(false), |_| {
            app.handle_speech_input(&ctx);
        })
        .discard_textures();
    assert!(app.speech.as_ref().expect("speech").recording_sink().is_none());
    assert!(app.speech_engaged_profile.is_none());
}

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

#[test]
fn root_picker_clears_stale_drop_state_on_opening_and_blocked_frames() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = Context::default();
    let workspace = app.board.create_workspace("drop isolation");
    let panel = app
        .board
        .create_panel(
            horizon_core::PanelOptions {
                kind: PanelKind::Editor,
                ..Default::default()
            },
            workspace,
        )
        .expect("panel");
    let detached = egui::ViewportId::from_hash_of("other-drop");
    for opening in [true, false] {
        app.file_drop_highlight = Some(crate::app::file_drop::FileDropHighlight::Panel(panel));
        app.file_hover_positions
            .insert(egui::ViewportId::ROOT, Pos2::new(10.0, 20.0));
        app.file_hover_positions.insert(detached, Pos2::new(30.0, 40.0));
        let _ = ctx
            .run_ui(RawInput::default(), |ui| {
                if opening {
                    open_session_picker(&ui.button("Resume"), panel, Vec::new());
                    app.render_file_drop_highlight(&ctx);
                } else {
                    app.process_frame_inputs(&ctx);
                }
            })
            .discard_textures();
        assert!(app.file_drop_highlight.is_none());
        assert!(!app.file_hover_positions.contains_key(&egui::ViewportId::ROOT));
        assert!(app.file_hover_positions.contains_key(&detached));
    }
}
