use crate::board::WorkspaceLayout;
use crate::panel::{PanelKind, PanelOptions, PanelResume};
use crate::runtime_state::{PanelState, RuntimeState, WorkspaceState};

use super::super::Board;

fn shell_panel_state(local_id: &str, cwd: Option<&std::path::Path>) -> PanelState {
    PanelState {
        local_id: local_id.to_string(),
        remote_workspace: None,
        name: "Shell".to_string(),
        name_is_custom: None,
        kind: PanelKind::Shell,
        command: Some("/bin/sh".to_string()),
        args: vec!["-c".to_string(), "exit".to_string()],
        cwd: cwd.map(|path| path.display().to_string()),
        ssh_connection: None,
        rows: 24,
        // Wide enough that a temporary directory path stays on one line.
        cols: 200,
        work_resume: crate::agent_work::ResumePolicy::default(),
        resume: PanelResume::Fresh,
        position: Some([0.0, 40.0]),
        size: Some([320.0, 220.0]),
        session_binding: None,
        template: None,
        editor_content: None,
        device_identity: None,
        browser_profile: None,
        browser_url: None,
    }
}

#[test]
fn a_new_panel_says_why_it_cannot_start_in_its_workspace_directory() {
    let state = tempfile::tempdir().expect("owned fixture root");
    let missing = state.path().join("renamed-away");
    let mut board = Board::new();
    let workspace_id = board.create_workspace("moved");
    board.workspace_mut(workspace_id).expect("workspace").cwd = Some(missing.clone());

    let panel_id = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Shell,
                command: Some("/bin/sh".to_string()),
                args: vec!["-c".to_string(), "exit".to_string()],
                cols: 200,
                ..PanelOptions::default()
            },
            workspace_id,
        )
        .expect("placeholder panel");

    let text = board
        .panel(panel_id)
        .expect("panel")
        .terminal()
        .expect("placeholder terminal")
        .last_lines_text(24);
    assert!(text.contains("Horizon could not start this panel"), "{text}");
    assert!(text.contains(&format!("Directory: {}", missing.display())), "{text}");
    assert!(text.contains("Reason: the directory does not exist"), "{text}");
    assert!(
        text.contains("Set the directory of this workspace, then restart the panel"),
        "{text}"
    );
}

#[test]
fn a_restored_panel_says_why_it_cannot_use_its_saved_directory() {
    let state = tempfile::tempdir().expect("owned fixture root");
    let missing = state.path().join("renamed-away");
    let runtime = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "Moved".to_string(),
            remote_workspace: None,
            cwd: Some(missing.display().to_string()),
            position: Some([0.0, 40.0]),
            template: None,
            layout: Some(WorkspaceLayout::Grid),
            panels: vec![shell_panel_state("shell", Some(&missing))],
        }],
        ..RuntimeState::default()
    };

    let board = Board::from_runtime_state(&runtime).expect("board");

    let panel_id = board.panel_id_by_local_id("shell").expect("restored panel");
    let text = board
        .panel(panel_id)
        .expect("panel")
        .terminal()
        .expect("placeholder terminal")
        .last_lines_text(24);
    assert!(text.contains("Horizon could not start this panel"), "{text}");
    assert!(text.contains(&format!("Directory: {}", missing.display())), "{text}");
}

#[test]
fn a_restart_in_a_directory_that_is_gone_explains_itself() {
    let state = tempfile::tempdir().expect("owned fixture root");
    let checkout = state.path().join("checkout");
    std::fs::create_dir(&checkout).expect("fixture checkout");
    let mut board = Board::new();
    let workspace_id = board.create_workspace("moved");
    board.workspace_mut(workspace_id).expect("workspace").cwd = Some(checkout.clone());
    let panel_id = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Shell,
                command: Some("/bin/sh".to_string()),
                args: vec!["-c".to_string(), "exit".to_string()],
                cols: 200,
                ..PanelOptions::default()
            },
            workspace_id,
        )
        .expect("panel in an existing directory");
    std::fs::remove_dir(&checkout).expect("the directory goes away");

    board
        .restart_panel(panel_id)
        .expect("the panel reports why it cannot start");

    let text = board
        .panel(panel_id)
        .expect("panel")
        .terminal()
        .expect("placeholder terminal")
        .last_lines_text(24);
    assert!(text.contains("Horizon could not start this panel"), "{text}");
    assert!(text.contains(&format!("Directory: {}", checkout.display())), "{text}");
    assert!(text.contains("Reason: the directory does not exist"), "{text}");
    board.shutdown_terminal_panels();
}

