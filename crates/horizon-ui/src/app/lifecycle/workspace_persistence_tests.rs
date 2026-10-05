use std::time::{Duration, Instant};

use horizon_core::{PanelId, RuntimeState, StartupDecision, WorkspaceId};

use super::super::HorizonApp;
use super::super::test_support::{editor_workspace_state, test_app_with_startup};

fn persistent_app() -> (tempfile::TempDir, HorizonApp, std::path::PathBuf) {
    let state = RuntimeState {
        workspaces: vec![
            editor_workspace_state("source", [0.0, 0.0]),
            editor_workspace_state("target", [800.0, 0.0]),
        ],
        ..RuntimeState::default()
    };
    let (temp, _ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(state.clone()),
    });
    let session = app.session_store.create_session_from_runtime(state).expect("session");
    app.activate_persistent_session(&session);
    app.root_viewport_stabilizer = None;
    app.runtime_dirty_since = None;
    (temp, app, session.runtime_state_path)
}

fn flush_and_restore(app: &mut HorizonApp, path: &std::path::Path) {
    assert!(app.runtime_is_dirty(), "workspace changes must schedule autosave");
    app.runtime_dirty_since = Some(Instant::now().checked_sub(Duration::from_secs(1)).expect("clock"));
    app.flush_runtime_if_dirty();
    assert!(!app.runtime_is_dirty());
    let saved = RuntimeState::load(path)
        .expect("load saved state")
        .expect("saved state");
    app.apply_runtime_state(&saved);
}

#[test]
fn queued_workspace_move_autosaves_membership_before_exit() {
    let (_temp, mut app, path) = persistent_app();
    let panel = app.board.panel_id_by_local_id("source-panel").expect("panel");
    let target = app.board.workspace_id_by_local_id("target").expect("target");
    let workspace_count = app.board.workspaces.len();
    let panel_count = app.board.panels.len();
    app.workspace_assignments.push((panel, target));

    app.apply_pending_workspace_changes();

    assert_eq!(app.board.workspaces.len(), workspace_count);
    assert_eq!(app.board.panels.len(), panel_count);
    assert_eq!(app.board.panel_workspace_id(panel), Some(target));
    flush_and_restore(&mut app, &path);
    let panel = app.board.panel_id_by_local_id("source-panel").expect("restored panel");
    let target = app.board.workspace_id_by_local_id("target").expect("restored target");
    assert_eq!(app.board.panel_workspace_id(panel), Some(target));
    assert_eq!(app.board.panels.len(), panel_count);
}

#[test]
fn queued_new_workspace_autosaves_created_workspace_and_membership() {
    let (_temp, mut app, path) = persistent_app();
    let panel = app.board.panel_id_by_local_id("source-panel").expect("panel");
    app.workspace_creates.push(panel);
    app.apply_pending_workspace_changes();
    let destination = app.board.panel_workspace_id(panel).expect("destination");
    let local_id = app.board.workspace(destination).expect("workspace").local_id.clone();

    flush_and_restore(&mut app, &path);

    let panel = app.board.panel_id_by_local_id("source-panel").expect("restored panel");
    let destination = app
        .board
        .workspace_id_by_local_id(&local_id)
        .expect("restored workspace");
    assert_eq!(app.board.panel_workspace_id(panel), Some(destination));
}

#[test]
fn queued_noop_workspace_moves_do_not_schedule_autosave() {
    let (_temp, mut app, _path) = persistent_app();
    let panel = app.board.panel_id_by_local_id("source-panel").expect("panel");
    let source = app.board.panel_workspace_id(panel).expect("source");
    app.workspace_assignments.extend([
        (panel, source),
        (panel, WorkspaceId(u64::MAX)),
        (PanelId(u64::MAX), source),
    ]);
    app.apply_pending_workspace_changes();
    assert!(!app.runtime_is_dirty());
    assert_eq!(app.board.panel_workspace_id(panel), Some(source));
}

#[test]
fn queued_incompatible_remote_move_does_not_schedule_autosave() {
    let (_temp, mut app, _path) = persistent_app();
    let panel = app.board.panel_id_by_local_id("source-panel").expect("panel");
    let source = app.board.panel_workspace_id(panel).expect("source");
    let target = app.board.workspace_id_by_local_id("target").expect("target");
    app.board.workspace_mut(target).expect("workspace").remote_workspace = Some(
        horizon_core::RemoteWorkspaceReference::new(
            "11111111-1111-4111-8111-111111111111".into(),
            "remote-workspace".into(),
        )
        .expect("remote reference"),
    );
    app.workspace_assignments.push((panel, target));
    app.apply_pending_workspace_changes();
    assert!(!app.runtime_is_dirty());
    assert_eq!(app.board.panel_workspace_id(panel), Some(source));
}

#[cfg(feature = "cloud-workspaces")]
#[test]
fn queued_move_to_cloud_workspace_does_not_schedule_autosave() {
    let (_temp, mut app, _path) = persistent_app();
    let panel = app.board.panel_id_by_local_id("source-panel").expect("panel");
    let source = app.board.panel_workspace_id(panel).expect("source");
    let target = app.board.workspace_id_by_local_id("target").expect("target");
    let local_id = app.board.workspace(target).expect("workspace").local_id.clone();
    app.board
        .cloud_groups
        .0
        .push(horizon_core::cloud_panel::CloudGroup::new(
            1,
            "Cloud".into(),
            local_id,
            std::path::PathBuf::new(),
            [0.0, 0.0],
        ));
    app.workspace_assignments.push((panel, target));
    app.apply_pending_workspace_changes();
    assert!(!app.runtime_is_dirty());
    assert_eq!(app.board.panel_workspace_id(panel), Some(source));
}
