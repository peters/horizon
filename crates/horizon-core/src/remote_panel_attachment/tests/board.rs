use super::{Fixture, OWNER, invalidate, local_options, running};
use crate::{
    Board, CanvasViewState, PanelId, PanelKind, PanelResume, PreparedRemotePanelHandoff, RemotePanelHandoffError,
    SshConnectionStatus,
    remote_panel_attachment::{RemotePanelAttachError, RemotePanelConnectionAttempt},
    runtime_state::{PanelState, RemoteWorkspaceReference, RuntimeState, WorkspaceState},
};
use std::time::{Duration, Instant};

fn board(fixture: &Fixture) -> Board {
    let reference = RemoteWorkspaceReference::new(OWNER.into(), "workspace".into()).expect("reference");
    let marker = fixture.directory.path().join("never-start-locally");
    let state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "remote-view".into(),
            name: "Retained remote view".into(),
            remote_workspace: Some(reference.clone()),
            panels: vec![PanelState {
                local_id: "terminal".into(),
                name: "Custom remote task".into(),
                kind: PanelKind::Ssh,
                remote_workspace: Some(reference),
                command: Some("/bin/sh".into()),
                args: vec![
                    "-c".into(),
                    ": > \"$1\"".into(),
                    "fixture".into(),
                    marker.to_string_lossy().into_owned(),
                ],
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let mut board = Board::from_runtime_state(&state).expect("inert remote view");
    let panel = &mut board.panels[0];
    assert!(panel.wait_for_shutdown(Duration::from_secs(2)));
    panel.process_output();
    assert!(panel.terminal().expect("placeholder").child_exited());
    assert!(!marker.exists());
    board
}

fn attempt(fixture: &Fixture, script: &str) -> RemotePanelConnectionAttempt {
    RemotePanelConnectionAttempt {
        allocation: fixture.current(),
        panel_id: "terminal".into(),
        observed_status: running(),
        terminal: crate::Terminal::spawn(local_options(script)).expect("owned local transport fixture"),
    }
}

fn handoff(fixture: &Fixture, script: &str) -> PreparedRemotePanelHandoff {
    PreparedRemotePanelHandoff::prepare(&fixture.store, attempt(fixture, script)).expect("fresh handoff")
}

fn saved_view(board: &Board) -> String {
    RuntimeState::from_board(
        board,
        crate::config::WindowConfig::default(),
        CanvasViewState::default(),
    )
    .to_yaml()
    .expect("saved view")
}

fn await_output(board: &mut Board, panel_id: PanelId, marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let panel = board.panel_mut(panel_id).expect("view");
        panel.process_output();
        if panel.terminal().expect("terminal").last_lines_text(24).contains(marker) {
            return;
        }
        assert!(Instant::now() < deadline, "local output deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn handoff_preserves_view_and_store_without_promoting_arbitrary_output() {
    let fixture = Fixture::new();
    let mut board = board(&fixture);
    let before_view = saved_view(&board);
    let before_store = fixture.current();
    let panel_id = board.panels[0].id;
    let focus = (board.focused, board.active_workspace);
    board
        .adopt_remote_panel_connection(
            OWNER,
            panel_id,
            handoff(&fixture, "printf 'unverified-transport-output\\n'; read -r fixture"),
        )
        .expect("handoff");
    await_output(&mut board, panel_id, "unverified-transport-output");
    assert_eq!(board.panel(panel_id).expect("view").ssh_status(), None);
    assert_eq!(saved_view(&board), before_view);
    assert_eq!((board.focused, board.active_workspace), focus);
    assert_eq!(fixture.current(), before_store);
    assert!(board.restart_panel(panel_id).is_err());
    assert!(!fixture.directory.path().join("never-start-locally").exists());
    board.shutdown_terminal_panels();
    assert_eq!(fixture.current(), before_store);
}

#[test]
fn copied_references_and_changed_view_identity_do_not_authorize_handoff() {
    for fault in 0..7 {
        let fixture = Fixture::new();
        let mut board = board(&fixture);
        let panel_id = board.panels[0].id;
        let mut current_session = OWNER;
        match fault {
            0 => current_session = "00000000-0000-4000-8000-000000000002",
            1 => board.panels[0].local_id = "different-panel".into(),
            2 => board.panels[0].kind = PanelKind::Shell,
            3 => board.panels[0].resume = PanelResume::Last,
            4 => board.workspaces[0].panels.clear(),
            5 => {
                board.workspaces[0].remote_workspace =
                    Some(RemoteWorkspaceReference::new(OWNER.into(), "different-workspace".into()).expect("reference"));
            }
            6 => board.panels[0].workspace_id = crate::WorkspaceId(u64::MAX),
            _ => unreachable!(),
        }
        let before = fixture.current();
        let original_content = board.panels[0].terminal().expect("placeholder").last_lines_text(24);
        let result = board.adopt_remote_panel_connection(current_session, panel_id, handoff(&fixture, "exit 0"));
        assert_eq!(
            result,
            Err(if fault == 0 {
                RemotePanelHandoffError::ClientSessionMismatch
            } else {
                RemotePanelHandoffError::TargetChanged
            })
        );
        assert_eq!(
            board.panels[0].terminal().expect("placeholder").last_lines_text(24),
            original_content
        );
        assert_eq!(fixture.current(), before);
    }
}

#[test]
fn missing_or_local_panel_cannot_receive_a_remote_connection() {
    let fixture = Fixture::new();
    let mut board = board(&fixture);
    let before = saved_view(&board);
    assert_eq!(
        board.adopt_remote_panel_connection(OWNER, PanelId(u64::MAX), handoff(&fixture, "exit 0")),
        Err(RemotePanelHandoffError::TargetChanged)
    );
    assert_eq!(saved_view(&board), before);
    let local_workspace = board.create_workspace("Local");
    let local = board
        .create_panel(
            crate::PanelOptions {
                kind: PanelKind::Ssh,
                command: Some("/bin/sh".into()),
                args: vec!["-c".into(), "exit 0".into()],
                ..crate::PanelOptions::default()
            },
            local_workspace,
        )
        .expect("local view");
    let before = saved_view(&board);
    assert_eq!(
        board.adopt_remote_panel_connection(OWNER, local, handoff(&fixture, "exit 0")),
        Err(RemotePanelHandoffError::TargetChanged)
    );
    assert_eq!(saved_view(&board), before);
    board.shutdown_terminal_panels();
}

#[test]
fn moved_remote_view_keeps_its_execution_identity_during_handoff() {
    let fixture = Fixture::new();
    let mut board = board(&fixture);
    let panel_id = board.panels[0].id;
    let local_workspace = board.create_workspace("Local visual container");
    board.assign_panel_to_workspace(panel_id, local_workspace);
    let before = saved_view(&board);
    board
        .adopt_remote_panel_connection(OWNER, panel_id, handoff(&fixture, "read -r fixture"))
        .expect("explicit connection to moved remote view");
    assert_eq!(saved_view(&board), before);
    assert_eq!(board.panel_workspace_id(panel_id), Some(local_workspace));
    board.shutdown_terminal_panels();
}

#[test]
fn stale_allocation_or_second_attempt_never_replaces_the_target_transport() {
    let fixture = Fixture::new();
    let mut board = board(&fixture);
    let panel_id = board.panels[0].id;
    let stale = attempt(&fixture, "exit 0");
    invalidate(&fixture.store);
    let before = saved_view(&board);
    assert_eq!(
        PreparedRemotePanelHandoff::prepare(&fixture.store, stale).expect_err("stale allocation"),
        RemotePanelHandoffError::Attachment(RemotePanelAttachError::StateChanged)
    );
    assert_eq!(saved_view(&board), before);
    board
        .adopt_remote_panel_connection(
            OWNER,
            panel_id,
            handoff(
                &fixture,
                "printf 'first-connection\\n'; read -r fixture; printf 'original-still-live\\n'; read -r fixture",
            ),
        )
        .expect("first handoff");
    await_output(&mut board, panel_id, "first-connection");
    assert_eq!(
        board.adopt_remote_panel_connection(OWNER, panel_id, handoff(&fixture, "exit 0")),
        Err(RemotePanelHandoffError::AlreadyConnected)
    );
    board
        .panel(panel_id)
        .expect("view")
        .terminal()
        .expect("transport")
        .write_input(b"fixture\n");
    await_output(&mut board, panel_id, "original-still-live");
    board.shutdown_terminal_panels();
}

#[test]
fn disconnected_transport_accepts_fresh_handoff_but_saved_restore_remains_inert() {
    let fixture = Fixture::new();
    let mut board = board(&fixture);
    let panel_id = board.panels[0].id;
    let before_store = fixture.current();
    board
        .adopt_remote_panel_connection(OWNER, panel_id, handoff(&fixture, "printf 'transport-ended\\n'"))
        .expect("first handoff");
    assert!(
        board
            .panel_mut(panel_id)
            .expect("panel")
            .wait_for_shutdown(Duration::from_secs(2))
    );
    board.panel_mut(panel_id).expect("panel").process_output();
    assert_eq!(
        board.panel(panel_id).expect("panel").ssh_status(),
        Some(SshConnectionStatus::Disconnected)
    );
    board
        .adopt_remote_panel_connection(
            OWNER,
            panel_id,
            handoff(&fixture, "printf 'reconnected-view\\n'; read -r fixture"),
        )
        .expect("fresh handoff after local exit");
    await_output(&mut board, panel_id, "reconnected-view");
    let state = RuntimeState::from_board(
        &board,
        crate::config::WindowConfig::default(),
        CanvasViewState::default(),
    );
    board.close_panel(panel_id);
    assert!(board.panel(panel_id).is_none());
    assert!(
        !board.workspaces.is_empty(),
        "remote workspace remains after its last local view closes"
    );
    assert_eq!(fixture.current(), before_store);
    let mut restored = Board::from_runtime_state(&state).expect("inert reopened view");
    assert!(restored.panels[0].wait_for_shutdown(Duration::from_secs(2)));
    restored.panels[0].process_output();
    assert!(
        restored.panels[0]
            .terminal()
            .expect("placeholder")
            .last_lines_text(24)
            .contains("Remote connection pending")
    );
    assert!(restored.restart_panel(restored.panels[0].id).is_err());
    assert!(!fixture.directory.path().join("never-start-locally").exists());
    assert_eq!(fixture.current(), before_store);
}
