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
