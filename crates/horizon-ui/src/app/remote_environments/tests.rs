use super::*;

fn empty_page(next: Option<&str>) -> InventoryPage {
    InventoryPage {
        rows: Vec::new(),
        next_cursor: next.map(String::from),
    }
}

#[test]
fn failed_next_page_keeps_previous_page_and_exact_retry_cursor() {
    let mut state = RemoteEnvironments {
        open: true,
        ..Default::default()
    };
    state.accept_result(None, Ok(empty_page(Some("page-two"))));
    state.accept_result(Some("page-two".into()), Err(LoadError::ReadPage));
    assert!(state.page_cursor.is_none());
    assert_eq!(
        state.page.as_ref().and_then(|page| page.next_cursor.as_deref()),
        Some("page-two")
    );
    let failure = state.failure.as_ref().expect("failure");
    assert_eq!(failure.cursor.as_deref(), Some("page-two"));
    state.accept_result(Some("page-two".into()), Ok(empty_page(None)));
    assert!(state.failure.is_none());
    assert_eq!(state.page_cursor.as_deref(), Some("page-two"));
    assert!(state.selected.is_none());
}

#[test]
fn repeated_reopen_retains_one_worker_and_discards_its_old_result() {
    let ctx = Context::default();
    let temp = tempfile::tempdir().expect("tempdir");
    let home = HorizonHome::from_root(temp.path().to_path_buf());
    let (tx, rx) = mpsc::sync_channel(1);
    let mut state = RemoteEnvironments {
        open: true,
        pending: Some(PendingLoad {
            rx,
            cursor: Some("old-page".into()),
            discard: false,
        }),
        ..Default::default()
    };
    for _ in 0..100 {
        state.close();
        state.open(&home, &ctx);
        state.start_load(&home, &ctx, None);
        assert!(state.pending.as_ref().is_some_and(|pending| pending.discard));
    }
    tx.send(Ok(empty_page(Some("old-result")))).expect("send");
    state.drain_result();
    assert!(state.page.is_none());
    assert!(state.pending.is_none());
    assert!(state.refresh_when_idle);
    assert!(!home.cloud_workflow_store_path().exists());
}

#[test]
fn closed_view_never_accepts_completion_or_requeues_refresh() {
    let (tx, rx) = mpsc::sync_channel(1);
    let mut state = RemoteEnvironments {
        open: true,
        pending: Some(PendingLoad {
            rx,
            cursor: None,
            discard: false,
        }),
        refresh_when_idle: true,
        ..Default::default()
    };
    state.close();
    tx.send(Ok(empty_page(None))).expect("send");
    state.drain_result();
    assert!(state.page.is_none());
    assert!(state.pending.is_none());
    assert!(!state.refresh_when_idle);
}

#[test]
fn disconnected_worker_surfaces_error_without_erasing_last_page() {
    let (tx, rx) = mpsc::sync_channel(1);
    let mut state = RemoteEnvironments {
        open: true,
        page: Some(empty_page(None)),
        pending: Some(PendingLoad {
            rx,
            cursor: None,
            discard: false,
        }),
        ..Default::default()
    };
    drop(tx);
    state.drain_result();
    assert!(state.page.is_some());
    assert!(state.pending.is_none());
    assert_eq!(
        state.failure.as_ref().map(|failure| failure.error),
        Some(LoadError::WorkerUnavailable)
    );
}

