//! The park state and the status line that a saved session keeps in its runtime index.
use super::*;

#[test]
fn a_saved_session_keeps_the_park_state_and_the_last_status_line_in_its_runtime_index() {
    let (_temp, mut app) = ready_cloud();
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
