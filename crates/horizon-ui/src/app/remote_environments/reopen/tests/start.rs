use super::super::start::PendingStart;
use super::*;
use horizon_core::remote_worker_status::{
    ConfiguredRemotePanelStatusRequest, RemotePanelStatus, prepare_configured_remote_git_start,
};

fn ready(store: &CloudWorkflowStore, scope: &mut RequestScope) {
    let record = store
        .load_remote_workspace(OWNER, "environment")
        .expect("read")
        .expect("record");
    let mut state = record.state().clone();
    state.spec.repository.branch = Some("work/synthetic".into());
    state.spec.panels[0].kind = PanelKind::Shell;
    let record = store.replace_remote_workspace(&record, &state).expect("intent");
    let allocation = store.allocate_remote_runtime(&record, i64::MAX).expect("allocation");
    let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcH";
    let allocation = store.reserve_remote_worker_request(&allocation, key).expect("request");
    let request = allocation.worker_request().expect("worker request");
    let status: horizon_core::cloud_run::interactive_worker::InteractiveWorkerStatus =
        serde_json::from_value(serde_json::json!({
            "worker":{"identity":{"provider":"local_docker","workflow_id":request.workflow_id,
                "job_id":request.job_id,"resource_id":"synthetic-worker"},
                "target":request.target,"ssh_public_key":key,"lease":{"lifetime":"persistent"}},
            "lifecycle":"provisioning",
            "ssh":{"host":"127.0.0.1","port":1,"username":"root","host_key":key}
        }))
        .expect("status");
    let mut saved = allocation.workspace().state().clone();
    let runtime = saved.runtime.as_mut().expect("reserved runtime");
    runtime.worker = Some(status.worker);
    runtime.ssh = status.ssh;
    scope.expected = store
        .replace_remote_workspace(allocation.workspace(), &saved)
        .expect("synthetic observation")
        .environment_summary();
    scope.config = serde_json::from_value(serde_json::json!({"local_docker":[{
        "name":"development","docker_host":"unix:///tmp/synthetic-must-not-connect.sock"
    }]}))
    .expect("config");
}

fn loaded(store: &CloudWorkflowStore, scope: &RequestScope, ctx: &Context) -> ReopenState {
    let mut state = ReopenState::default();
    let catalog = RemoteViewCatalog::load(store, OWNER, &scope.expected).expect("catalog");
    let tx = pending(&mut state, scope, ctx);
    assert!(tx.send(Ok(Completion::Catalog(Box::new(catalog)))).is_ok());
    state.drain(
        &client(&HorizonHome::from_root("/unused-start-test".into()), scope),
        &mut Board::new(),
        ctx,
    );
    state
}

