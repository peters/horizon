use std::path::PathBuf;

use crate::cloud_panel::CloudGroup;

use super::super::*;
use super::editor_panel_options;

/// Record a cloud in `workspace` and keep the workspace like the per-frame UI pass does.
fn add_cloud(board: &mut Board, issue: u32, workspace: WorkspaceId) {
    let local_id = board.workspace(workspace).expect("workspace").local_id.clone();
    board.cloud_groups.0.push(CloudGroup::new(
        issue,
        format!("Cloud {issue}"),
        local_id,
        PathBuf::new(),
        [0.0, 0.0],
    ));
    board.retain_workspace_when_empty(workspace);
}

fn remove_cloud(board: &mut Board, issue: u32) {
    board.cloud_groups.0.retain(|group| group.issue != issue);
}

#[test]
fn released_cloud_workspace_is_removed_by_the_next_cleanup() {
    let mut board = Board::new();
    let local = board.create_workspace("local");
    let remaining = board
        .create_panel(editor_panel_options(), local)
        .expect("panel should spawn");
    let cloud = board.create_workspace("cloud");
    add_cloud(&mut board, 1, cloud);
    board.active_workspace = Some(cloud);
    board.focused = None;
    board.remove_empty_workspaces();
    assert!(board.workspace(cloud).is_some(), "an empty cloud keeps its workspace");

    remove_cloud(&mut board, 1);
    assert!(board.release_empty_workspace_retention(cloud), "the hold is released");
    assert!(board.workspace(cloud).is_some(), "removal waits for the cleanup pass");
    board.remove_empty_workspaces();

    assert!(board.workspace(cloud).is_none());
    assert_eq!(board.active_workspace, Some(local));
    assert_eq!(board.focused, Some(remaining));
}

#[derive(Clone, Copy, Debug)]
enum Departure {
    /// The focused last panel of a plain workspace closes.
    LastPanel,
    /// A cloud whose focused member is the workspace's last panel is removed.
    CloudWithMember,
    /// An empty cloud in the active workspace, with nothing focused, is removed.
    EmptyCloud,
}

#[test]
fn removing_a_cloud_moves_focus_like_closing_the_last_panel() {
    let outcome = |departure: Departure| {
        let mut board = Board::new();
        let first = board.create_workspace("first");
        board
            .create_panel(editor_panel_options(), first)
            .expect("panel should spawn");
        let second = board.create_workspace("second");
        let most_recent = board
            .create_panel(editor_panel_options(), second)
            .expect("panel should spawn");
        let target = board.create_workspace("target");
        if !matches!(departure, Departure::LastPanel) {
            add_cloud(&mut board, 1, target);
        }
        if matches!(departure, Departure::EmptyCloud) {
            board.focus_workspace(target);
            assert_eq!(board.focused, None);
        } else {
            let last = board
                .create_panel(editor_panel_options(), target)
                .expect("panel should spawn");
            assert_eq!(board.focused, Some(last));
            board.close_panel(last);
        }
        if !matches!(departure, Departure::LastPanel) {
            remove_cloud(&mut board, 1);
            board.release_empty_workspace_retention(target);
        }
        board.remove_empty_workspaces();
        assert!(board.workspace(target).is_none(), "{departure:?}");
        assert_eq!(board.focused, Some(most_recent), "{departure:?}");
        assert_eq!(board.active_workspace, Some(second), "{departure:?}");
        (board.focused, board.active_workspace, board.workspaces.len())
    };

    let closed = outcome(Departure::LastPanel);
    assert_eq!(outcome(Departure::CloudWithMember), closed);
    assert_eq!(outcome(Departure::EmptyCloud), closed);
}

#[test]
fn releasing_the_active_workspace_of_an_empty_board_selects_the_first_workspace() {
    let mut board = Board::new();
    let first = board.create_workspace("first");
    let cloud = board.create_workspace("cloud");
    add_cloud(&mut board, 1, cloud);
    board.focus_workspace(cloud);
    remove_cloud(&mut board, 1);
    board.release_empty_workspace_retention(cloud);
    board.remove_empty_workspaces();

    assert!(board.workspace(cloud).is_none());
    assert_eq!(board.focused, None);
    assert_eq!(board.active_workspace, Some(first));
}

