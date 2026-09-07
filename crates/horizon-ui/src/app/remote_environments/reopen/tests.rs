use super::super::{InventoryPage, RemoteEnvironments, paint::InventoryRow};
use super::*;
use crate::app::test_support::{raw_input, test_app};
use crate::test_egui::DiscardTextures;
use horizon_core::{CanvasViewState, PanelKind, PanelOptions, RuntimeState, WindowConfig};
use std::time::{Duration, Instant};

const OWNER: &str = "00000000-0000-4000-8000-000000000001";
const FOREIGN: &str = "00000000-0000-4000-8000-000000000002";

fn seed(home: &HorizonHome, owner: &str, count: usize) -> (CloudWorkflowStore, RemoteEnvironmentSummary) {
    let store = CloudWorkflowStore::open(home).expect("store");
    let panels: Vec<_> = (0..count)
        .map(|index| {
            serde_json::json!({
                "panel_local_id":format!("task-{index}"), "kind":"command",
                "command":{"program":"private-must-not-execute", "args":["private-argument"]}
            })
        })
        .collect();
    let state = serde_json::from_value(serde_json::json!({"version":1,"spec":{
        "workspace_local_id":"environment","working_directory":".","generation":0,
        "target":{"provider":"local_docker","profile":"development","disk_gib":20,
            "lifetime":"persistent","image":format!("example/worker@sha256:{}", "a".repeat(64))},
        "repository":{"repository":"example/project","commit":"b".repeat(40)},"panels":panels
    }}))
    .expect("state");
    let record = store.create_remote_workspace(owner, &state).expect("record");
    (store, record.environment_summary())
}

fn scope(expected: &RemoteEnvironmentSummary) -> RequestScope {
    RequestScope {
        expected: expected.clone(),
        owner: expected.owning_session_id.clone(),
        config: RemoteProviderConfig::default(),
    }
}

fn client<'a>(home: &'a HorizonHome, scope: &'a RequestScope) -> ClientContext<'a> {
    ClientContext {
        home,
        config: &scope.config,
        selected: Some(&scope.expected),
        owner: Some(&scope.owner),
    }
}

fn pending(
    state: &mut ReopenState,
    scope: &RequestScope,
    ctx: &Context,
) -> mpsc::SyncSender<Result<Completion, String>> {
    let (tx, rx) = mpsc::sync_channel(1);
    state.pending = Some(PendingReopen {
        rx,
        scope: scope.clone(),
        discard: false,
    });
    state.repaint_context = Some(ctx.clone());
    tx
}

fn prepared(store: &CloudWorkflowStore, scope: &RequestScope, board: &Board) -> Box<PreparedRemoteViewReopen> {
    let catalog = RemoteViewCatalog::load(store, &scope.owner, &scope.expected).expect("catalog");
    let request = board
        .request_remote_view_reopen(&scope.owner, &catalog, "task-0")
        .expect("request");
    let store = store.clone();
    std::thread::spawn(move || request.prepare(&store).map(Box::new))
        .join()
        .expect("worker")
        .expect("prepared")
}

