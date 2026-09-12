use super::*;
use crate::board::vec2_eq;
use crate::{CanvasViewState, PanelResume, RuntimeState, WindowConfig, WorkspaceId};

const OWNER: &str = "00000000-0000-4000-8000-000000000001";
const FOREIGN: &str = "00000000-0000-4000-8000-000000000002";

struct Fixture {
    _directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    record: StoredRemoteWorkspace,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        let store = CloudWorkflowStore::open_path(directory.path().join("private/state.sqlite3")).expect("store");
        let state = serde_json::from_value(serde_json::json!({
            "version": 1, "spec": {
                "workspace_local_id": "remote-workspace", "working_directory": "nested", "generation": 0,
                "target": {"provider": "local_docker", "profile": "development", "disk_gib": 20,
                    "lifetime": "persistent", "image": format!("example/worker@sha256:{}", "a".repeat(64))},
                "repository": {"repository": "example/project", "commit": "b".repeat(40)},
                "panels": [
                    {"panel_local_id": "alpha", "kind": "command", "working_directory": "nested",
                     "command": {"program": "must-not-execute-private-marker", "args": ["private-argument"]},
                     "task_handoff": "private-task"},
                    {"panel_local_id": "bravo", "kind": "claude", "agent_session_id": "private-agent-session"}
                ]
            }
        }))
        .expect("saved intent");
        let record = store.create_remote_workspace(OWNER, &state).expect("record");
        Self {
            _directory: directory,
            store,
            record,
        }
    }

    fn catalog(&self) -> RemoteViewCatalog {
        RemoteViewCatalog::load(&self.store, OWNER, &self.record.environment_summary()).expect("catalog")
    }

    fn prepare(&self, board: &Board, id: &str) -> PreparedRemoteViewReopen {
        let request = board
            .request_remote_view_reopen(OWNER, &self.catalog(), id)
            .expect("request");
        let store = self.store.clone();
        std::thread::spawn(move || request.prepare(&store))
            .join()
            .expect("worker")
            .expect("prepared view")
    }

    fn assert_unchanged(&self) {
        assert_eq!(
            self.store
                .load_remote_workspace(OWNER, "remote-workspace")
                .expect("read"),
            Some(self.record.clone())
        );
        assert!(
            self.store
                .load_remote_allocation(OWNER, "remote-workspace")
                .expect("allocation")
                .is_none()
        );
    }
}

fn snapshot(board: &Board) -> (String, u64, u64) {
    (
        serde_json::to_string(&RuntimeState::from_board(
            board,
            WindowConfig::default(),
            CanvasViewState::default(),
        ))
        .expect("snapshot"),
        board.next_panel_id,
        board.next_workspace_id,
    )
}

fn matching_workspace(board: &mut Board) -> WorkspaceId {
    let id = board.create_workspace("Existing remote views");
    board.workspace_mut(id).expect("workspace").remote_workspace =
        Some(RemoteWorkspaceReference::new(OWNER.into(), "remote-workspace".into()).expect("reference"));
    id
}

#[test]
fn catalog_caches_only_static_shell_start_eligibility() {
    let mut fixture = Fixture::new();
    let mut state = fixture.record.state().clone();
    let mut shell = state.spec.panels[0].clone();
    shell.kind = PanelKind::Shell;
    shell.task_handoff = None;
    shell.agent_session_id = Some("private-agent-marker".into());
    assert!(!saved_shell_start_eligible(&shell));
    shell.agent_session_id = None;
    state.spec.panels = [
        "shell",
        "command",
        "agent",
        "missing-command",
        "handoff",
        "agent-session",
    ]
    .into_iter()
    .map(|id| {
        let mut panel = shell.clone();
        panel.panel_local_id = id.into();
        match id {
            "command" => panel.kind = PanelKind::Command,
            "agent" => panel.kind = PanelKind::Claude,
            "missing-command" => panel.command = None,
            "handoff" => panel.task_handoff = Some("private-task-marker".into()),
            "agent-session" => {
                panel.kind = PanelKind::Claude;
                panel.agent_session_id = Some("private-agent-marker".into());
            }
            _ => {}
        }
        panel
    })
    .collect();
    fixture.record = fixture
        .store
        .replace_remote_workspace(&fixture.record, &state)
        .expect("bindings");
    let catalog = fixture.catalog();
    assert_eq!(catalog.panel_ids().len(), 6);
    assert_eq!(catalog.shell_start_eligible, [true, false, false, false, false, false]);
    for id in catalog.panel_ids() {
        assert_eq!(catalog.saved_shell_start_eligible(id), id == "shell");
        assert!(!id.contains("private-"));
    }
    assert!(!catalog.saved_shell_start_eligible("unknown"));
    fixture.assert_unchanged();

    let summary = fixture.record.environment_summary();
    state.spec.panels[0].command = None;
    fixture.record = fixture
        .store
        .replace_remote_workspace(&fixture.record, &state)
        .expect("change");
    assert!(
        catalog.saved_shell_start_eligible("shell"),
        "cached hint is not live authority"
    );
    assert!(!fixture.catalog().saved_shell_start_eligible("shell"));
    assert!(matches!(
        RemoteViewCatalog::load(&fixture.store, OWNER, &summary),
        Err(RemoteViewReopenError::StateChanged)
    ));
    assert!(matches!(
        RemoteViewCatalog::load(&fixture.store, FOREIGN, &fixture.record.environment_summary()),
        Err(RemoteViewReopenError::ClientSessionMismatch)
    ));
    fixture.assert_unchanged();
}

