use crate::app::upkeep::form::Bot;
use crate::app::{sidebar::NewWorkspace, test_support::test_app};

#[test]
fn new_upkeep_opens_its_dialog_and_makes_a_workspace_only_once_it_starts() {
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    let before = app.board.workspaces.len();
    app.create_new_workspace(&ctx, NewWorkspace::Upkeep, None);
    assert!(app.upkeep.form.is_none(), "not without cloud workspaces");
    app.cloud_prototype.ready = true;
    app.create_new_workspace(&ctx, NewWorkspace::Upkeep, None);
    assert!(app.upkeep.form.is_some());
    assert_eq!(app.board.workspaces.len(), before, "New Upkeep makes no workspace yet");
}

#[test]
fn a_start_that_cannot_begin_keeps_the_dialog_open_with_the_reason() {
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    app.cloud_prototype.ready = true;
    app.open_upkeep_dialog(&ctx);
    let form = app.upkeep.form.as_mut().unwrap();
    form.selected.insert("acme/web".into());
    form.bots.insert(Bot::Renovate);
    let config = form.config();
    form.starting = true;
    let refused = app.start_upkeep(&ctx, None, config);
    assert_eq!(
        refused,
        Err("Open a saved session from Sessions before starting a cloud.".into())
    );
    app.upkeep_started(refused);
    let form = app.upkeep.form.as_ref().unwrap();
    assert!(!form.starting);
    assert_eq!(
        form.error.as_deref(),
        Some("Open a saved session from Sessions before starting a cloud.")
    );
    assert!(app.upkeep.preparing.is_none());
    app.upkeep_started(Ok(()));
    assert!(app.upkeep.form.is_none());
}

#[test]
fn a_dialog_closed_while_starting_shows_again_with_the_failure() {
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    app.cloud_prototype.ready = true;
    app.open_upkeep_dialog(&ctx);
    let form = app.upkeep.form.as_mut().unwrap();
    form.starting = true;
    form.hidden = true;
    app.open_upkeep_dialog(&ctx);
    let form = app.upkeep.form.as_mut().unwrap();
    assert!(!form.hidden && form.starting, "New Upkeep shows the start under way");
    form.hidden = true;
    app.upkeep_started(Err("Cloning acme/web failed".into()));
    let form = app.upkeep.form.as_ref().unwrap();
    assert!(!form.hidden && !form.starting);
    assert_eq!(form.error.as_deref(), Some("Cloning acme/web failed"));
}

#[test]
fn a_second_start_while_one_prepares_is_refused_not_dropped() {
    let (_temp, mut app) = test_app();
    let (_tx, receiver) = std::sync::mpsc::channel();
    app.upkeep.preparing = Some(crate::app::upkeep::Preparing {
        receiver,
        cloud_id: "c1".into(),
        session: None,
        workspace: None,
    });
    let ctx = egui::Context::default();
    let config = horizon_core::upkeep::worker::Config::default();
    assert_eq!(
        app.start_upkeep(&ctx, None, config),
        Err("An Upkeep is still starting. Wait until its cloud appears.".into())
    );
}

#[test]
fn an_earlier_clone_is_found_whatever_the_case_of_its_folders() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("Acme/Web")).unwrap();
    assert_eq!(super::spelled(temp.path(), "acme/web").as_deref(), Some("Acme/Web"));
    assert_eq!(super::spelled(temp.path(), "acme/api"), None);
    assert_eq!(super::spelled(temp.path(), "other/web"), None);
}

#[test]
fn a_start_from_another_session_adds_no_cloud() {
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    app.cloud_prototype.ready = true;
    app.open_upkeep_dialog(&ctx);
    app.upkeep.form.as_mut().unwrap().starting = true;
    let (tx, receiver) = std::sync::mpsc::channel();
    tx.send(Ok(crate::app::upkeep::Prepared {
        path: "/nowhere".into(),
        revision: "0".repeat(40),
        run_token: 1,
    }))
    .unwrap();
    app.upkeep.preparing = Some(crate::app::upkeep::Preparing {
        receiver,
        cloud_id: "upkeep-test-no-such-cloud".into(),
        session: Some("another-session".into()),
        workspace: None,
    });
    let workspaces = app.board.workspaces.len();
    app.poll_upkeep_start(&ctx);
    assert!(app.upkeep.preparing.is_none());
    assert_eq!(app.board.workspaces.len(), workspaces);
    assert_eq!(
        app.upkeep.form.as_ref().unwrap().error.as_deref(),
        Some("The session changed while the repository was prepared. Start the Upkeep again.")
    );
}

fn upkeep(app: &mut crate::app::HorizonApp, board: horizon_core::PanelId) -> &mut horizon_core::upkeep::UpkeepBoard {
    app.board.panel_mut(board).unwrap().content.upkeep_mut().unwrap()
}

#[test]
fn the_board_of_a_saved_upkeep_runs_now_and_pauses_through_its_exchange() {
    use horizon_core::upkeep::{Request, worker};
    use horizon_core::{PanelKind, PanelOptions};

    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    let workspace = app.board.create_workspace("Upkeep");
    let status = worker::directory("upkeep-test-board").join("status.json");
    let board = app
        .board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Upkeep,
                command: Some(status.display().to_string()),
                ..PanelOptions::default()
            },
            workspace,
        )
        .unwrap();
    let second = app
        .board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Upkeep,
                command: Some(status.display().to_string()),
                ..PanelOptions::default()
            },
            workspace,
        )
        .unwrap();
    let saved = crate::app::upkeep::Saved::new(vec![("upkeep-test-board".into(), worker::Config::default())]);
    app.link_upkeep_boards(&saved);
    assert_eq!(upkeep(&mut app, board).cloud.as_deref(), Some("upkeep-test-board"));

    // As the board does when Run now is pressed.
    let pressed = (
        horizon_core::upkeep::time::OffsetDateTime::now_utc(),
        std::time::Instant::now(),
    );
    upkeep(&mut app, board).run_requested = Some(pressed);
    upkeep(&mut app, board).request = Some(Request::RunNow);
    app.apply_upkeep_requests(&ctx);
    let sync = &app.upkeep.syncs["upkeep-test-board"];
    assert!(sync.signals.run_now && sync.next.is_none());
    assert_eq!(
        upkeep(&mut app, second).run_requested,
        Some(pressed),
        "another board of the Upkeep waits for the same run"
    );

    upkeep(&mut app, board).request = Some(Request::Pause(true));
    app.apply_upkeep_requests(&ctx);
    assert_eq!(app.upkeep.syncs["upkeep-test-board"].signals.paused, Some(true));

    app.link_upkeep_boards(&crate::app::upkeep::Saved::default());
    assert_eq!(
        upkeep(&mut app, board).cloud,
        None,
        "a board whose Upkeep is gone offers nothing"
    );
}