fn settle(state: &mut ReopenState, client: &ClientContext<'_>, board: &mut Board, ctx: &Context) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.is_pending() {
        state.drain(client, board, ctx);
        assert!(Instant::now() < deadline, "bounded local worker");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn foreign_and_ephemeral_clients_are_rejected_before_opening_storage() {
    let temp = tempfile::tempdir().expect("fixture");
    let (_, expected) = seed(&HorizonHome::from_root(temp.path().join("seed")), OWNER, 1);
    let home = HorizonHome::from_root(temp.path().join("unused"));
    let scope = scope(&expected);
    let ctx = Context::default();
    let mut state = ReopenState::default();
    for owner in [None, Some(FOREIGN)] {
        let mut client = client(&home, &scope);
        client.owner = owner;
        state.load(&client, &ctx);
        assert!(!state.is_pending() && state.catalog.is_none() && state.notice.is_some());
    }
    assert!(!home.root().exists());
}

#[test]
fn explicit_load_and_reopen_are_inert_persistable_and_revalidate_cached_presence() {
    let temp = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(temp.path().join("state"));
    let (store, expected) = seed(&home, OWNER, 3);
    let original = store.load_remote_workspace(OWNER, "environment").expect("record");
    let scope = scope(&expected);
    let client = client(&home, &scope);
    let ctx = Context::default();
    let mut board = Board::new();
    let mut state = ReopenState::default();
    let unrelated = board.create_workspace("Local");
    board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Editor,
                local_id: Some("task-1".into()),
                ..Default::default()
            },
            unrelated,
        )
        .expect("editor");
    state.load(&client, &ctx);
    settle(&mut state, &client, &mut board, &ctx);
    assert_eq!(state.catalog.as_ref().expect("catalog").rows.len(), 3);
    assert!(!state.catalog.as_ref().expect("catalog").rows[1].present);
    state.reopen(&client, &board, 1, &ctx);
    assert!(!state.is_pending() && board.panels.len() == 1);
    assert_eq!(
        state.notice.as_deref(),
        Some(
            horizon_core::RemoteViewReopenError::ViewAlreadyPresent
                .to_string()
                .as_str()
        )
    );
    state.reopen(&client, &board, 0, &ctx);
    settle(&mut state, &client, &mut board, &ctx);
    assert_eq!(board.panels.len(), 2);
    assert!(board.panels[1].terminal().expect("inert terminal").child_exited());
    assert!(state.catalog.as_ref().expect("catalog").rows[0].present);
    assert!(!state.catalog.as_ref().expect("catalog").rows[1].present);
    state.reopen(&client, &board, 0, &ctx);
    assert!(!state.is_pending() && board.panels.len() == 2);
    let saved = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
    saved.validate_remote_references().expect("persistable");
    assert!(!saved.to_yaml().expect("yaml").contains("private-"));
    board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Editor,
                local_id: Some("task-2".into()),
                ..Default::default()
            },
            unrelated,
        )
        .expect("late conflicting editor");
    state.reopen(&client, &board, 2, &ctx);
    assert!(!state.is_pending() && board.panels.len() == 3);
    state.reopen(&client, &board, 1, &ctx);
    assert!(!state.is_pending() && board.panels.len() == 3);
    assert_eq!(
        store.load_remote_workspace(OWNER, "environment").expect("record"),
        original
    );
}

#[test]
fn invalidation_retains_one_worker_until_discard_and_explicit_retry() {
    let temp = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(temp.path().join("state"));
    let (store, expected) = seed(&home, OWNER, 0);
    let scope = scope(&expected);
    let client = client(&home, &scope);
    let ctx = Context::default();
    let mut board = Board::new();
    let mut state = ReopenState::default();
    let tx = pending(&mut state, &scope, &ctx);
    for _ in 0..100 {
        state.invalidate();
        state.load(&client, &ctx);
        state.reopen(&client, &board, 0, &ctx);
        assert!(state.pending.as_ref().expect("slot").discard);
    }
    let catalog = RemoteViewCatalog::load(&store, OWNER, &expected).expect("catalog");
    assert!(tx.send(Ok(Completion::Catalog(Box::new(catalog)))).is_ok());
    assert_eq!(state.drain(&client, &mut board, &ctx), None);
    assert!(!state.is_pending() && state.catalog.is_none() && state.notice.is_none());
    state.load(&client, &ctx);
    settle(&mut state, &client, &mut board, &ctx);
    assert!(state.catalog.as_ref().expect("empty catalog").rows.is_empty());
    drop(pending(&mut state, &scope, &ctx));
    state.drain(&client, &mut board, &ctx);
    assert_eq!(state.notice.as_deref(), Some(worker_failure()));
}

