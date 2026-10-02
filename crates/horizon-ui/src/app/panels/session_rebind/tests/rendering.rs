use super::*;

#[test]
fn modal_blocks_the_tooltip_layer_used_by_root_and_workspace_toolbars() {
    let ctx = Context::default();
    let mut activated = false;
    let mut run = |events, launch| {
        ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0))),
                events,
                ..Default::default()
            },
            |ui| {
                egui::Area::new(Id::new("underlying_toolbar"))
                    .order(egui::Order::Tooltip)
                    .fixed_pos(Pos2::new(20.0, 20.0))
                    .show(&ctx, |ui| {
                        activated |= ui.button("Underlying toolbar").clicked();
                    });
                if launch {
                    open_session_picker(&ui.button("Resume"), PanelId(1), Vec::new());
                }
                render_session_picker(&ctx, PanelId(1), Vec::new());
            },
        )
        .discard_textures()
    };
    run(Vec::new(), true);
    let output = run(Vec::new(), false);
    let at = text_center(&output, "Underlying toolbar").expect("toolbar drawn");
    for pressed in [true, false] {
        run(
            vec![
                Event::PointerMoved(at),
                Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
            false,
        );
    }
    assert!(!activated, "the dismissing click cannot activate the toolbar");
    assert!(session_picker_panel(&ctx).is_none());
}

#[test]
fn idle_picker_requests_a_provider_refresh_without_recent_panel_output() {
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
        .expect("idle panel");
    app.session_catalog_refresh.last_full_refresh =
        std::time::Instant::now().checked_sub(std::time::Duration::from_secs(3));
    app.maybe_refresh_session_catalog();
    assert!(app.session_catalog_refresh.receiver.is_none());
    let _ = ctx
        .run_ui(RawInput::default(), |ui| {
            open_session_picker(&ui.button("Resume"), panel, Vec::new());
            assert!(app.render_saved_session_picker(&ctx, panel).is_none());
        })
        .discard_textures();
    assert!(
        app.session_catalog_refresh.receiver.is_some(),
        "an open picker refreshes independently of panel output"
    );
    assert_eq!(app.session_catalog_refresh.provider, Some(PanelKind::Editor));
}