#[test]
fn a_restart_follows_the_directory_the_person_set_for_the_workspace() {
    let state = tempfile::tempdir().expect("owned fixture root");
    let checkout = state.path().join("youpark.no");
    std::fs::create_dir(&checkout).expect("fixture checkout");
    let mut board = Board::new();
    let workspace_id = board.create_workspace("moved");
    board.workspace_mut(workspace_id).expect("workspace").cwd = Some(checkout.clone());
    let panel_id = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Shell,
                command: Some("/bin/sh".to_string()),
                args: vec!["-c".to_string(), "exit".to_string()],
                cols: 200,
                ..PanelOptions::default()
            },
            workspace_id,
        )
        .expect("panel in an existing directory");
    let renamed = state.path().join("youpark.no-old");
    std::fs::rename(&checkout, &renamed).expect("the directory moves");
    board
        .restart_panel(panel_id)
        .expect("the panel reports why it cannot start");

    // The person sets the directory of the workspace, as the panel says.
    assert!(board.set_workspace_directory(workspace_id, Some(&renamed)));
    board
        .restart_panel(panel_id)
        .expect("the panel starts in the new directory");

    let panel = board.panel(panel_id).expect("panel");
    assert_eq!(panel.launch_cwd.as_ref(), Some(&renamed));
    let text = panel.terminal().expect("terminal").last_lines_text(24);
    assert!(!text.contains("Horizon could not start this panel"), "{text}");
    board.shutdown_terminal_panels();
}

#[test]
fn a_panel_with_its_own_directory_keeps_it_when_the_workspace_directory_changes() {
    let state = tempfile::tempdir().expect("owned fixture root");
    let workspace_directory = state.path().join("workspace");
    let own_directory = state.path().join("own");
    let other_directory = state.path().join("other");
    for directory in [&workspace_directory, &own_directory, &other_directory] {
        std::fs::create_dir(directory).expect("fixture directory");
    }
    let mut board = Board::new();
    let workspace_id = board.create_workspace("moved");
    board.workspace_mut(workspace_id).expect("workspace").cwd = Some(workspace_directory.clone());
    let own_panel = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Shell,
                command: Some("/bin/sh".to_string()),
                args: vec!["-c".to_string(), "exit".to_string()],
                cwd: Some(own_directory.clone()),
                cols: 200,
                ..PanelOptions::default()
            },
            workspace_id,
        )
        .expect("panel with its own directory");

    board.set_workspace_directory(workspace_id, Some(&other_directory));

    assert_eq!(
        board.panel(own_panel).expect("panel").launch_cwd.as_ref(),
        Some(&own_directory),
        "a panel with its own directory keeps it"
    );
    board.shutdown_terminal_panels();
}

#[test]
fn setting_the_same_workspace_directory_reports_no_change() {
    let state = tempfile::tempdir().expect("owned fixture root");
    let mut board = Board::new();
    let workspace_id = board.create_workspace("moved");
    board.workspace_mut(workspace_id).expect("workspace").cwd = Some(state.path().to_path_buf());

    assert!(!board.set_workspace_directory(workspace_id, Some(state.path())));
    assert!(!board.set_workspace_directory(workspace_id, None));
    assert!(board.set_workspace_directory(workspace_id, Some(&state.path().join("elsewhere"))));
    assert_eq!(
        board.workspace(workspace_id).expect("workspace").cwd.as_deref(),
        Some(state.path().join("elsewhere").as_path())
    );
}