#[test]
fn missing_and_invalid_storage_errors_leave_an_explicit_retry_without_board_changes() {
    let temp = tempfile::tempdir().expect("fixture");
    let (_, expected) = seed(&HorizonHome::from_root(temp.path().join("seed")), OWNER, 1);
    let scope = scope(&expected);
    let ctx = Context::default();
    for malformed in [false, true] {
        let home = HorizonHome::from_root(temp.path().join(if malformed { "invalid" } else { "missing" }));
        if malformed {
            std::fs::write(home.root(), b"not a directory").expect("invalid storage fixture");
        }
        let mut board = Board::new();
        let mut state = ReopenState::default();
        for _ in 0..2 {
            state.load(&client(&home, &scope), &ctx);
            settle(&mut state, &client(&home, &scope), &mut board, &ctx);
            assert!(state.catalog.is_none() && state.notice.is_some());
            assert!(board.panels.is_empty() && board.workspaces.is_empty());
        }
    }
}

#[test]
fn queued_preparation_rechecks_owner_selection_config_and_explicit_discard() {
    let temp = tempfile::tempdir().expect("fixture");
    let home = HorizonHome::from_root(temp.path().join("state"));
    let (store, expected) = seed(&home, OWNER, 1);
    let original = scope(&expected);
    let ctx = Context::default();
    for fault in 0..4 {
        let mut board = Board::new();
        let mut state = ReopenState::default();
        let tx = pending(&mut state, &original, &ctx);
        assert!(
            tx.send(Ok(Completion::View(prepared(&store, &original, &board))))
                .is_ok()
        );
        let mut changed = original.clone();
        match fault {
            0 => changed.owner = FOREIGN.into(),
            1 => changed.expected.revision += 1,
            2 => changed
                .config
                .local_docker
                .push(horizon_core::cloud_run::local_docker::LocalDockerProfile {
                    name: "changed".into(),
                    docker_host: "unix:///unused/socket".into(),
                }),
            _ => state.invalidate(),
        }
        assert_eq!(state.drain(&client(&home, &changed), &mut board, &ctx), None);
        assert!(board.panels.is_empty() && board.workspaces.is_empty() && state.notice.is_none());
    }
}

fn app_fixture() -> (
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
    let (store, expected) = seed(app.session_store.home(), &session.session_id, 1);
    app.remote_environments = RemoteEnvironments {
        open: true,
        selected: Some(0),
        page: Some(InventoryPage {
            rows: vec![InventoryRow::new(expected.clone())],
            next_cursor: None,
        }),
        ..Default::default()
    };
    (temp, app, store, scope(&expected))
}

#[test]
fn application_adoption_marks_dirty_and_retries_blocked_persistence() {
    let (_temp, mut app, store, scope) = app_fixture();
    let ctx = Context::default();
    let tx = pending(&mut app.remote_environments.reopen, &scope, &ctx);
    assert!(
        tx.send(Ok(Completion::View(prepared(&store, &scope, &app.board))))
            .is_ok()
    );
    app.remote_reopen_action(InventoryAction::None, &ctx);
    assert!(app.runtime_dirty_since.is_some() && app.board.panels.len() == 1);
    app.pending_startup_runtime_state = Some(RuntimeState::default());
    app.runtime_dirty_since = Some(Instant::now().checked_sub(Duration::from_secs(1)).expect("test clock"));
    app.flush_runtime_if_dirty();
    assert!(app.runtime_dirty_since.is_some());
    assert!(
        app.session_store
            .resume_session(&scope.owner)
            .expect("saved")
            .runtime_state
            .workspaces
            .is_empty()
    );
    app.pending_startup_runtime_state = None;
    app.root_viewport_stabilizer = None;
    app.runtime_dirty_since = Some(Instant::now().checked_sub(Duration::from_secs(1)).expect("test clock"));
    app.flush_runtime_if_dirty();
    assert!(app.runtime_dirty_since.is_none());
    let saved = app
        .session_store
        .resume_session(&scope.owner)
        .expect("saved")
        .runtime_state;
    assert_eq!(saved.workspaces[0].panels[0].local_id, "task-0");
    assert!(!saved.to_yaml().expect("yaml").contains("private-"));
}