#[test]
fn modal_consumes_terminal_input_and_the_escape_dismissal_frame() {
    use crate::app::test_support::{raw_input, test_app};
    use crate::test_egui::DiscardTextures;
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    app.remote_environments.open = true;
    app.remote_environments.page = Some(empty_page(None));
    let _ = ctx
        .run_ui(raw_input([900.0, 680.0], None), |ui| {
            assert!(app.render_remote_environments(ui).is_some());
        })
        .discard_textures();
    let mut input = raw_input([900.0, 680.0], None);
    input.events.push(egui::Event::Text("must-not-reach-terminal".into()));
    input.events.push(egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: Some(egui::Key::Escape),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    input
        .dropped_files
        .push(crate::app::test_support::dropped_file("/tmp/never-open-this-file"));
    let _ = ctx
        .run_ui(input, |ui| {
            assert!(app.render_remote_environments(ui).is_some());
            assert!(!app.remote_environments.open);
            ui.input(|input| {
                assert!(input.events.is_empty());
                assert!(input.raw.events.is_empty());
                assert!(input.raw.dropped_files.is_empty());
                assert!(input.keys_down.is_empty());
            });
            assert!(app.terminal_keyboard_events.is_empty());
            app.handle_shortcuts(ui);
            assert!(app.board.panels.is_empty());
        })
        .discard_textures();
}

#[test]
fn loader_finds_owned_record_without_creating_a_local_session() {
    use crate::app::test_support::raw_input;
    use crate::test_egui::DiscardTextures;
    use horizon_core::remote_workspace::RemoteWorkspaceState;
    let temp = tempfile::tempdir().expect("tempdir");
    let home = HorizonHome::from_root(temp.path().join("home"));
    let store = CloudWorkflowStore::open(&home).expect("store");
    let owner = "00000000-0000-4000-8000-000000000001".to_string();
    let state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
        "version": 1,
        "spec": {
            "workspace_local_id": "detached-workspace",
            "target": { "provider": "local_docker", "profile": "development",
                "image": format!("example/worker@sha256:{}", "a".repeat(64)),
                "disk_gib": 20, "lifetime": "persistent" },
            "repository": { "repository": "example/project", "commit": "b".repeat(40) },
            "working_directory": ".", "generation": 0, "panels": []
        }
    }))
    .expect("state");
    let stored = store.create_remote_workspace(&owner, &state).expect("record");
    let page = load_page(&home, None).expect("page");
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].summary, stored.environment_summary());
    let ctx = Context::default();
    let mut view = RemoteEnvironments {
        open: true,
        page: Some(empty_page(None)),
        ..Default::default()
    };
    let render = |view: &RemoteEnvironments| {
        for _ in 0..2 {
            let _ = ctx
                .run_ui(raw_input([900.0, 680.0], None), |ui| {
                    let _ = paint::show(ui, view);
                })
                .discard_textures();
        }
        ctx.data(|data| data.get_temp::<egui::Rect>(egui::Id::new("inventory-close-test")))
            .expect("close control")
            .top()
    };
    let empty_top = render(&view);
    view.page = Some(page);
    assert!(
        render(&view) < empty_top - 100.0,
        "populated dialog must grow beyond its cached empty height"
    );
    view.failure = Some(LoadFailure {
        error: LoadError::ReadPage,
        cursor: None,
    });
    assert!(render(&view) >= 32.0, "error controls must remain inside the viewport");
    assert!(!home.sessions_dir().exists());
    assert_eq!(
        store.load_remote_workspace(&owner, "detached-workspace").expect("load"),
        Some(stored)
    );
}

#[test]
fn close_click_keeps_pointer_state_across_press_idle_and_release_frames() {
    use crate::app::test_support::{raw_input, test_app};
    use crate::test_egui::DiscardTextures;
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    app.remote_environments.open = true;
    app.remote_environments.page = Some(empty_page(None));
    let mut frame = |events| {
        let mut input = raw_input([900.0, 680.0], None);
        input.events = events;
        let _ = ctx
            .run_ui(input, |ui| {
                let saved = app.render_remote_environments(ui).expect("modal frame");
                assert!(!ui.input(|input| input.pointer.primary_down()));
                HorizonApp::restore_remote_environment_input(ui, saved);
            })
            .discard_textures();
    };
    frame(Vec::new());
    frame(Vec::new());
    let position = ctx
        .data(|data| data.get_temp::<egui::Rect>(egui::Id::new("inventory-close-test")))
        .expect("close control")
        .center();
    let button = |pressed| egui::Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(vec![egui::Event::PointerMoved(position), button(true)]);
    frame(Vec::new());
    assert!(ctx.input(|input| input.pointer.primary_down()));
    frame(vec![egui::Event::PointerMoved(position)]);
    frame(vec![button(false)]);
    assert!(!app.remote_environments.open);
}
