//! The park state and the status line that a saved session keeps in its runtime index.
use super::*;

/// Saves the board of `app` as a new session and makes it the active persistent session.
fn persist(app: &mut HorizonApp) -> String {
    let saved = RuntimeState::from_board(
        &app.board,
        horizon_core::WindowConfig::default(),
        horizon_core::CanvasViewState::default(),
    );
    let session = app.session_store.create_session_from_runtime(saved).unwrap().session_id;
    app.active_session = Some(crate::app::ActiveSession {
        session_id: session.clone(),
        lease: None,
        last_lease_refresh: None,
        persistent: true,
    });
    session
}

#[test]
fn a_saved_session_keeps_the_park_state_and_the_last_status_line_in_its_runtime_index() {
    let (_temp, mut app) = ready_cloud();
    let session = persist(&mut app);
    let recorded = |app: &HorizonApp| app.session_store.cloud_panel_statuses(&session).unwrap();
    app.board.focused = None;
    app.sync_cloud_presentations();
    assert_eq!(recorded(&app).len(), MEMBERS.len());
    assert!(recorded(&app).iter().all(|status| status.parked));

    app.cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .parking
        .statuses
        .insert(
            "one".into(),
            SessionStatus {
                id: "one".into(),
                activity: SessionActivity::Idle,
                quiet_for: Some(Duration::from_secs(5)),
                lines: vec!["synthetic prompt".into()],
            },
        );
    app.record_cloud_parking(0);
    show(&mut app, "one");
    app.sync_cloud_parking();
    app.sync_cloud_parking();

    let statuses = recorded(&app);
    assert!(statuses.iter().all(|status| !status.parked), "the cloud attached again");
    let one = statuses.iter().find(|status| status.panel_local_id == "one").unwrap();
    assert_eq!(one.activity, Some(SessionActivity::Idle));
    assert_eq!(one.last_line.as_deref(), Some("synthetic prompt"));
}

#[test]
fn a_later_session_restore_in_a_parked_cloud_records_the_new_panel_as_parked() {
    let (_temp, mut app) = ready_cloud();
    let session = persist(&mut app);
    app.board.focused = None;
    app.sync_cloud_presentations();
    let cloud = app.cloud_prototype.production.runtimes.get_mut(&1).unwrap();
    cloud
        .state
        .as_mut()
        .unwrap()
        .sessions
        .push(super::super::super::Session {
            panel_id: "late".into(),
            agent: "shell".into(),
            tmux: "late".into(),
            branch: String::new(),
            worktree: "/workspace/checkout".into(),
        });
    cloud.pending_session_attachments.insert("late".into());
    cloud.next_attachment_attempt = None;
    app.sync_cloud_presentations();

    let statuses = app.session_store.cloud_panel_statuses(&session).unwrap();
    let late = statuses.iter().find(|status| status.panel_local_id == "late");
    assert!(late.expect("the late panel is recorded").parked);
}
