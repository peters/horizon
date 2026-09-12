use super::*;
use horizon_core::remote_worker_status::{RemotePanelObservation, RemotePanelStatus};

fn loaded(store: &CloudWorkflowStore, scope: &RequestScope, ctx: &Context) -> ReopenState {
    let mut state = ReopenState::default();
    let catalog = RemoteViewCatalog::load(store, &scope.owner, &scope.expected).expect("catalog");
    let tx = pending(&mut state, scope, ctx);
    assert!(
        tx.send(Ok(Completion::Catalog(Box::new(catalog)))).is_ok(),
        "catalog result"
    );
    let home = HorizonHome::from_root("/unused-test-home".into());
    state.drain(&client(&home, scope), &mut Board::new(), ctx);
    state
}

fn observation(status: RemotePanelStatus) -> Completion {
    Completion::Inspection(RemotePanelObservation {
        panel_id: "task-0".into(),
        status,
        observed_at_millis: 1_700_000_000_000,
    })
}

fn text(state: &ReopenState, ctx: &Context) -> String {
    let output = ctx
        .run_ui(raw_input([1000.0, 900.0], None), |ui| {
            super::super::paint::show(ui, state, true, &mut InventoryAction::None);
        })
        .discard_textures();
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn queue(state: &mut ReopenState, scope: &RequestScope, ctx: &Context) -> mpsc::SyncSender<Result<Completion, String>> {
    let tx = pending(state, scope, ctx);
    state.pending.as_mut().expect("pending").inspection = Some("task-0".into());
    tx
}

#[test]
fn zero_view_results_preserve_board_runtime_and_show_unknown_completion_honestly() {
    let (_temp, mut app, store, scope) = app_fixture();
    let ctx = Context::default();
    let before = store
        .load_remote_workspace(&scope.owner, "environment")
        .expect("record");
    let saved = app
        .session_store
        .resume_session(&scope.owner)
        .expect("session")
        .runtime_state
        .to_yaml()
        .expect("yaml");
    app.runtime_dirty_since = None;
    app.remote_environments.reopen = loaded(&store, &scope, &ctx);
    for (status, expected) in [
        (RemotePanelStatus::Running { pid: 42 }, "Running at check (PID 42)"),
        (
            RemotePanelStatus::Exited {
                pid: 42,
                exit_status: None,
            },
            "exit status unknown",
        ),
        (
            RemotePanelStatus::Exited {
                pid: 42,
                exit_status: Some(7),
            },
            "exit status 7",
        ),
        (RemotePanelStatus::Unavailable, "Retained task unavailable at check"),
    ] {
        assert!(
            queue(&mut app.remote_environments.reopen, &scope, &ctx)
                .send(Ok(observation(status)))
                .is_ok(),
            "result"
        );
        app.remote_reopen_action(InventoryAction::None, &ctx);
        let text = text(&app.remote_environments.reopen, &ctx);
        assert!(text.contains(expected) && text.contains("2023-11-14") && text.contains("not monitoring"));
        assert!(!text.contains("private-") && app.board.panels.is_empty() && app.board.workspaces.is_empty());
        assert!(app.runtime_dirty_since.is_none());
    }
    assert_eq!(
        store
            .load_remote_workspace(&scope.owner, "environment")
            .expect("record"),
        before
    );
    assert_eq!(
        app.session_store
            .resume_session(&scope.owner)
            .expect("session")
            .runtime_state
            .to_yaml()
            .expect("yaml"),
        saved
    );
}

#[test]
fn failed_or_wrong_panel_results_keep_prior_observation_explicitly_stale_and_retryable() {
    let (_temp, mut app, store, scope) = app_fixture();
    let ctx = Context::default();
    app.remote_environments.reopen = loaded(&store, &scope, &ctx);
    app.remote_environments
        .reopen
        .accept_inspection("task-0", Ok(observation(RemotePanelStatus::Running { pid: 42 })));
    let mut wrong = RemotePanelObservation {
        panel_id: "wrong".into(),
        status: RemotePanelStatus::Unavailable,
        observed_at_millis: 2,
    };
    for result in [
        Err("Task check failed; retry now.".into()),
        Ok(Completion::Inspection(wrong.clone())),
    ] {
        assert!(
            queue(&mut app.remote_environments.reopen, &scope, &ctx)
                .send(result)
                .is_ok(),
            "failure"
        );
        app.remote_reopen_action(InventoryAction::None, &ctx);
        let text = text(&app.remote_environments.reopen, &ctx);
        assert!(text.contains("Running at check") && text.contains("previous observation may be stale"));
        assert!(!app.remote_environments.reopen.is_pending());
    }
    wrong.panel_id = "task-0".into();
    assert!(
        queue(&mut app.remote_environments.reopen, &scope, &ctx)
            .send(Ok(Completion::Inspection(wrong)))
            .is_ok(),
        "retry"
    );
    app.remote_reopen_action(InventoryAction::None, &ctx);
    assert!(!text(&app.remote_environments.reopen, &ctx).contains("may be stale"));
}

#[test]
fn disappeared_database_is_not_recreated_by_task_catalog_or_inventory_reads() {
    let (temp, mut app, store, scope) = app_fixture();
    let ctx = Context::default();
    let home = app.session_store.home().clone();
    app.remote_environments.reopen = loaded(&store, &scope, &ctx);
    app.remote_environments
        .reopen
        .accept_inspection("task-0", Ok(observation(RemotePanelStatus::Running { pid: 42 })));
    let saved = app
        .session_store
        .resume_session(&scope.owner)
        .expect("session")
        .runtime_state
        .to_yaml()
        .expect("runtime YAML");
    app.runtime_dirty_since = None;
    let directory = store.path().parent().expect("database directory");
    let retained = temp.path().join("retained-cloud-store");
    let database = std::fs::read(store.path()).expect("database bytes");
    std::fs::rename(directory, &retained).expect("retain database and any WAL sidecars");

    app.remote_reopen_action(InventoryAction::InspectTask(0), &ctx);
    // The action may already have drained a fast failure; settle accepts either timing.
    settle(
        &mut app.remote_environments.reopen,
        &client(&home, &scope),
        &mut app.board,
        &ctx,
    );
    let rendered = text(&app.remote_environments.reopen, &ctx);
    assert!(rendered.contains("The saved environment could not be safely read. Refresh and retry."));
    assert!(rendered.contains("Running at check (PID 42)") && rendered.contains("previous observation may be stale"));
    assert!(!app.remote_environments.reopen.is_pending() && !directory.exists());

    app.remote_reopen_action(InventoryAction::ListReopenPanels, &ctx);
    settle(
        &mut app.remote_environments.reopen,
        &client(&home, &scope),
        &mut app.board,
        &ctx,
    );
    assert!(app.remote_environments.reopen.catalog.is_none());
    assert_eq!(
        app.remote_environments.reopen.notice.as_deref(),
        Some("The saved environment could not be safely read. Refresh and retry.")
    );
    assert!(matches!(
        super::super::super::load_page(&home, None),
        Err(super::super::super::LoadError::OpenStore)
    ));
    assert!(
        !directory.exists(),
        "no replacement directory, database or WAL sidecars"
    );
    assert_eq!(
        std::fs::read(retained.join(store.path().file_name().expect("database name"))).expect("retained database"),
        database
    );
    assert!(app.board.panels.is_empty() && app.board.workspaces.is_empty() && app.runtime_dirty_since.is_none());
    assert_eq!(
        app.session_store
            .resume_session(&scope.owner)
            .expect("session")
            .runtime_state
            .to_yaml()
            .expect("runtime YAML"),
        saved
    );
}

#[test]
fn queued_checks_are_discarded_for_session_selection_config_close_and_stop_without_freeing_slot_early() {
    for fault in 0..5 {
        let (_temp, mut app, store, scope) = app_fixture();
        let ctx = Context::default();
        app.remote_environments.reopen = loaded(&store, &scope, &ctx);
        let tx = queue(&mut app.remote_environments.reopen, &scope, &ctx);
        match fault {
            0 => app.remote_environments.invalidate_session_views(),
            1 => app
                .remote_environments
                .apply(InventoryAction::Refresh, app.session_store.home(), &ctx),
            2 => app.remote_environments.invalidate_provider_state(),
            3 => app
                .remote_environments
                .apply(InventoryAction::Close, app.session_store.home(), &ctx),
            _ => app.remote_environments.stop_action(
                InventoryAction::RequestStop,
                app.session_store.home(),
                &scope.config,
                &ctx,
            ),
        }
        let home = app.session_store.home().clone();
        app.remote_environments
            .reopen
            .inspect_task(&client(&home, &scope), 0, &ctx);
        assert!(
            app.remote_environments
                .reopen
                .pending
                .as_ref()
                .expect("retained slot")
                .discard
        );
        assert!(
            tx.send(Ok(observation(RemotePanelStatus::Running { pid: 42 }))).is_ok(),
            "late result"
        );
        app.remote_reopen_action(InventoryAction::None, &ctx);
        assert!(!app.remote_environments.reopen.is_pending() && app.remote_environments.reopen.catalog.is_none());
        assert!(app.board.panels.is_empty());
    }
}

#[test]
fn rendered_check_uses_only_selected_panel_and_completed_or_pending_display_does_not_poll() {
    let (_temp, mut app, store, scope) = app_fixture();
    let ctx = Context::default();
    app.remote_environments.reopen = loaded(&store, &scope, &ctx);
    let _ = text(&app.remote_environments.reopen, &ctx);
    let position = ctx
        .data(|data| data.get_temp::<egui::Rect>(egui::Id::new(("inspect-task-test", 0_usize))))
        .expect("check button")
        .center();
    let mut action = InventoryAction::None;
    for pressed in [true, false] {
        let mut input = raw_input([1000.0, 900.0], None);
        input.events.extend([
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        let _ = ctx
            .run_ui(input, |ui| {
                super::super::paint::show(ui, &app.remote_environments.reopen, true, &mut action);
            })
            .discard_textures();
    }
    assert!(matches!(action, InventoryAction::InspectTask(0)));
    app.remote_reopen_action(action, &ctx);
    let home = app.session_store.home().clone();
    settle(
        &mut app.remote_environments.reopen,
        &client(&home, &scope),
        &mut app.board,
        &ctx,
    );
    let expected = if cfg!(target_os = "linux") {
        "no local provider profile exactly matches"
    } else {
        "protected remote panel inspection is not yet supported on this platform"
    };
    assert!(text(&app.remote_environments.reopen, &ctx).contains(expected));
    let tx = queue(&mut app.remote_environments.reopen, &scope, &ctx);
    for _ in 0..30 {
        let _ = text(&app.remote_environments.reopen, &ctx);
    }
    assert!(
        !ctx.has_requested_repaint(),
        "pending task check must not animate or poll"
    );
    drop(tx);
    app.remote_reopen_action(InventoryAction::None, &ctx);
    for _ in 0..30 {
        let _ = text(&app.remote_environments.reopen, &ctx);
    }
    assert!(!ctx.has_requested_repaint(), "completed task check must not poll");
    assert!(app.board.panels.is_empty());
}

#[test]
fn runpod_task_check_uses_existing_action_without_opening_a_view_or_polling() {
    let (_temp, mut app, store, mut scope) = app_fixture();
    let saved = store
        .load_remote_workspace(&scope.owner, "environment")
        .expect("read")
        .expect("record");
    let mut state = saved.state().clone();
    assert!(state.runtime.is_none(), "seed provider before allocation or trust");
    state.spec.target.provider = horizon_core::cloud_run::CloudProvider::RunPod;
    scope.expected = store
        .replace_remote_workspace(&saved, &state)
        .expect("dormant RunPod fixture")
        .environment_summary();
    app.remote_environments.page.as_mut().expect("page").rows[0] = InventoryRow::new(scope.expected.clone());
    let ctx = Context::default();
    app.remote_environments.reopen = loaded(&store, &scope, &ctx);
    app.runtime_dirty_since = None;
    let before = std::fs::read(store.path()).expect("database bytes");
    assert!(text(&app.remote_environments.reopen, &ctx).contains("Check retained task"));
    app.remote_reopen_action(InventoryAction::InspectTask(0), &ctx);
    let home = app.session_store.home().clone();
    settle(
        &mut app.remote_environments.reopen,
        &client(&home, &scope),
        &mut app.board,
        &ctx,
    );
    let expected = if cfg!(target_os = "linux") {
        "no RunPod profile exactly matches"
    } else {
        "protected remote panel inspection is not yet supported on this platform"
    };
    assert!(text(&app.remote_environments.reopen, &ctx).contains(expected));
    for _ in 0..30 {
        let _ = text(&app.remote_environments.reopen, &ctx);
    }
    assert!(!ctx.has_requested_repaint() && !app.remote_environments.reopen.is_pending());
    assert!(app.board.panels.is_empty() && app.board.workspaces.is_empty() && app.runtime_dirty_since.is_none());
    assert_eq!(std::fs::read(store.path()).expect("database bytes"), before);
}

#[test]
fn runpod_profile_changes_discard_queued_task_observations() {
    let (_temp, app, store, mut scope) = app_fixture();
    let saved = store
        .load_remote_workspace(&scope.owner, "environment")
        .expect("read")
        .expect("record");
    let mut state = saved.state().clone();
    state.spec.target.provider = horizon_core::cloud_run::CloudProvider::RunPod;
    scope.expected = store
        .replace_remote_workspace(&saved, &state)
        .expect("dormant RunPod")
        .environment_summary();
    scope.config.runpod.push(
        serde_json::from_value(serde_json::json!({
            "name":"development", "gpu_type_ids":["synthetic-gpu"], "gpu_count":1,
            "ports":["22/tcp"], "volume_gib":0
        }))
        .expect("profile"),
    );
    let ctx = Context::default();
    let mut reopen = loaded(&store, &scope, &ctx);
    let tx = queue(&mut reopen, &scope, &ctx);
    scope.config.runpod[0].data_center_id = Some("EU-RO-1".into());
    tx.send(Ok(observation(RemotePanelStatus::Running { pid: 42 })))
        .expect("late result");
    let mut board = Board::new();
    reopen.drain(&client(app.session_store.home(), &scope), &mut board, &ctx);
    assert!(!reopen.is_pending() && reopen.notice.is_none());
    assert!(!text(&reopen, &ctx).contains("Running at check"));
    reopen.inspect_task(&client(app.session_store.home(), &scope), 0, &ctx);
    assert!(reopen.catalog.is_none() && !reopen.is_pending());
    assert!(text(&reopen, &ctx).contains("The session or saved selection changed"));
    assert!(board.panels.is_empty() && board.workspaces.is_empty());
}
