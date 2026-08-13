use super::*;

#[test]
fn startup_organization_is_disabled_by_default() {
    let runtime_state = two_workspace_runtime(Some(CanvasViewState::new([24.0, -12.0], 1.0)));
    let expected_left = runtime_state.workspaces[0].position.expect("left position");
    let expected_right = runtime_state.workspaces[1].position.expect("right position");
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime_state),
    });
    app.theme_applied = true;

    run_frame_at_configured_size(&ctx, &mut app);

    assert_position_near(workspace_position(&app, "left"), expected_left);
    assert_position_near(workspace_position(&app, "right"), expected_right);
    assert!(app.runtime_dirty_since.is_none());
}

#[test]
fn startup_organization_waits_for_async_board_restore() {
    let (_temp, ctx, mut app) = enabled_test_app(RuntimeState::default());
    let (bootstrap_tx, bootstrap_rx) = std::sync::mpsc::channel();
    app.startup_receiver = Some(bootstrap_rx);
    app.theme_applied = true;

    run_app_frame(&ctx, &mut app);
    assert!(app.board.workspaces.is_empty());

    bootstrap_tx
        .send(StartupBootstrapOutcome::Ready(Box::new(StartupBootstrap {
            runtime_state: two_workspace_runtime(Some(CanvasViewState::default())),
            session_catalog: AgentSessionCatalog::default(),
            runtime_state_changed: false,
        })))
        .expect("startup bootstrap");
    run_frame_at_configured_size(&ctx, &mut app);

    assert_horizontal_row(&app, "left", "right");
}

#[test]
fn startup_organization_waits_for_session_choice_and_activation() {
    let chooser = StartupChooser {
        reason: StartupPromptReason::MultipleRecoverable,
        config_path: "/tmp/horizon-config.yaml".to_string(),
        sessions: Vec::new(),
    };
    let (_temp, ctx, mut app) = test_app_with_config_and_startup(&enabled_config(), StartupDecision::Choose(chooser));
    app.theme_applied = true;

    run_app_frame(&ctx, &mut app);
    assert!(app.startup_chooser.is_some());
    assert!(app.board.workspaces.is_empty());

    app.activate_ephemeral_session(&two_workspace_runtime(Some(CanvasViewState::default())));
    run_frame_at_configured_size(&ctx, &mut app);

    assert!(app.startup_chooser.is_none());
    assert_horizontal_row(&app, "left", "right");
}

#[test]
fn loading_another_runtime_state_rearms_startup_organization() {
    let (_temp, ctx, mut app) = enabled_test_app(two_workspace_runtime(Some(CanvasViewState::default())));
    app.theme_applied = true;
    run_frame_at_configured_size(&ctx, &mut app);

    let second_runtime = RuntimeState {
        canvas_view: Some(CanvasViewState::default()),
        workspaces: vec![
            editor_workspace_state("second-left", [50.0, 120.0]),
            editor_workspace_state("second-right", [850.0, 640.0]),
        ],
        ..RuntimeState::default()
    };
    app.activate_ephemeral_session(&second_runtime);
    run_frame_at_configured_size(&ctx, &mut app);

    assert_horizontal_row(&app, "second-left", "second-right");
}

#[test]
fn config_enablement_takes_effect_on_the_next_loaded_session() {
    let runtime_state = two_workspace_runtime(Some(CanvasViewState::default()));
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime_state.clone()),
    });
    app.theme_applied = true;
    run_frame_at_configured_size(&ctx, &mut app);

    app.apply_runtime_config(&enabled_config());
    run_frame_at_configured_size(&ctx, &mut app);
    assert!((workspace_position(&app, "left")[1] - workspace_position(&app, "right")[1]).abs() > 1.0);

    app.activate_ephemeral_session(&runtime_state);
    run_frame_at_configured_size(&ctx, &mut app);
    assert_horizontal_row(&app, "left", "right");
}
