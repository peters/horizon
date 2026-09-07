use super::*;

#[test]
fn handoff_can_be_delivered_from_a_background_thread() {
    fn assert_send<T: Send>() {}
    assert_send::<PreparedRemotePanelHandoff>();
}

#[cfg(target_os = "linux")]
#[test]
fn expired_handoff_preserves_the_disconnected_view() {
    use crate::{CanvasViewState, PanelKind, PanelState, RuntimeState, WindowConfig, WorkspaceState};

    let owner = "00000000-0000-4000-8000-000000000001";
    let reference = RemoteWorkspaceReference::new(owner.into(), "workspace".into()).expect("reference");
    let state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "visual-workspace".into(),
            remote_workspace: Some(reference.clone()),
            panels: vec![PanelState {
                local_id: "terminal".into(),
                kind: PanelKind::Ssh,
                remote_workspace: Some(reference),
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let mut board = Board::from_runtime_state(&state).expect("inert view");
    assert!(board.panels[0].wait_for_shutdown(Duration::from_secs(2)));
    board.panels[0].process_output();
    let panel_id = board.panels[0].id;
    let before = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
    let handoff = PreparedRemotePanelHandoff {
        owner_session_id: owner.into(),
        workspace_local_id: "workspace".into(),
        panel_local_id: "terminal".into(),
        terminal: Terminal::spawn(crate::terminal::TerminalSpawnOptions {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "exit 0".into()],
            cwd: None,
            rows: 24,
            cols: 80,
            cell_width: 8,
            cell_height: 16,
            scrollback_limit: 100,
            window_id: 0,
            replay_bytes: Vec::new(),
            env: std::collections::HashMap::new(),
            kitty_keyboard: false,
        })
        .expect("owned transport"),
        admitted_at: Instant::now()
            .checked_sub(MAX_HANDOFF_AGE + Duration::from_secs(1))
            .expect("expired fixture instant"),
    };
    let debug = format!("{handoff:?}");
    assert!(!debug.contains(owner) && !debug.contains("workspace") && !debug.contains("terminal"));
    assert_eq!(
        board.adopt_remote_panel_connection(owner, panel_id, handoff),
        Err(RemotePanelHandoffError::Expired)
    );
    let after = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
    assert_eq!(after.to_yaml().expect("after"), before.to_yaml().expect("before"));
    assert!(board.panels[0].terminal().expect("inert view").child_exited());
}
