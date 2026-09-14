use super::super::{Board, InventoryAction};
use super::*;
use crate::app::remote_environments::{InventoryPage, RemoteEnvironments, paint::InventoryRow};
use crate::app::test_support::{raw_input, test_app};
use crate::test_egui::DiscardTextures;
use horizon_core::{RuntimeState, cloud_run::CloudWorkflowStore};
use std::time::{Duration, Instant};

fn fixture() -> (
    tempfile::TempDir,
    crate::app::HorizonApp,
    CloudWorkflowStore,
    RequestScope,
) {
    let (temp, mut app) = test_app();
    let session = app
        .session_store
        .create_session_from_runtime(RuntimeState::default())
        .expect("session");
    app.activate_persistent_session(&session);
    let store = CloudWorkflowStore::open(app.session_store.home()).expect("store");
    let state = serde_json::from_value(serde_json::json!({"version":1,"spec":{
        "workspace_local_id":"environment", "working_directory":"nested", "generation":0,
        "target":{"provider":"local_docker", "profile":"development", "disk_gib":20,
            "lifetime":"persistent", "image":format!("example/worker@sha256:{}", "a".repeat(64))},
        "repository":{"repository":"example/project", "commit":"b".repeat(40)},
        "panels":[{"panel_local_id":"original", "kind":"shell", "command":{"program":"must-not-execute", "args":[]}}]
    }}))
    .expect("state");
    let saved = store
        .create_remote_workspace(&session.session_id, &state)
        .expect("workspace");
    let allocation = store.allocate_remote_runtime(&saved, i64::MAX).expect("allocation");
    let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcH";
    let allocation = store.reserve_remote_worker_request(&allocation, key).expect("request");
    let request = allocation.worker_request().expect("worker request");
    let status: horizon_core::cloud_run::interactive_worker::InteractiveWorkerStatus =
        serde_json::from_value(serde_json::json!({
            "worker":{"identity":{"provider":"local_docker","workflow_id":request.workflow_id,
                "job_id":request.job_id,"resource_id":"synthetic-worker"},
                "target":request.target,"ssh_public_key":key,"lease":{"lifetime":"persistent"}},
            "lifecycle":"provisioning",
            "ssh":{"host":"127.0.0.1","port":1,"username":"horizon","host_key":key}
        }))
        .expect("status");
    let mut state = allocation.workspace().state().clone();
    let runtime = state.runtime.as_mut().expect("runtime");
    runtime.phase = horizon_core::remote_workspace::RemoteRuntimePhase::Reconciling;
    runtime.worker = Some(status.worker);
    runtime.ssh = status.ssh;
    let saved = store
        .replace_remote_workspace(allocation.workspace(), &state)
        .expect("observation");
    let expected = saved.environment_summary();
    let scope = RequestScope {
        expected: expected.clone(),
        owner: session.session_id,
        config: app.template_config.remote.clone(),
    };
    app.remote_environments = RemoteEnvironments {
        open: true,
        selected: Some(0),
        page: Some(InventoryPage {
            rows: vec![InventoryRow::new(expected)],
            next_cursor: None,
        }),
        ..Default::default()
    };
    (temp, app, store, scope)
}

fn client<'a>(home: &'a HorizonHome, scope: &'a RequestScope) -> ClientContext<'a> {
    ClientContext {
        home,
        config: &scope.config,
        selected: Some(&scope.expected),
        owner: Some(&scope.owner),
    }
}

