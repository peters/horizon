use crate::{Board, CanvasViewState, PanelKind, RuntimeState, WindowConfig};

#[test]
fn feature_disabled_board_roundtrip_preserves_opaque_clouds_and_empty_workspaces() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("must-not-spawn");
    let opaque = serde_json::json!([
        {"issue": 1, "workspace": "cloud-space", "panels": ["cloud-member"], "remote": {"id": "worker-id", "revision": "pinned", "future_state": {"preserve": true}}, "unknown_field": [1,2,3]},
        {"issue": 2, "workspace": "empty-cloud-space", "panels": [], "remote": {"id": "empty-worker"}}
    ]);
    let mut state: RuntimeState = serde_json::from_value(serde_json::json!({
        "cloud_groups": opaque,
        "workspaces": [
            {"local_id": "cloud-space", "name": "Cloud", "panels": [{"local_id": "cloud-member", "name": "Remote session", "kind": PanelKind::Shell, "command": "/bin/sh", "args": ["-c", "touch \"$1\"", "fixture", marker]}]},
            {"local_id": "empty-cloud-space", "name": "Empty cloud", "panels": []}
        ]
    })).unwrap();
    for _ in 0..2 {
        let mut board = Board::from_runtime_state(&state).unwrap();
        board.remove_empty_workspaces();
        assert_eq!(board.workspaces.len(), 2);
        assert_eq!(board.panels.len(), 1);
        assert!(board.restart_panel(board.panels[0].id).is_err());
        assert!(
            board.panels[0]
                .terminal()
                .unwrap()
                .last_lines_text(30)
                .contains("Cloud support is disabled")
        );
        assert!(
            !marker.exists(),
            "feature-disabled restore must not launch a cloud command"
        );
        state = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(value["cloud_groups"], opaque);
        assert_eq!(value["workspaces"][0]["panels"][0]["local_id"], "cloud-member");
        state = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    }
}

#[test]
fn legacy_no_cloud_state_writes_a_compatible_empty_array() {
    let state: RuntimeState = serde_json::from_str("{}").unwrap();
    assert_eq!(
        serde_json::to_value(state).unwrap()["cloud_groups"],
        serde_json::json!([])
    );
}

#[test]
fn disabled_cloud_membership_survives_remove_and_move_requests() {
    let state: RuntimeState = serde_json::from_value(serde_json::json!({
        "cloud_groups": [{"workspace":"cloud", "panels":["member"], "remote":{}}],
        "workspaces":[
            {"local_id":"cloud", "panels":[{"local_id":"member", "kind":PanelKind::Shell}]},
            {"local_id":"local", "panels":[{"local_id":"ordinary", "kind":PanelKind::Shell, "command":"/bin/true"}]}
        ]
    }))
    .unwrap();
    let mut board = Board::from_runtime_state(&state).unwrap();
    let cloud = board.workspace_id_by_local_id("cloud").unwrap();
    let local = board.workspace_id_by_local_id("local").unwrap();
    let member = board.panels.iter().find(|p| p.local_id == "member").unwrap().id;
    let ordinary = board.panels.iter().find(|p| p.local_id == "ordinary").unwrap().id;
    board.remove_workspace(cloud);
    assert!(board.workspace(cloud).is_some());
    board.assign_panel_to_workspace(member, local);
    board.assign_panel_to_workspace(ordinary, cloud);
    assert_eq!(board.panel(member).unwrap().workspace_id, cloud);
    assert_eq!(board.panel(ordinary).unwrap().workspace_id, local);
    board.remove_workspace(local);
    assert!(board.workspace(local).is_some(), "only a cloud destination remains");
    let next = board.create_workspace("Other local");
    board.assign_panel_to_workspace(ordinary, next);
    assert_eq!(board.panel(ordinary).unwrap().workspace_id, next);
    board.assign_panel_to_workspace(ordinary, local);
    board.remove_workspace(local);
    assert!(board.workspace(local).is_none());
    assert_eq!(board.panel(ordinary).unwrap().workspace_id, next);
    board.active_workspace = Some(cloud);
    assert_eq!(board.ensure_local_workspace("Fallback"), next);
}
