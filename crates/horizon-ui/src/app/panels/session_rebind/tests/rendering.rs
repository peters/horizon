use super::*;

#[test]
fn panel_rendering_keeps_the_picker_visible_while_its_body_is_suppressed() {
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
    for frame_index in 0..3 {
        let output = ctx
            .run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0))),
                    ..Default::default()
                },
                |ui| {
                    if frame_index == 0 {
                        let response = ui.button("Launch picker");
                        open_session_picker(
                            &response,
                            panel,
                            vec![AgentSessionBinding::new(
                                PanelKind::Codex,
                                "exact-session-id".into(),
                                None,
                                Some("Recovery test".into()),
                                None,
                            )],
                        );
                    }
                    app.handle_canvas_pan(&ctx);
                    app.render_panels(ui);
                },
            )
            .discard_textures();
        if frame_index > 0 {
            assert!(
                text_center(&output, "Resume a session").is_some(),
                "picker visible in panel frame {frame_index}"
            );
        }
    }
}

#[test]
fn count_and_paging_include_sessions_beyond_the_first_eight() {
    let ctx = Context::default();
    let options: Vec<_> = (1..=16)
        .map(|index| {
            AgentSessionBinding::new(
                PanelKind::Codex,
                format!("session-{index}"),
                None,
                Some(format!("Conversation {index}")),
                None,
            )
        })
        .collect();
    let run = |events| {
        let mut outcome = None;
        let output = ctx
            .run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 900.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    outcome = Some(render_options(ui, &options, false));
                },
            )
            .discard_textures();
        (output, outcome.expect("options rendered"))
    };
    let (output, outcome) = run(Vec::new());
    assert_eq!(outcome.option_rects.len(), 8);
    assert!(text_center(&output, "16 sessions available · Newest first").is_some());
    let first_top = outcome.option_rects[0].top();
    run(vec![
        Event::PointerMoved(Pos2::new(250.0, 350.0)),
        Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, -600.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        },
    ]);
    for _ in 0..20 {
        run(Vec::new());
    }
    let (output, scrolled) = run(Vec::new());
    assert!(scrolled.option_rects[0].top() < first_top - 20.0);
    let at = text_center(&output, "Next").expect("next page available");
    for pressed in [true, false] {
        run(vec![
            Event::PointerMoved(at),
            Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            },
        ]);
    }
    let (output, outcome) = run(Vec::new());
    assert_eq!(outcome.option_rects.len(), 8);
    assert!((outcome.option_rects[0].top() - first_top).abs() < 1.0);
    assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::epaint::Shape::Text(text) if text.galley.job.text.contains("Conversation 9"))));
    assert!(text_center(&output, "9–16 of 16").is_some());
}

#[test]
fn keyboard_copy_keeps_the_picker_open() {
    let ctx = Context::default();
    frame(&ctx, Vec::new(), true);
    frame(&ctx, Vec::new(), false);
    let key = |key| Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    frame(&ctx, vec![key(Key::Tab)], false);
    let copied = frame(&ctx, vec![key(Key::Enter)], false);
    assert!(
        copied
            .platform_output
            .commands
            .iter()
            .any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "exact-session-id"))
    );
    assert_eq!(session_picker_panel(&ctx), Some(PanelId(1)));
}

#[test]
fn recovery_picker_outlives_parent_menu_and_copy_keeps_it_open() {
    let ctx = Context::default();
    frame(&ctx, Vec::new(), true);
    let visible = frame(&ctx, Vec::new(), false);
    assert!(!egui::Popup::is_id_open(&ctx, Id::new("recovery_parent_menu")));
    assert!(text_center(&visible, "Resume a session").is_some());
    assert_eq!(session_picker_panel(&ctx), Some(PanelId(1)));
    let at = text_center(&visible, "Copy ID").expect("copy action visible after parent closed");
    frame(
        &ctx,
        vec![
            Event::PointerMoved(at),
            Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
        false,
    );
    let copied = frame(
        &ctx,
        vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
        false,
    );
    assert!(
        copied
            .platform_output
            .commands
            .iter()
            .any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "exact-session-id"))
    );
    assert!(text_center(&frame(&ctx, Vec::new(), false), "Resume a session").is_some());
    frame(
        &ctx,
        vec![Event::Key {
            key: Key::Escape,
            physical_key: Some(Key::Escape),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
        false,
    );
    assert!(text_center(&frame(&ctx, Vec::new(), false), "Resume a session").is_none());
    assert!(session_picker_panel(&ctx).is_none());
}

#[test]
fn every_viewport_reconciles_deleted_bindings_before_offering_resume() {
    let ctx = Context::default();
    let a = AgentSessionBinding::new(PanelKind::Claude, "kept".into(), Some("/sample".into()), None, None);
    let b = AgentSessionBinding::new(PanelKind::Claude, "deleted".into(), Some("/sample".into()), None, None);
    let detached = egui::ViewportId::from_hash_of("detached-recovery");
    for (viewport, panel_id) in [(egui::ViewportId::ROOT, PanelId(1)), (detached, PanelId(2))] {
        let mut input = RawInput {
            viewport_id: viewport,
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 900.0))),
            ..Default::default()
        };
        input.viewports.insert(viewport, egui::ViewportInfo::default());
        let _ = ctx
            .run_ui(input, |ui| {
                open_session_picker(&ui.button("Open picker"), panel_id, vec![a.clone(), b.clone()]);
            })
            .discard_textures();
    }
    let report = horizon_core::AgentSessionDeletionReport {
        deleted: vec![horizon_core::AgentSessionKey::new(b.kind, &b.session_id)],
        ..Default::default()
    };
    finish_session_deletion(&ctx, PanelId(1), egui::ViewportId::ROOT, vec![a.clone()], &report);
    for (viewport, panel_id) in [(detached, PanelId(2)), (egui::ViewportId::ROOT, PanelId(1))] {
        let mut input = RawInput {
            viewport_id: viewport,
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 900.0))),
            ..Default::default()
        };
        input.viewports.insert(viewport, egui::ViewportInfo::default());
        let _ = ctx
            .run_ui(input.clone(), |_| {
                assert!(render_session_picker(&ctx, panel_id, vec![a.clone()]).is_none());
            })
            .discard_textures();
        let output = ctx
            .run_ui(input, |_| {
                assert!(render_session_picker(&ctx, panel_id, vec![a.clone()]).is_none());
            })
            .discard_textures();
        assert!(
            text_center(&output, "1 session available · Newest first").is_some(),
            "viewport {viewport:?}, shapes {:?}",
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        );
        let state = ctx
            .data(|data| data.get_temp::<SessionPicker>(Id::new(("session_recovery_picker", viewport))))
            .expect("picker");
        assert_eq!(state.options.as_ref(), std::slice::from_ref(&a));
        assert!(!state.options.iter().any(|binding| binding.session_id == b.session_id));
    }
}