fn settle(state: &mut ReopenState, client: &ClientContext<'_>, ctx: &Context) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut board = Board::new();
    while state.is_pending() {
        state.drain(client, &mut board, ctx);
        assert!(Instant::now() < deadline, "bounded worker");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(board.panels.is_empty() && board.workspaces.is_empty());
}

fn preview(state: &mut ReopenState, client: &ClientContext<'_>, ctx: &Context) {
    state.add_action(Action::Open, client, ctx);
    let form = state.add.form.as_mut().expect("form");
    form.program = "/bin/sh".into();
    form.arguments = "-c\nprintf 'literal $HOME; test'".into();
    form.directory = "nested/second".into();
    state.add_action(Action::Prepare, client, ctx);
    settle(state, client, ctx);
    assert!(state.add.confirmation.is_some(), "{:?}", state.notice);
}

#[test]
fn save_consumes_exact_preview_once_preserves_existing_state_and_creates_no_view() {
    let (_temp, app, store, scope) = fixture();
    let client = client(app.session_store.home(), &scope);
    let before = store
        .load_remote_allocation(&scope.owner, "environment")
        .expect("read")
        .expect("allocation");
    let mut state = ReopenState::default();
    let ctx = Context::default();
    preview(&mut state, &client, &ctx);
    assert_eq!(
        store
            .load_remote_allocation(&scope.owner, "environment")
            .expect("unchanged"),
        Some(before.clone())
    );
    let id = state
        .add
        .confirmation
        .as_ref()
        .expect("preview")
        .panel()
        .panel_local_id
        .clone();
    state.add_action(Action::Confirm, &client, &ctx);
    state.add_action(Action::Confirm, &client, &ctx);
    settle(&mut state, &client, &ctx);
    state.add_action(Action::Confirm, &client, &ctx);
    let saved = store
        .load_remote_allocation(&scope.owner, "environment")
        .expect("read")
        .expect("saved");
    let saved_state = saved.workspace().state();
    assert_eq!(saved_state.spec.panels.len(), 2);
    assert_eq!(saved_state.spec.panels[0], before.workspace().state().spec.panels[0]);
    assert_eq!(saved_state.runtime, before.workspace().state().runtime);
    assert_eq!(saved_state.spec.panels[1].panel_local_id, id);
    assert_eq!(
        saved_state.spec.panels[1].command.as_ref().expect("command").args,
        ["-c", "printf 'literal $HOME; test'"]
    );
    assert_eq!(
        saved_state.spec.panels[1].working_directory.as_deref(),
        Some("nested/second")
    );
    assert!(
        state.refresh_inventory
            && state
                .add_notice
                .as_deref()
                .is_some_and(|text| text.contains("panel saved"))
    );
    assert!(!state.is_pending());
}

#[test]
fn cancel_and_all_client_context_changes_revoke_consent_without_saving() {
    for change in 0..6 {
        let (_temp, app, store, scope) = fixture();
        let home = app.session_store.home();
        let before = store
            .load_remote_allocation(&scope.owner, "environment")
            .expect("before");
        let mut state = ReopenState::default();
        let ctx = Context::default();
        preview(&mut state, &client(home, &scope), &ctx);
        let mut changed = scope.clone();
        let other_home = HorizonHome::from_root(home.root().join("unused"));
        match change {
            0 => state.add_action(Action::Cancel, &client(home, &scope), &ctx),
            1 => changed.owner = "foreign".into(),
            2 => changed.expected.workspace_local_id = "different".into(),
            3 => changed
                .config
                .local_docker
                .push(horizon_core::cloud_run::local_docker::LocalDockerProfile {
                    name: "different".into(),
                    docker_host: "unix:///unused".into(),
                }),
            4 => state.invalidate(),
            _ => {}
        }
        state.add_action(
            Action::Confirm,
            &client(if change == 5 { &other_home } else { home }, &changed),
            &ctx,
        );
        assert!(!state.is_pending() && state.add.confirmation.is_none());
        assert_eq!(
            store
                .load_remote_allocation(&scope.owner, "environment")
                .expect("unchanged"),
            before
        );
        assert!(!other_home.root().exists());
    }
}

#[test]
fn missing_or_foreign_owner_cannot_open_the_form_or_store() {
    let (_temp, app, _store, scope) = fixture();
    let home = HorizonHome::from_root(app.session_store.home().root().join("unused"));
    for owner in [None, Some("foreign")] {
        let mut state = ReopenState::default();
        state.add_action(
            Action::Open,
            &ClientContext {
                owner,
                ..client(&home, &scope)
            },
            &Context::default(),
        );
        assert!(state.add.form.is_none() && !state.is_pending() && !home.root().exists());
    }
}

#[test]
fn invalid_input_and_saved_revision_conflict_require_a_fresh_preview() {
    let (_temp, app, store, scope) = fixture();
    let client = client(app.session_store.home(), &scope);
    let ctx = Context::default();
    let mut state = ReopenState::default();
    state.add_action(Action::Open, &client, &ctx);
    state.add.form.as_mut().expect("form").directory = "../escape".into();
    state.add_action(Action::Prepare, &client, &ctx);
    settle(&mut state, &client, &ctx);
    assert!(state.add.confirmation.is_none() && state.notice.as_deref().is_some_and(|text| text.contains("invalid")));
    preview(&mut state, &client, &ctx);
    let saved = store
        .load_remote_workspace(&scope.owner, "environment")
        .expect("read")
        .expect("saved");
    let mut changed = saved.state().clone();
    changed.spec.panels[0].working_directory = Some("other".into());
    let updated = store
        .replace_remote_workspace(&saved, &changed)
        .expect("concurrent writer");
    state.add_action(Action::Confirm, &client, &ctx);
    settle(&mut state, &client, &ctx);
    assert_eq!(
        store
            .load_remote_workspace(&scope.owner, "environment")
            .expect("unchanged"),
        Some(updated)
    );
    assert!(state.add.confirmation.is_none() && state.refresh_inventory);
    assert!(state.add_notice.as_deref().is_some_and(|text| text.contains("changed")));
}

#[test]
fn queued_preview_is_discarded_and_single_flight_survives_cancel() {
    let (_temp, app, _store, scope) = fixture();
    let client = client(app.session_store.home(), &scope);
    let ctx = Context::default();
    let mut state = ReopenState::default();
    state.add_action(Action::Open, &client, &ctx);
    state.add_action(Action::Prepare, &client, &ctx);
    state.add_action(Action::Prepare, &client, &ctx);
    state.add_action(Action::Cancel, &client, &ctx);
    state.add_action(Action::Open, &client, &ctx);
    assert!(state.is_pending() && state.add.form.is_none());
    settle(&mut state, &client, &ctx);
    assert!(state.add.confirmation.is_none() && !state.refresh_inventory);
}

#[test]
fn lost_and_stale_save_responses_refresh_inventory_without_retry_or_false_success() {
    for stale in [false, true] {
        let (_temp, app, _store, scope) = fixture();
        let client = client(app.session_store.home(), &scope);
        let ctx = Context::default();
        let mut state = ReopenState::default();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        state.pending = Some(super::super::PendingReopen {
            rx,
            scope: scope.clone(),
            discard: stale,
            inspection: None,
            start: None,
            add: Some(PendingAdd {
                scope: Scope {
                    request: scope.clone(),
                    home: client.home.clone(),
                },
                saving: true,
            }),
        });
        drop(tx);
        settle(&mut state, &client, &ctx);
        let notice = state.add_notice.as_deref().expect("outcome warning");
        assert!(notice.contains("unknown") && notice.contains("No retry") || notice.contains("no retry"));
        assert!(state.refresh_inventory && state.add.confirmation.is_none());
        state.add_action(Action::Confirm, &client, &ctx);
        assert!(!state.is_pending());
    }
}

#[test]
fn application_refreshes_saved_inventory_after_save_without_changing_the_board() {
    let (_temp, mut app, store, scope) = fixture();
    let ctx = Context::default();
    preview(
        &mut app.remote_environments.reopen,
        &client(app.session_store.home(), &scope),
        &ctx,
    );
    app.remote_reopen_action(InventoryAction::AddShell(Action::Confirm), &ctx);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.remote_reopen_action(InventoryAction::None, &ctx);
        app.remote_environments.drain_result();
        if !app.remote_environments.reopen.is_pending() && app.remote_environments.pending.is_none() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let saved = store
        .load_remote_workspace(&scope.owner, "environment")
        .expect("read")
        .expect("saved");
    assert_eq!(
        app.remote_environments.page.as_ref().expect("page").rows[0].summary,
        saved.environment_summary()
    );
    assert_eq!(saved.state().spec.panels.len(), 2);
    assert!(app.board.panels.is_empty() && app.board.workspaces.is_empty());
    assert!(
        app.remote_environments
            .reopen
            .add_notice
            .as_deref()
            .is_some_and(|text| text.contains("saved"))
    );
}

#[test]
fn preview_renders_exact_literal_intent_and_enter_does_not_save_or_poll() {
    let (_temp, app, store, scope) = fixture();
    let client = client(app.session_store.home(), &scope);
    let ctx = Context::default();
    let mut state = ReopenState::default();
    preview(&mut state, &client, &ctx);
    let before = store
        .load_remote_allocation(&scope.owner, "environment")
        .expect("before");
    let mut text = String::new();
    for frame in 0..30 {
        let mut input = raw_input([1000.0, 900.0], None);
        input.time = Some(f64::from(frame) * 0.1);
        if frame == 2 {
            input.events.push(egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
        }
        let mut action = InventoryAction::None;
        let output = ctx
            .run_ui(input, |ui| state.show_add(ui, true, &mut action))
            .discard_textures();
        assert!(matches!(action, InventoryAction::None));
        text = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    assert!(
        text.contains("literal $HOME; test")
            && text.contains("nested/second")
            && text.contains("Save independent Shell panel")
    );
    assert!(!ctx.has_requested_repaint(), "settled preview must not poll");
    assert_eq!(
        store
            .load_remote_allocation(&scope.owner, "environment")
            .expect("unchanged"),
        before
    );
}

#[test]
fn concurrent_inventory_load_cannot_consume_the_post_save_refresh() {
    let (_temp, mut app, _store, _scope) = fixture();
    let ctx = Context::default();
    let (_tx, rx) = std::sync::mpsc::sync_channel(1);
    app.remote_environments.pending = Some(crate::app::remote_environments::PendingLoad {
        rx,
        cursor: None,
        discard: false,
    });
    app.remote_environments.reopen.refresh_inventory = true;
    app.remote_reopen_action(InventoryAction::None, &ctx);
    assert!(app.remote_environments.refresh_when_idle);
    assert!(app.remote_environments.pending.is_some());
    app.remote_reopen_action(InventoryAction::None, &ctx);
    assert!(app.remote_environments.refresh_when_idle);
}