fn text(state: &ReopenState, ctx: &Context) -> String {
    let output = ctx
        .run_ui(raw_input([1100.0, 1200.0], None), |ui| {
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

#[test]
fn preview_is_explicit_cancelable_and_confirm_requires_the_saved_snapshot() {
    let temp = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(temp.path().join("state"));
    let (store, expected) = seed(&home, OWNER, 1);
    let mut scope = scope(&expected);
    ready(&store, &mut scope);
    let before = store.load_remote_allocation(OWNER, "environment").expect("allocation");
    let ctx = Context::default();
    let mut state = loaded(&store, &scope, &ctx);
    let mut board = Board::new();
    assert!(!text(&state, &ctx).contains("private-argument"));
    state.confirm_start(&client(&home, &scope), &ctx);
    assert!(!state.is_pending());
    for cancel in [true, false] {
        state.prepare_start(&client(&home, &scope), 0, &ctx);
        settle(&mut state, &client(&home, &scope), &mut board, &ctx);
        let rendered = text(&state, &ctx);
        assert!(rendered.contains("private-argument") && rendered.contains("work/synthetic"));
        assert!(rendered.contains("Closing this view does not stop it"));
        if cancel {
            state.start.cancel();
        }
        state.confirm_start(&client(&home, &scope), &ctx);
        settle(&mut state, &client(&home, &scope), &mut board, &ctx);
        assert!(!text(&state, &ctx).contains("Start this saved Shell task?"));
    }
    // No private identity exists: confirmation fails closed before any provider connection.
    assert!(text(&state, &ctx).contains("Inspect the retained task"));
    assert!(board.panels.is_empty());
    assert_eq!(
        store.load_remote_allocation(OWNER, "environment").expect("allocation"),
        before
    );
}

#[test]
fn pending_start_is_single_flight_and_stale_preview_is_discarded() {
    let temp = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(temp.path().join("state"));
    let (store, expected) = seed(&home, OWNER, 1);
    let mut scope = scope(&expected);
    ready(&store, &mut scope);
    let ctx = Context::default();
    for invalidate in [false, true] {
        let mut state = loaded(&store, &scope, &ctx);
        let tx = pending(&mut state, &scope, &ctx);
        state.pending.as_mut().expect("pending").start = Some(PendingStart::Prepare("task-0".into()));
        state.prepare_start(&client(&home, &scope), 0, &ctx);
        state.confirm_start(&client(&home, &scope), &ctx);
        let prepared = prepare_configured_remote_git_start(
            &store,
            &scope.config,
            ConfiguredRemotePanelStatusRequest {
                expected: &scope.expected,
                client_session_id: OWNER,
                panel_id: "task-0",
            },
        )
        .expect("preview");
        if invalidate {
            state.invalidate();
        }
        assert!(tx.send(Ok(Completion::StartPreview(Box::new(prepared)))).is_ok());
        let mut changed = scope.clone();
        changed.owner = FOREIGN.into();
        state.drain(&client(&home, &changed), &mut Board::new(), &ctx);
        assert!(!state.is_pending() && !text(&state, &ctx).contains("Start this saved Shell task?"));
    }
}

#[test]
fn lost_start_response_never_offers_automatic_retry_or_fabricates_status() {
    let temp = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(temp.path().join("state"));
    let (store, expected) = seed(&home, OWNER, 1);
    let scope = scope(&expected);
    let ctx = Context::default();
    let mut state = loaded(&store, &scope, &ctx);
    let tx = pending(&mut state, &scope, &ctx);
    state.pending.as_mut().expect("pending").start = Some(PendingStart::Execute("task-0".into()));
    drop(tx);
    state.drain(&client(&home, &scope), &mut Board::new(), &ctx);
    let rendered = text(&state, &ctx);
    assert!(rendered.contains("outcome is unknown") && !rendered.contains("retry now"));
    for _ in 0..3 {
        state.drain(&client(&home, &scope), &mut Board::new(), &ctx);
    }
    assert!(!state.is_pending());
    state.accept_start(
        PendingStart::Execute("task-0".into()),
        scope,
        Ok(Completion::Started(RemotePanelStatus::Exited {
            pid: 42,
            exit_status: None,
        })),
    );
    assert!(text(&state, &ctx).contains("exit status unknown"));
}

#[test]
fn unavailable_start_response_is_definite_and_never_restarts_or_retries() {
    let temp = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(temp.path().join("state"));
    let (store, expected) = seed(&home, OWNER, 1);
    let scope = scope(&expected);
    let ctx = Context::default();
    let mut state = loaded(&store, &scope, &ctx);
    let mut board = Board::new();
    state.accept_start(
        PendingStart::Execute("task-0".into()),
        scope.clone(),
        Ok(Completion::Started(RemotePanelStatus::Unavailable)),
    );
    let rendered = text(&state, &ctx);
    assert!(rendered.contains("Unavailable at start response. Not restarted."));
    assert!(!rendered.contains("outcome is unknown"));
    for _ in 0..3 {
        assert!(state.drain(&client(&home, &scope), &mut board, &ctx).is_none());
        assert!(!state.is_pending());
        assert_eq!(text(&state, &ctx), rendered);
    }
    assert!(board.panels.is_empty());
}

#[test]
fn rendered_start_and_direct_prepare_refuse_unsupported_saved_shapes_and_provider() {
    use horizon_core::cloud_run::CloudProvider;
    for (provider, kind, command, handoff, supported) in [
        (CloudProvider::LocalDocker, PanelKind::Shell, true, false, true),
        (CloudProvider::RunPod, PanelKind::Shell, true, false, true),
        (CloudProvider::LocalDocker, PanelKind::Command, true, false, false),
        (CloudProvider::LocalDocker, PanelKind::Pi, true, false, false),
        (CloudProvider::LocalDocker, PanelKind::Shell, false, false, false),
        (CloudProvider::LocalDocker, PanelKind::Shell, true, true, false),
        (CloudProvider::Azure, PanelKind::Shell, true, false, false),
    ] {
        let temp = tempfile::tempdir().expect("fixture");
        let home = HorizonHome::from_root(temp.path().join("state"));
        let (store, _) = seed(&home, OWNER, 1);
        let record = store
            .load_remote_workspace(OWNER, "environment")
            .expect("load")
            .expect("record");
        let mut saved = record.state().clone();
        saved.spec.target.provider = provider;
        let panel = &mut saved.spec.panels[0];
        panel.kind = kind;
        if !command {
            panel.command = None;
        }
        panel.task_handoff = handoff.then(|| "private-handoff".into());
        let record = store.replace_remote_workspace(&record, &saved).expect("fixture intent");
        let scope = scope(&record.environment_summary());
        let ctx = Context::default();
        let mut state = loaded(&store, &scope, &ctx);
        let rendered = text(&state, &ctx);
        assert!(rendered.contains("Start saved Shell task"));
        assert!(!rendered.contains("private-"));
        let position = ctx
            .data(|data| data.get_temp::<egui::Rect>(egui::Id::new(("start-task-request", 0_usize))))
            .expect("start button")
            .center();
        let mut action = InventoryAction::None;
        for pressed in [true, false] {
            let mut input = raw_input([1100.0, 1200.0], None);
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
                .run_ui(input, |ui| super::super::paint::show(ui, &state, true, &mut action))
                .discard_textures();
        }
        assert_eq!(
            matches!(action, InventoryAction::PrepareTaskStart(0)),
            supported,
            "{provider:?}/{kind:?}"
        );
        if !supported {
            assert!(matches!(action, InventoryAction::None));
            let unopened = HorizonHome::from_root(temp.path().join("must-not-open"));
            state.prepare_start(&client(&unopened, &scope), 0, &ctx);
            assert!(!state.is_pending() && state.notice.is_none());
            assert!(!unopened.root().exists());
        }
        assert_eq!(
            store.load_remote_workspace(OWNER, "environment").expect("unchanged"),
            Some(record)
        );
    }
}

#[test]
fn stale_execution_keeps_an_unattributed_unknown_warning_after_reopening() {
    let temp = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(temp.path().join("state"));
    let (store, expected) = seed(&home, OWNER, 1);
    let scope = scope(&expected);
    let ctx = Context::default();
    let mut state = loaded(&store, &scope, &ctx);
    let tx = pending(&mut state, &scope, &ctx);
    state.pending.as_mut().expect("pending").start = Some(PendingStart::Execute("task-0".into()));
    state.invalidate();
    assert!(
        tx.send(Ok(Completion::Started(RemotePanelStatus::Running { pid: 4321 })))
            .is_ok()
    );
    state.drain(&client(&home, &scope), &mut Board::new(), &ctx);
    state.invalidate();
    let rendered = text(&state, &ctx);
    assert!(rendered.contains("Its outcome is unknown") && !rendered.contains("4321"));
    assert!(!state.is_pending());
}