#[test]
fn escape_discards_a_success_already_queued_before_the_modal_frame() {
    let (_temp, mut app, store, scope) = app_fixture();
    let ctx = Context::default();
    let _ = ctx
        .run_ui(raw_input([900.0, 700.0], None), |ui| {
            let _ = app.render_remote_environments(ui);
        })
        .discard_textures();
    let tx = pending(&mut app.remote_environments.reopen, &scope, &ctx);
    assert!(
        tx.send(Ok(Completion::View(prepared(&store, &scope, &app.board))))
            .is_ok()
    );
    let mut input = raw_input([900.0, 700.0], None);
    input.events.push(egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    let _ = ctx
        .run_ui(input, |ui| {
            let _ = app.render_remote_environments(ui);
        })
        .discard_textures();
    assert!(!app.remote_environments.open && app.board.panels.is_empty());
    assert!(!app.remote_environments.reopen.is_pending() && app.remote_environments.reopen.notice.is_none());
}

#[test]
fn same_owner_board_replacement_and_competing_reconnect_discard_prepared_views() {
    for replace_board in [false, true] {
        let (_temp, mut app, store, scope) = app_fixture();
        let ctx = Context::default();
        let tx = pending(&mut app.remote_environments.reopen, &scope, &ctx);
        assert!(
            tx.send(Ok(Completion::View(prepared(&store, &scope, &app.board))))
                .is_ok()
        );
        if replace_board {
            app.apply_runtime_state(&RuntimeState::default());
        } else {
            app.remote_environments
                .apply(InventoryAction::ListReconnectViews, app.session_store.home(), &ctx);
            app.remote_reconnect_action(InventoryAction::ListReconnectViews, &ctx);
        }
        assert!(
            app.remote_environments
                .reopen
                .pending
                .as_ref()
                .expect("retained slot")
                .discard
        );
        app.remote_reopen_action(InventoryAction::None, &ctx);
        assert!(app.board.panels.is_empty() && app.remote_environments.reopen.notice.is_none());
    }
}

#[test]
fn post_paint_config_invalidation_wakes_a_completed_catalog_once_without_polling() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let (_temp, mut app, store, scope) = app_fixture();
    let ctx = Context::default();
    let tx = pending(&mut app.remote_environments.reopen, &scope, &ctx);
    let catalog = RemoteViewCatalog::load(&store, &scope.owner, &scope.expected).expect("catalog");
    assert!(tx.send(Ok(Completion::Catalog(Box::new(catalog)))).is_ok());
    for frame in 0..30 {
        let mut input = raw_input([900.0, 700.0], None);
        input.time = Some(f64::from(frame) * 0.1);
        let _ = ctx
            .run_ui(input, |ui| {
                let _ = app.render_remote_environments(ui);
            })
            .discard_textures();
        if !ctx.has_requested_repaint() {
            break;
        }
    }
    assert!(app.board.panels.is_empty() && app.remote_environments.reopen.catalog.is_some());
    assert!(!ctx.has_requested_repaint(), "completed catalog must not poll");
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&requests);
    ctx.set_request_repaint_callback(move |_| {
        observed.fetch_add(1, Ordering::Relaxed);
    });
    let mut config = app.template_config.clone();
    app.apply_runtime_config(&config);
    assert_eq!(requests.load(Ordering::Relaxed), 0);
    config
        .remote
        .local_docker
        .push(horizon_core::cloud_run::local_docker::LocalDockerProfile {
            name: "changed".into(),
            docker_host: "unix:///unused/socket".into(),
        });
    app.apply_runtime_config(&config);
    assert_eq!(requests.load(Ordering::Relaxed), 1);
    assert!(app.remote_environments.reopen.catalog.is_none());
    app.remote_environments.invalidate_provider_state();
    assert_eq!(
        requests.load(Ordering::Relaxed),
        1,
        "repeated invalidation must not loop"
    );
}
