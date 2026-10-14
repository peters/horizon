use crate::app::test_support::test_app;
use crate::app::upkeep::form::Bot;

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
fn an_agent_proposal_opens_new_upkeep_filled_in_for_its_workspace() {
    use crate::app::upkeep::form::Every;
    use horizon_core::upkeep::worker::{Checkouts, Config};

    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("Agents");
    let config = Config {
        repositories: vec!["Acme/Web".into(), "acme/api".into()],
        authors: vec!["app/renovate".into(), "someone-else".into()],
        agent: "codex".into(),
        every_minutes: 240,
        instructions: "Be careful.".into(),
        checkouts: Checkouts::Partial,
        ..Config::default()
    };
    assert!(
        app.propose_new_upkeep(workspace, &config).is_err(),
        "not without cloud workspaces"
    );
    app.cloud_prototype.ready = true;
    app.propose_new_upkeep(workspace, &config).unwrap();
    let form = app.upkeep.form.as_ref().unwrap();
    assert_eq!(form.workspace, Some(workspace));
    assert_eq!(form.selected.iter().collect::<Vec<_>>(), ["acme/api", "acme/web"]);
    assert_eq!(form.bots.iter().copied().collect::<Vec<_>>(), [Bot::Renovate]);
    assert_eq!(form.every, Every::FourHours);
    assert_eq!(
        (form.checkouts, form.instructions.as_str()),
        (Checkouts::Partial, "Be careful.")
    );
    assert!(form.stale(), "its first frame searches for repositories");
    let proposal = form.proposed.as_ref().unwrap();
    assert_eq!(
        proposal.name, "Agents",
        "the dialog names the workspace an agent filled it in for"
    );
    assert_eq!(proposal.local_id, app.board.workspace(workspace).unwrap().local_id);
    assert!(
        app.propose_new_upkeep(workspace, &config).is_err(),
        "one New Upkeep at a time"
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
        Some(horizon_core::upkeep::time::OffsetDateTime::now_utc()),
        std::time::Instant::now(),
    );
    upkeep(&mut app, board).run_requested = Some(pressed);
    // A token kept from a failed exchange may be spent on the worker already.
    app.upkeep
        .syncs
        .entry("upkeep-test-board".into())
        .or_default()
        .signals
        .run_token = Some(7);
    upkeep(&mut app, board).request = Some(Request::RunNow);
    app.apply_upkeep_requests(&ctx);
    let sync = &app.upkeep.syncs["upkeep-test-board"];
    assert!(sync.signals.run_now && sync.next.is_none());
    assert_eq!(sync.signals.run_token, None, "a press gets a new token");
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

#[test]
fn saved_settings_are_read_again_and_sent_at_once() {
    let (_temp, mut app) = test_app();
    app.upkeep.saved = Some(std::sync::Arc::default());
    app.upkeep.syncs.entry("c1".into()).or_default().next = Some(std::time::Instant::now());
    app.upkeep_settings_saved("c1");
    assert!(app.upkeep.saved.is_none());
    assert!(app.upkeep.syncs["c1"].next.is_none());
    assert_eq!(app.upkeep.syncs["c1"].loop_running(), None);
}

#[test]
fn an_upkeep_cloud_reaches_only_the_allowlist_and_never_stops_when_idle() {
    let profile = super::profile().unwrap();
    assert_eq!(profile.provider, horizon_core::cloud_runtime::provider::HETZNER.id);
    assert_eq!(profile.egress, Some(horizon_core::cloud_runtime::Egress::default()));
    assert_eq!(profile.idle_stop_minutes, None);
    assert!(profile.validate(false).is_ok());
}