#[test]
fn outside_click_dismisses_picker_without_closing_the_underlying_panel() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = Context::default();
    let workspace = app.board.create_workspace("recovery");
    let panel = app
        .board
        .create_panel(
            horizon_core::PanelOptions {
                kind: PanelKind::Editor,
                position: Some([20.0, 100.0]),
                size: Some([300.0, 200.0]),
                ..Default::default()
            },
            workspace,
        )
        .expect("editor panel");
    let raw = || RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0))),
        ..Default::default()
    };
    for index in 0..3 {
        let _ = ctx
            .run_ui(raw(), |ui| {
                if index == 0 {
                    open_session_picker(&ui.button("Resume"), panel, Vec::new());
                }
                app.render_panels(ui);
            })
            .discard_textures();
    }
    let geometry = app
        .visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None)
        .into_iter()
        .find(|(id, _)| *id == panel)
        .expect("panel geometry")
        .1;
    let at = crate::app::panels::PanelFrame::new(geometry.screen_rect).close.center();
    for pressed in [true, false] {
        let _ = ctx
            .run_ui(
                RawInput {
                    events: vec![
                        Event::PointerMoved(at),
                        Event::PointerButton {
                            pos: at,
                            button: PointerButton::Primary,
                            pressed,
                            modifiers: Modifiers::NONE,
                        },
                    ],
                    ..raw()
                },
                |ui| app.render_panels(ui),
            )
            .discard_textures();
    }
    assert!(app.panels_to_close.is_empty());
    assert!(app.board.panel(panel).is_some());
    assert!(session_picker_panel(&ctx).is_none(), "only the picker dismisses");
}

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
fn metadata_refresh_preserves_pagination_but_identity_scope_changes_reset_it() {
    let ctx = Context::default();
    let mut options: Vec<_> = (1..=16)
        .map(|index| {
            AgentSessionBinding::new(
                PanelKind::Codex,
                format!("session-{index}"),
                Some("/example".into()),
                Some(format!("Conversation {index}")),
                None,
            )
        })
        .collect();
    options[0] = options[1].clone();
    let run = |options: &[AgentSessionBinding], events, launch| {
        ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 900.0))),
                events,
                ..Default::default()
            },
            |ui| {
                if launch {
                    open_session_picker(&ui.button("Resume"), PanelId(1), options.to_vec());
                }
                assert!(render_session_picker(&ctx, PanelId(1), options.to_vec()).is_none());
            },
        )
        .discard_textures()
    };
    run(&options, Vec::new(), true);
    let output = run(&options, Vec::new(), false);
    let next = text_center(&output, "Next").expect("next page");
    for pressed in [true, false] {
        run(
            &options,
            vec![
                Event::PointerMoved(next),
                Event::PointerButton {
                    pos: next,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
            false,
        );
    }
    assert!(text_center(&run(&options, Vec::new(), false), "9–16 of 16").is_some());
    for binding in &mut options {
        binding.updated_at = Some(42);
        binding.label = Some("Updated synthetic title".into());
    }
    options.swap(0, 1);
    run(&options, Vec::new(), false);
    let updated = run(&options, Vec::new(), false);
    assert!(text_center(&updated, "9–16 of 16").is_some());
    assert!(updated.shapes.iter().any(|shape| {
        matches!(&shape.shape, egui::epaint::Shape::Text(text) if text.galley.job.text.starts_with("Updated synthetic title\n"))
    }));
    options[0].session_id = "newly-added-session".into();
    assert!(text_center(&run(&options, Vec::new(), false), "1–8 of 16").is_some());
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

#[test]
fn picker_provider_refresh_is_not_throttled_by_another_provider() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let ctx = Context::default();
    app.session_catalog_refresh.last_full_refresh = Some(std::time::Instant::now());
    app.session_catalog_refresh
        .picker_times
        .insert(PanelKind::Codex, std::time::Instant::now());
    app.refresh_session_catalog_for_picker(&ctx, PanelKind::Claude);
    assert_eq!(app.session_catalog_refresh.provider, Some(PanelKind::Claude));
    assert!(app.session_catalog_refresh.receiver.is_some());
}

#[test]
fn provider_scan_completion_tracks_only_its_provider() {
    let (_temp, mut app) = crate::app::test_support::test_app();
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(Ok(horizon_core::AgentSessionCatalog::default()))
        .expect("send provider scan");
    app.session_catalog_refresh.receiver = Some(rx);
    app.session_catalog_refresh.provider = Some(PanelKind::Claude);
    app.maybe_refresh_session_catalog();
    assert!(
        app.session_catalog_refresh
            .picker_times
            .contains_key(&PanelKind::Claude)
    );
    assert!(!app.session_catalog_refresh.picker_times.contains_key(&PanelKind::Codex));
    assert!(app.session_catalog_refresh.provider.is_none());
}

#[test]
fn wrapped_cards_keep_all_metadata_visible_and_delete_actions_identifiable() {
    for width in [300.0, 440.0] {
        let ctx = Context::default();
        ctx.enable_accesskit();
        let binding = AgentSessionBinding::new(
            PanelKind::Claude,
            "00000000-0000-0000-0000-000000000123".into(),
            None,
            Some("W".repeat(60)),
            Some(1_700_000_000),
        );
        let mut outcome = SessionRebindRenderOutcome::default();
        let output = ctx
            .run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 900.0))),
                    ..Default::default()
                },
                |ui| render_session_row(ui, &binding, false, &mut SessionDeletionUi::default(), &mut outcome),
            )
            .discard_textures();
        let button = outcome.option_rects[0];
        let card_text = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.job.text.contains(&binding.session_id) => {
                    Some((shape.clip_rect, text))
                }
                _ => None,
            })
            .expect("full metadata galley");
        let bounds = Rect::from_min_size(card_text.1.pos, card_text.1.galley.size());
        assert!(
            button.expand(0.5).contains_rect(bounds),
            "button {button:?}, text {bounds:?}"
        );
        assert!(card_text.0.contains_rect(bounds), "metadata must not be clipped");
        assert!(button.height() > 78.0, "wrapped title grows the row");
        let label = format!("Delete conversation {}", binding.session_id);
        assert!(
            output
                .platform_output
                .accesskit_update
                .expect("accessibility update")
                .nodes
                .iter()
                .any(|(_, node)| node.label() == Some(label.as_str()))
        );
    }
}