#[test]
fn workspace_stays_while_another_cloud_or_a_panel_uses_it() {
    let mut board = Board::new();
    let shared = board.create_workspace("shared");
    add_cloud(&mut board, 1, shared);
    add_cloud(&mut board, 2, shared);
    remove_cloud(&mut board, 1);
    assert!(
        !board.release_empty_workspace_retention(shared),
        "cloud 2 still holds it"
    );
    board.remove_empty_workspaces();
    assert!(board.workspace(shared).is_some(), "the other cloud keeps its workspace");

    let mixed = board.create_workspace("mixed");
    add_cloud(&mut board, 3, mixed);
    let hidden = board
        .create_panel(editor_panel_options(), mixed)
        .expect("panel should spawn");
    board.retain_workspace_when_empty(mixed);
    assert!(board.set_panel_visible(hidden, false));
    remove_cloud(&mut board, 3);
    board.release_empty_workspace_retention(mixed);
    board.remove_empty_workspaces();
    assert!(board.workspace(mixed).is_some(), "a hidden panel keeps its workspace");

    board.close_panel(hidden);
    assert!(
        board.workspace(mixed).is_none(),
        "without a cloud it closes with its last panel like any workspace"
    );
}

#[test]
fn close_all_panels_keeps_a_cloud_workspace_until_its_cloud_is_removed() {
    let mut board = Board::new();
    let workspace = board.create_workspace("cloud");
    add_cloud(&mut board, 1, workspace);
    board
        .create_panel(editor_panel_options(), workspace)
        .expect("panel should spawn");
    board.retain_workspace_when_empty(workspace);

    board.close_panels_in_workspace(workspace);
    board.remove_empty_workspaces();
    assert!(board.workspace(workspace).is_some());

    remove_cloud(&mut board, 1);
    board.release_empty_workspace_retention(workspace);
    board.remove_empty_workspaces();
    assert!(board.workspace(workspace).is_none());
}

#[test]
fn legacy_remote_view_stays_after_release() {
    let mut board = Board::new();
    let remote = board.create_workspace("Legacy remote");
    board.workspace_mut(remote).expect("workspace").remote_workspace = Some(
        crate::RemoteWorkspaceReference::new("123e4567-e89b-42d3-a456-426614174000".into(), "remote-workspace".into())
            .expect("reference"),
    );
    board.release_empty_workspace_retention(remote);
    board.remove_empty_workspaces();
    assert!(board.workspace(remote).is_some());
}

#[cfg(feature = "cloud-workspaces")]
#[test]
fn a_restored_cloud_member_says_why_it_waits_instead_of_blaming_its_command() {
    use crate::{CanvasViewState, CloudWait, RuntimeState, WindowConfig};
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("must-not-spawn");
    let mut state: RuntimeState = serde_json::from_value(serde_json::json!({
        "workspaces": [{"local_id": "cloud-space", "name": "Cloud", "panels": [
            {"local_id": "cloud-member", "name": "Claude", "kind": PanelKind::Shell,
                "command": "/bin/sh", "args": ["-c", "touch \"$1\"", "fixture", marker]},
            {"local_id": "broken", "name": "Broken", "kind": PanelKind::Codex, "command": "bad\0codex"}
        ]}]
    }))
    .unwrap();
    let mut group = CloudGroup::new(1, "Cloud 1".into(), "cloud-space".into(), PathBuf::new(), [0.0, 0.0]);
    group.panels.push("cloud-member".into());
    group.remote = Some(
        serde_json::from_value(serde_json::json!({
            "id": "cloud-1", "revision": "a", "profile_name": "cpu",
            "profile": {"provider": "hetzner", "image": "registry.example/worker", "cpu": 2, "memory_gb": 4}
        }))
        .unwrap(),
    );
    state.cloud_groups = crate::cloud_panel::CloudGroups(vec![group]);
    let mut board = Board::from_runtime_state(&state).unwrap();
    let member = board.panel_id_by_local_id("cloud-member").unwrap();
    let text = |board: &Board, id| board.panel(id).unwrap().terminal().unwrap().last_lines_text(30);

    let reconnecting = text(&board, member);
    assert!(
        reconnecting.contains("Horizon is reconnecting the cloud of this panel."),
        "{reconnecting}"
    );
    assert!(reconnecting.contains("Panel: Claude"));
    assert!(!reconnecting.contains("Fix the command or binary"), "{reconnecting}");

    let panel = board.panel_mut(member).unwrap();
    assert!(panel.show_cloud_wait(CloudWait::Stopped).unwrap());
    assert_eq!(panel.cloud_wait(), Some(CloudWait::Stopped));
    assert!(
        !panel.show_cloud_wait(CloudWait::Stopped).unwrap(),
        "the text is already shown"
    );
    let stopped = text(&board, member);
    assert!(stopped.contains("The cloud of this panel is stopped."), "{stopped}");
    assert!(stopped.contains("Choose Resume worker on the cloud card to restore this panel."));
    assert!(!stopped.contains("reconnecting") && !stopped.contains("Fix the command or binary"));
    assert!(!marker.exists(), "a cloud member never starts its command locally");

    // A real command failure keeps its own advice, and no cloud text replaces it.
    let broken = board.panel_id_by_local_id("broken").unwrap();
    assert!(
        !board
            .panel_mut(broken)
            .unwrap()
            .show_cloud_wait(CloudWait::Stopped)
            .unwrap()
    );
    assert!(text(&board, broken).contains("Fix the command or binary, then restart the panel."));

    let saved = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
    let saved_member = saved.workspaces[0]
        .panels
        .iter()
        .find(|panel| panel.local_id == "cloud-member")
        .unwrap();
    assert_eq!(
        saved_member.command.as_deref(),
        Some("/bin/sh"),
        "the member keeps its command"
    );
}