#[test]
fn catalog_presence_requires_both_saved_identity_and_execution_reference() {
    let fixture = Fixture::new();
    let catalog = fixture.catalog();
    assert!(!catalog.view_is_present(&Board::new(), "alpha"));
    for (owner, workspace, present) in [
        (None, "remote-workspace", false),
        (Some(FOREIGN), "remote-workspace", false),
        (Some(OWNER), "other-environment", false),
        (Some(OWNER), "remote-workspace", true),
    ] {
        let reference =
            owner.map(|owner| RemoteWorkspaceReference::new(owner.into(), workspace.into()).expect("reference"));
        let mut board = Board::from_runtime_state(&RuntimeState {
            workspaces: vec![crate::WorkspaceState {
                local_id: "visual".into(),
                remote_workspace: reference.clone(),
                panels: vec![crate::PanelState {
                    local_id: "alpha".into(),
                    kind: if reference.is_some() {
                        PanelKind::Ssh
                    } else {
                        PanelKind::Editor
                    },
                    remote_workspace: reference,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        })
        .expect("inert board");
        assert_eq!(catalog.view_is_present(&board, "alpha"), present);
        assert!(!catalog.view_is_present(&board, "unknown"));
        if present {
            let id = board.panels[0].id;
            let destination = board.create_workspace("Other placement");
            board.assign_panel_to_workspace(id, destination);
            assert!(catalog.view_is_present(&board, "alpha"));
        }
    }
    fixture.assert_unchanged();
}

#[test]
fn reopening_is_inert_copy_safe_and_persistable_after_all_views_close() {
    let fixture = Fixture::new();
    let mut board = Board::new();
    assert_eq!(fixture.catalog().panel_ids(), &["alpha", "bravo"]);
    let before = snapshot(&board);
    let prepared = fixture.prepare(&board, "alpha");
    assert_eq!(snapshot(&board), before);
    assert_eq!(
        board.adopt_reopened_remote_view(FOREIGN, prepared),
        Err(RemoteViewReopenError::ClientSessionMismatch)
    );
    assert_eq!(snapshot(&board), before);
    let prepared = fixture.prepare(&board, "alpha");
    let alpha = board
        .adopt_reopened_remote_view(OWNER, prepared)
        .expect("reopened alpha");
    let workspace = board.panels[0].workspace_id;
    let prepared = fixture.prepare(&board, "bravo");
    let bravo = board
        .adopt_reopened_remote_view(OWNER, prepared)
        .expect("reopened bravo");
    assert_eq!(board.workspaces.len(), 1);
    for panel in &board.panels {
        assert_eq!(panel.workspace_id, workspace);
        assert_eq!(panel.kind, PanelKind::Ssh);
        assert_eq!(panel.resume, PanelResume::Fresh);
        assert_eq!(panel.ssh_status(), Some(SshConnectionStatus::Disconnected));
        assert!(panel.terminal().expect("terminal").child_exited());
        assert!(panel.launch_command.is_none() && panel.launch_args.is_empty() && panel.launch_cwd.is_none());
        assert!(panel.ssh_connection.is_none() && panel.session_binding.is_none() && panel.template.is_none());
    }
    assert!(board.panel_mut(alpha).expect("alpha").restart().is_err());
    let saved = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
    saved.validate_remote_references().expect("persistable");
    assert!(!saved.to_yaml().expect("yaml").contains("private-"));
    let restored = Board::from_runtime_state(&saved).expect("inert restoration");
    assert_eq!(restored.panels[0].local_id, "alpha");
    assert_eq!(restored.panels[1].local_id, "bravo");
    board.close_panel(alpha);
    assert_eq!(board.panels.len(), 1);
    board.close_panel(bravo);
    assert!(board.panels.is_empty());
    let prepared = fixture.prepare(&board, "alpha");
    board
        .adopt_reopened_remote_view(OWNER, prepared)
        .expect("reopened after final close");
    assert_eq!(board.panels[0].workspace_id, workspace);
    fixture.assert_unchanged();
}

#[test]
fn admission_rejects_foreign_stale_missing_and_corrupt_records() {
    let fixture = Fixture::new();
    let summary = fixture.record.environment_summary();
    assert!(matches!(
        RemoteViewCatalog::load(&fixture.store, FOREIGN, &summary),
        Err(RemoteViewReopenError::ClientSessionMismatch)
    ));
    let mut foreign_claim = summary.clone();
    foreign_claim.owning_session_id = FOREIGN.into();
    assert!(RemoteViewCatalog::load(&fixture.store, FOREIGN, &foreign_claim).is_err());
    let board = Board::new();
    assert!(matches!(
        board.request_remote_view_reopen(FOREIGN, &fixture.catalog(), "alpha"),
        Err(RemoteViewReopenError::ClientSessionMismatch)
    ));
    assert!(matches!(
        board.request_remote_view_reopen(OWNER, &fixture.catalog(), "unknown"),
        Err(RemoteViewReopenError::MissingPanel)
    ));
    let request = board
        .request_remote_view_reopen(OWNER, &fixture.catalog(), "alpha")
        .expect("request");
    let mut changed = fixture.record.state().clone();
    changed.spec.working_directory = "another-directory".into();
    fixture
        .store
        .replace_remote_workspace(&fixture.record, &changed)
        .expect("change");
    assert!(matches!(
        request.prepare(&fixture.store),
        Err(RemoteViewReopenError::StateChanged)
    ));
    assert!(matches!(
        RemoteViewCatalog::load(&fixture.store, OWNER, &summary),
        Err(RemoteViewReopenError::StateChanged)
    ));
    let database = rusqlite::Connection::open(fixture.store.path()).expect("fixture connection");
    database
        .execute(
            "UPDATE remote_workspaces SET snapshot = ?1",
            [b"invalid-fixture".as_slice()],
        )
        .expect("corrupt fixture");
    assert!(matches!(
        RemoteViewCatalog::load(&fixture.store, OWNER, &summary),
        Err(RemoteViewReopenError::StorageUnavailable)
    ));
    database
        .execute("DELETE FROM remote_workspaces", [])
        .expect("remove fixture row");
    assert!(matches!(
        RemoteViewCatalog::load(&fixture.store, OWNER, &summary),
        Err(RemoteViewReopenError::StateChanged)
    ));
}

#[test]
fn adoption_reuses_the_exact_workspace_and_allows_visual_movement() {
    let fixture = Fixture::new();
    let mut board = Board::new();
    let workspace = matching_workspace(&mut board);
    let before = snapshot(&board);
    let prepared = fixture.prepare(&board, "alpha");
    assert_eq!(snapshot(&board), before);
    board.workspace_mut(workspace).expect("workspace").position = [700.0, 800.0];
    let id = board.adopt_reopened_remote_view(OWNER, prepared).expect("reopened");
    assert_eq!(board.panel(id).expect("panel").workspace_id, workspace);
    assert!(vec2_eq(
        board.workspace(workspace).expect("workspace").position,
        [700.0, 800.0]
    ));
    let before = snapshot(&board);
    assert!(matches!(
        board.request_remote_view_reopen(OWNER, &fixture.catalog(), "alpha"),
        Err(RemoteViewReopenError::ViewAlreadyPresent)
    ));
    assert_eq!(snapshot(&board), before);
    fixture.assert_unchanged();
}

#[test]
fn changed_existing_targets_and_new_ambiguity_reject_without_board_mutation() {
    let fixture = Fixture::new();
    for change in ["local-id", "reference", "ambiguity", "next-panel", "invalid-unrelated"] {
        let mut board = Board::new();
        let workspace = matching_workspace(&mut board);
        let prepared = fixture.prepare(&board, "alpha");
        match change {
            "local-id" => board.workspace_mut(workspace).expect("workspace").local_id = "replacement".into(),
            "reference" => board.workspace_mut(workspace).expect("workspace").remote_workspace = None,
            "ambiguity" => {
                matching_workspace(&mut board);
            }
            "next-panel" => board.next_panel_id += 1,
            "invalid-unrelated" => {
                let unrelated = board.create_workspace("Unrelated");
                board.workspace_mut(unrelated).expect("workspace").local_id.clear();
            }
            _ => unreachable!(),
        }
        let before = snapshot(&board);
        assert!(board.adopt_reopened_remote_view(OWNER, prepared).is_err(), "{change}");
        assert_eq!(snapshot(&board), before, "{change}");
    }
    fixture.assert_unchanged();
}

#[test]
fn changed_new_workspace_proposals_and_competing_reopens_are_inert() {
    let fixture = Fixture::new();
    for becomes_matching in [false, true] {
        let mut board = Board::new();
        let prepared = fixture.prepare(&board, "alpha");
        if becomes_matching {
            matching_workspace(&mut board);
        } else {
            let _ = board.create_workspace("Unrelated new workspace");
        }
        let before = snapshot(&board);
        assert_eq!(
            board.adopt_reopened_remote_view(OWNER, prepared),
            Err(RemoteViewReopenError::TargetChanged)
        );
        assert_eq!(snapshot(&board), before);
    }
    let mut board = Board::new();
    let first = fixture.prepare(&board, "alpha");
    let second = fixture.prepare(&board, "bravo");
    board.adopt_reopened_remote_view(OWNER, first).expect("first proposal");
    let before = snapshot(&board);
    assert_eq!(
        board.adopt_reopened_remote_view(OWNER, second),
        Err(RemoteViewReopenError::TargetChanged)
    );
    assert_eq!(snapshot(&board), before);
    fixture.assert_unchanged();
}

#[test]
fn the_first_remote_view_requires_valid_unrelated_persistence_identities() {
    let fixture = Fixture::new();
    for change in [
        "invalid-workspace",
        "duplicate-workspace",
        "duplicate-numeric",
        "exhausted-workspace",
        "exhausted-panel",
    ] {
        let mut board = Board::new();
        let first = board.create_workspace("Local one");
        let second = board.create_workspace("Local two");
        match change {
            "invalid-workspace" => board.workspace_mut(first).expect("workspace").local_id = "invalid/id".into(),
            "duplicate-workspace" => {
                let duplicate = board.workspace(first).expect("workspace").local_id.clone();
                board.workspace_mut(second).expect("workspace").local_id = duplicate;
            }
            "duplicate-numeric" => board.workspace_mut(second).expect("workspace").id = first,
            "exhausted-workspace" => board.next_workspace_id = u64::MAX,
            "exhausted-panel" => board.next_panel_id = u64::MAX,
            _ => unreachable!(),
        }
        let before = snapshot(&board);
        assert!(
            matches!(
                board.request_remote_view_reopen(OWNER, &fixture.catalog(), "alpha"),
                Err(RemoteViewReopenError::TargetChanged)
            ),
            "{change}"
        );
        assert_eq!(snapshot(&board), before, "{change}");
    }
    fixture.assert_unchanged();
}

#[test]
fn foreign_local_panel_identity_is_not_replaced_or_treated_as_ownership() {
    let fixture = Fixture::new();
    let mut board = Board::new();
    let workspace = board.create_workspace("Foreign copied reference");
    let reference = RemoteWorkspaceReference::new(FOREIGN.into(), "another-environment".into()).expect("reference");
    let id = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Ssh,
                local_id: Some("alpha".into()),
                remote_workspace: Some(reference),
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("foreign inert view");
    assert!(
        board
            .panel_mut(id)
            .expect("panel")
            .wait_for_shutdown(Duration::from_secs(2))
    );
    board.panel_mut(id).expect("panel").process_output();
    let before = snapshot(&board);
    assert!(matches!(
        board.request_remote_view_reopen(OWNER, &fixture.catalog(), "alpha"),
        Err(RemoteViewReopenError::ViewAlreadyPresent)
    ));
    assert_eq!(snapshot(&board), before);
    fixture.assert_unchanged();
}

#[test]
fn unrelated_panel_identity_and_membership_fail_closed() {
    let fixture = Fixture::new();
    for change in ["invalid-id", "duplicate-id", "orphan", "numeric-collision"] {
        let mut board = Board::new();
        let workspace = board.create_workspace("Local editor views");
        for local_id in ["local-one", "local-two"] {
            board
                .create_panel(
                    PanelOptions {
                        kind: PanelKind::Editor,
                        local_id: Some(local_id.into()),
                        ..PanelOptions::default()
                    },
                    workspace,
                )
                .expect("local editor");
        }
        match change {
            "invalid-id" => board.panels[0].local_id = "invalid/id".into(),
            "duplicate-id" => board.panels[1].local_id = board.panels[0].local_id.clone(),
            "orphan" => {
                board.workspace_mut(workspace).expect("workspace").panels.pop();
            }
            "numeric-collision" => board.next_panel_id = board.panels[0].id.0,
            _ => unreachable!(),
        }
        let before = snapshot(&board);
        assert!(
            matches!(
                board.request_remote_view_reopen(OWNER, &fixture.catalog(), "alpha"),
                Err(RemoteViewReopenError::TargetChanged)
            ),
            "{change}"
        );
        assert_eq!(snapshot(&board), before, "{change}");
    }
    fixture.assert_unchanged();
}