#[cfg(unix)]
#[test]
fn running_cloud_member_parks_with_its_last_screen() {
    use crate::panel::CloudWait;

    let temp = tempfile::tempdir().unwrap();
    let mut board = Board::new();
    let workspace = board.create_workspace("cloud-space");
    let member = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Command,
                command: Some("/bin/sh".into()),
                args: vec!["-c".into(), "printf 'AGENT SCREEN\\n'; sleep 30".into()],
                cwd: Some(temp.path().to_path_buf()),
                ..PanelOptions::default()
            },
            workspace,
        )
        .unwrap();
    let text = |board: &Board| board.panel(member).unwrap().terminal().unwrap().last_lines_text(30);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !text(&board).contains("AGENT SCREEN") {
        assert!(std::time::Instant::now() < deadline, "the member must print its screen");
        board.process_output();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let panel = board.panel_mut(member).unwrap();
    panel.set_parked_agent_status(crate::agents::AgentStatus::Working);
    assert_eq!(
        panel.agent_status(),
        crate::agents::AgentStatus::Idle,
        "only a parked member takes it"
    );
    assert!(panel.park_cloud().unwrap());
    assert_eq!(panel.cloud_wait(), Some(CloudWait::Parked));
    assert!(!panel.park_cloud().unwrap(), "already parked");
    panel.set_parked_agent_status(crate::agents::AgentStatus::Working);
    assert_eq!(panel.agent_status(), crate::agents::AgentStatus::Working);
    board.process_output();
    assert_eq!(
        board.panel(member).unwrap().agent_status(),
        crate::agents::AgentStatus::Working,
        "the placeholder screen must not reset the reported status"
    );
    assert!(text(&board).contains("AGENT SCREEN"), "{}", text(&board));

    // A stop replaces the parked screen, and a later park says why the panel waits.
    let panel = board.panel_mut(member).unwrap();
    assert!(panel.show_cloud_wait(CloudWait::Stopped).unwrap());
    assert!(panel.park_cloud().unwrap());
    assert_eq!(panel.cloud_wait(), Some(CloudWait::Parked));
    assert!(text(&board).contains("This panel is parked."), "{}", text(&board));
}

#[cfg(unix)]
#[test]
fn a_parked_snapshot_keeps_blank_rows_and_the_scrolled_viewport() {
    let temp = tempfile::tempdir().unwrap();
    let mut board = Board::new();
    let workspace = board.create_workspace("cloud-space");
    let spawn = |board: &mut Board, script: &str| {
        board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Command,
                    command: Some("/bin/sh".into()),
                    args: vec!["-c".into(), script.into()],
                    cwd: Some(temp.path().to_path_buf()),
                    ..PanelOptions::default()
                },
                workspace,
            )
            .unwrap()
    };
    let blank = spawn(&mut board, "printf 'TOP\\n\\n\\nAFTER-BLANKS\\n'; sleep 30");
    let scrolled = spawn(&mut board, "seq 1 300; sleep 30");
    let viewport = |board: &Board, id| board.panel(id).unwrap().terminal().unwrap().viewport_text();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !(viewport(&board, blank).iter().any(|row| row == "AFTER-BLANKS")
        && viewport(&board, scrolled).iter().any(|row| row == "300"))
    {
        assert!(std::time::Instant::now() < deadline, "both panels must print");
        board.process_output();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(viewport(&board, blank)[..4], ["TOP", "", "", "AFTER-BLANKS"]);
    board
        .panel_mut(scrolled)
        .unwrap()
        .terminal_mut()
        .unwrap()
        .set_scrollback(100);
    let before = viewport(&board, scrolled);
    assert!(!before.iter().any(|row| row == "300"), "the viewport is scrolled back");

    for id in [blank, scrolled] {
        let shown = viewport(&board, id);
        assert!(board.panel_mut(id).unwrap().park_cloud().unwrap());
        assert_eq!(viewport(&board, id), shown, "the parked panel shows what the user saw");
    }
    assert_eq!(viewport(&board, scrolled), before);
}
