use super::*;
use crate::cloud_run::{CloudProvider, CloudWorkflowStore, GitCommitSha, GitSource, WorkerLifetime, WorkerTarget};
use crate::remote_workspace::{RemotePanelBinding, RemoteWorkspaceSpec, RemoteWorkspaceState};
use crate::{HorizonHome, Panel, PanelId, SessionStore, WorkspaceId};
use serde_json::{Value, json};
use std::time::Duration;

const OWNER: &str = "11111111-1111-4111-8111-111111111111";

fn reference(owner: &str) -> RemoteWorkspaceReference {
    RemoteWorkspaceReference::new(owner.into(), "remote-environment".into()).expect("reference")
}

fn snapshot(owner: &str) -> RuntimeState {
    RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "remote-view".into(),
            name: "Remote workspace".into(),
            remote_workspace: Some(reference(owner)),
            panels: vec![PanelState {
                local_id: "remote-panel".into(),
                name: "Remote task".into(),
                kind: PanelKind::Ssh,
                remote_workspace: Some(reference(owner)),
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    }
}

fn marker_command(path: &Path) -> (String, Vec<String>) {
    if cfg!(windows) {
        (
            "cmd.exe".into(),
            vec![
                "/D".into(),
                "/C".into(),
                format!("echo unexpected>\"{}\"", path.display()),
            ],
        )
    } else {
        (
            "/bin/sh".into(),
            vec![
                "-c".into(),
                ": > \"$1\"".into(),
                "marker".into(),
                path.display().to_string(),
            ],
        )
    }
}

#[test]
fn reference_serialization_rejects_ambiguous_or_malformed_identity() {
    let original = reference(OWNER);
    let value = serde_json::to_value(&original).expect("reference value");
    assert_eq!(
        value,
        json!({"owner_session_id": OWNER, "workspace_local_id": "remote-environment"})
    );
    assert_eq!(
        serde_json::from_value::<RemoteWorkspaceReference>(value.clone()).expect("roundtrip"),
        original
    );
    for invalid in [
        Value::Null,
        json!({}),
        json!({"owner_session_id": OWNER, "workspace_local_id": "../escape"}),
        json!({"owner_session_id": "00000000-0000-0000-0000-000000000000", "workspace_local_id": "remote"}),
        json!({"owner_session_id": "11111111111141118111111111111111", "workspace_local_id": "remote"}),
        json!({"owner_session_id": "not-a-session", "workspace_local_id": "remote"}),
        json!({"owner_session_id": OWNER, "workspace_local_id": "remote", "grant": true}),
    ] {
        assert!(serde_json::from_value::<RemoteWorkspaceReference>(invalid).is_err());
    }
    let duplicate = format!("owner_session_id: {OWNER}\nowner_session_id: {OWNER}\nworkspace_local_id: remote\n");
    assert!(serde_yaml::from_str::<RemoteWorkspaceReference>(&duplicate).is_err());
    for field_path in [
        "/workspaces/0/remote_workspace",
        "/workspaces/0/panels/0/remote_workspace",
    ] {
        let mut state = serde_json::to_value(snapshot(OWNER)).expect("state value");
        *state.pointer_mut(field_path).expect("reference field") = Value::Null;
        assert!(serde_json::from_value::<RuntimeState>(state).is_err());
    }
}

#[test]
fn remote_snapshots_require_explicit_current_version_and_unrepaired_ids() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("runtime.yaml");
    let original = snapshot(OWNER).to_yaml().expect("snapshot");
    for version in [None, Some(0), Some(1), Some(2)] {
        let header = version.map_or_else(String::new, |value| format!("version: {value}\n"));
        let yaml = original.replacen("version: 3\n", &header, 1);
        std::fs::write(&path, &yaml).expect("fixture");
        assert!(RuntimeState::load(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("retained"), yaml);
    }
    for invalid in invalid_snapshots() {
        let yaml = serde_yaml::to_string(&invalid).expect("invalid fixture");
        let mut unmodified = invalid.clone();
        unmodified.ensure_local_ids();
        assert_eq!(serde_yaml::to_string(&unmodified).expect("identity retained"), yaml);
        assert!(invalid.to_yaml().is_err());
        assert!(Board::from_runtime_state(&invalid).is_err());
        std::fs::write(&path, &yaml).expect("fixture");
        assert!(RuntimeState::load(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("retained"), yaml);
    }
}

fn invalid_snapshots() -> Vec<RuntimeState> {
    let mut missing_workspace = snapshot(OWNER);
    missing_workspace.workspaces[0].local_id.clear();
    let mut missing_panel = snapshot(OWNER);
    missing_panel.workspaces[0].panels[0].local_id.clear();
    let mut duplicate_workspace = snapshot(OWNER);
    duplicate_workspace
        .workspaces
        .push(duplicate_workspace.workspaces[0].clone());
    let mut duplicate_panel = snapshot(OWNER);
    let repeated = duplicate_panel.workspaces[0].panels[0].clone();
    duplicate_panel.workspaces[0].panels.push(repeated);
    let mut missing_reference = snapshot(OWNER);
    missing_reference.workspaces[0].panels[0].remote_workspace = None;
    let mut wrong_reference = snapshot(OWNER);
    wrong_reference.workspaces[0].panels[0].remote_workspace = Some(reference("22222222-2222-4222-8222-222222222222"));
    vec![
        missing_workspace,
        missing_panel,
        duplicate_workspace,
        duplicate_panel,
        missing_reference,
        wrong_reference,
    ]
}

#[test]
fn remote_views_never_execute_saved_commands_on_restore_or_restart() {
    let directory = tempfile::tempdir().expect("directory");
    let marker = directory.path().join("must-not-run");
    let (program, args) = marker_command(&marker);
    let mut state = snapshot(OWNER);
    state.workspaces[0].panels[0].command = Some(program.clone());
    state.workspaces[0].panels[0].args.clone_from(&args);
    for transcript_root in [None, Some(directory.path())] {
        if transcript_root.is_some() {
            std::fs::write(directory.path().join("remote-panel.bin"), b"Retained remote output\r\n")
                .expect("transcript");
        }
        let mut board = Board::from_runtime_state_with_transcripts(&state, transcript_root).expect("deferred board");
        let panel = &mut board.panels[0];
        assert!(panel.wait_for_shutdown(Duration::from_secs(2)));
        assert!(!marker.exists());
        let content = panel.terminal().expect("snapshot terminal").last_lines_text(24);
        assert!(content.contains("Remote connection pending"));
        assert_eq!(content.contains("Retained remote output"), transcript_root.is_some());
        let identity = panel.id;
        assert!(
            panel
                .restart()
                .expect_err("remote restart blocked")
                .to_string()
                .contains("locally")
        );
        assert!(board.restart_panel(identity).is_err());
        let saved = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
        assert_eq!(
            saved.workspaces[0].remote_workspace,
            state.workspaces[0].remote_workspace
        );
        assert_eq!(
            saved.workspaces[0].panels[0].remote_workspace,
            state.workspaces[0].panels[0].remote_workspace
        );
        assert_eq!(saved.workspaces[0].panels[0].command.as_deref(), Some(program.as_str()));
        assert_eq!(saved.workspaces[0].panels[0].args, args);
        assert!(!marker.exists());
    }
    let mut control = Panel::spawn(
        PanelId(9),
        WorkspaceId(9),
        PanelOptions {
            kind: PanelKind::Ssh,
            command: Some(program),
            args,
            ..PanelOptions::default()
        },
    )
    .expect("ordinary SSH command positive control");
    assert!(control.wait_for_shutdown(Duration::from_secs(2)));
    assert!(marker.exists(), "the same unguarded saved command really executes");
}

#[test]
fn remote_views_reject_local_content_factories_and_agent_bindings() {
    for kind in [
        PanelKind::Shell,
        PanelKind::Command,
        PanelKind::Claude,
        PanelKind::Codex,
        PanelKind::Pi,
        PanelKind::OpenCode,
        PanelKind::Gemini,
        PanelKind::KiloCode,
        PanelKind::Grok,
        PanelKind::Editor,
        PanelKind::GitChanges,
        PanelKind::Usage,
        PanelKind::Browser,
    ] {
        let mut state = snapshot(OWNER);
        state.workspaces[0].panels[0].kind = kind;
        assert!(state.to_yaml().is_err());
        assert!(Board::from_runtime_state(&state).is_err());
        let options = state.workspaces[0].panels[0].to_panel_options(&state.browser);
        assert!(Panel::spawn(PanelId(1), WorkspaceId(1), options).is_err());
    }
    let mut state = snapshot(OWNER);
    assert!(!state.needs_agent_binding_bootstrap());
    assert!(!state.normalize_agent_bindings(&HashSet::new()));
    state.workspaces[0].panels[0].resume = PanelResume::Last;
    assert!(state.to_yaml().is_err());
    state.workspaces[0].panels[0].resume = PanelResume::Fresh;
    state.workspaces[0].panels[0].session_binding = Some(AgentSessionBinding::new(
        PanelKind::Pi,
        "remote-task".into(),
        None,
        None,
        None,
    ));
    assert!(state.to_yaml().is_err());
}

#[test]
fn moving_or_closing_views_preserves_remote_identity_and_empty_workspaces() {
    let mut board = Board::from_runtime_state(&snapshot(OWNER)).expect("board");
    let remote_id = board.workspaces[0].id;
    let panel_id = board.panels[0].id;
    let local_id = board.create_workspace("Local");
    board.assign_panel_to_workspace(panel_id, local_id);
    assert_eq!(
        board.panel(panel_id).expect("moved").remote_workspace(),
        Some(&reference(OWNER))
    );
    board.remove_empty_workspaces();
    assert!(board.workspace(remote_id).is_some());
    let detached = vec![DetachedWorkspaceState {
        workspace_local_id: "remote-view".into(),
        window: WindowConfig::default(),
    }];
    let saved = RuntimeState::from_board_with_detached_workspaces(
        &board,
        WindowConfig::default(),
        CanvasViewState::default(),
        detached,
    );
    assert!(saved.to_yaml().is_ok());
    assert_eq!(saved.detached_workspaces[0].workspace_local_id, "remote-view");
    let mut restored = Board::from_runtime_state(&saved).expect("moved reference restored");
    assert!(restored.panels[0].restart().is_err());
    board.assign_panel_to_workspace(panel_id, remote_id);
    board.close_panel(panel_id);
    board.remove_empty_workspaces();
    assert!(board.panels.is_empty());
    assert_eq!(
        board
            .workspace(remote_id)
            .expect("empty remote view retained")
            .remote_workspace,
        Some(reference(OWNER))
    );
}

#[test]
fn incompatible_workspace_moves_and_removal_cannot_reinterpret_execution() {
    let mut state = snapshot(OWNER);
    let mut other = snapshot("22222222-2222-4222-8222-222222222222").workspaces.remove(0);
    other.local_id = "other-view".into();
    other.panels.clear();
    state.workspaces.push(other);
    let mut board = Board::from_runtime_state(&state).expect("board");
    let source = board.workspaces[0].id;
    let target = board.workspaces[1].id;
    let panel = board.panels[0].id;
    board.assign_panel_to_workspace(panel, target);
    assert_eq!(board.panel(panel).expect("panel").workspace_id, source);
    board.remove_workspace(source);
    assert!(board.workspace(source).is_some());
    assert!(board.create_panel(PanelOptions::default(), source).is_err());
    let local = board.create_workspace("Local destination");
    board.remove_workspace(source);
    assert!(board.workspace(source).is_none());
    assert_eq!(board.panel(panel).expect("relocated").workspace_id, local);
    assert_eq!(
        board.panel(panel).expect("relocated").remote_workspace(),
        Some(&reference(OWNER))
    );
    assert!(board.restart_panel(panel).is_err());
}

#[test]
fn copied_and_deleted_client_sessions_do_not_adopt_or_remove_remote_aggregates() {
    let directory = tempfile::tempdir().expect("directory");
    let home = HorizonHome::from_root(directory.path().to_path_buf());
    let sessions = SessionStore::new(home.clone(), home.config_path());
    let source = sessions
        .create_session_from_runtime(RuntimeState::default())
        .expect("source session");
    let state = snapshot(&source.session_id);
    sessions
        .save_runtime_state(&source.session_id, &state)
        .expect("save references");
    let cloud = CloudWorkflowStore::open(&home).expect("private cloud store");
    let original = cloud
        .create_remote_workspace(&source.session_id, &aggregate())
        .expect("owned aggregate");
    let bytes = std::fs::read(&source.runtime_state_path).expect("source bytes");
    std::fs::write(source.transcript_root.join("remote-panel.bin"), b"retained output").expect("transcript");
    let copy = sessions.duplicate_session(&source.session_id).expect("copy");
    assert_ne!(copy.session_id, source.session_id);
    assert_eq!(
        copy.runtime_state.workspaces[0].remote_workspace,
        Some(reference(&source.session_id))
    );
    assert!(
        cloud
            .load_remote_workspace(&copy.session_id, "remote-environment")
            .is_err()
    );
    let mut board = Board::from_runtime_state(&copy.runtime_state).expect("inert foreign view");
    assert!(board.panels[0].restart().is_err());
    board.shutdown_terminal_panels();
    assert_eq!(
        std::fs::read(&source.runtime_state_path).expect("unchanged source"),
        bytes
    );
    assert_eq!(
        std::fs::read(copy.transcript_root.join("remote-panel.bin")).expect("copied output"),
        b"retained output"
    );
    sessions.delete_session(&copy.session_id).expect("delete copied client");
    sessions
        .delete_session(&source.session_id)
        .expect("delete original client");
    drop(cloud);
    let reopened = CloudWorkflowStore::open(&home).expect("reopen inventory");
    assert_eq!(
        reopened
            .list_remote_workspaces(&source.session_id)
            .expect("retained inventory"),
        vec![original]
    );
}

fn aggregate() -> RemoteWorkspaceState {
    RemoteWorkspaceState::new(RemoteWorkspaceSpec {
        workspace_local_id: "remote-environment".into(),
        target: WorkerTarget {
            provider: CloudProvider::LocalDocker,
            profile: "synthetic".into(),
            image: format!("registry.example/worker@sha256:{}", "a".repeat(64)),
            disk_gib: 20,
            lifetime: WorkerLifetime::Persistent,
            max_hourly_cost_micros: None,
        },
        repository: GitSource {
            repository: "owner/project".into(),
            commit: GitCommitSha::parse("b".repeat(40)).expect("commit"),
            branch: None,
        },
        working_directory: ".".into(),
        generation: 0,
        panels: vec![RemotePanelBinding {
            panel_local_id: "remote-panel".into(),
            kind: PanelKind::Pi,
            command: None,
            working_directory: Some("src".into()),
            task_handoff: Some("Continue synthetic task".into()),
            agent_session_id: Some("remote-native-session".into()),
        }],
    })
    .expect("aggregate")
}

#[test]
fn rejected_remote_session_operations_preserve_saved_files() {
    let directory = tempfile::tempdir().expect("directory");
    let home = HorizonHome::from_root(directory.path().to_path_buf());
    let store = SessionStore::new(home.clone(), home.config_path());
    let session = store.create_session_from_runtime(snapshot(OWNER)).expect("session");
    let metadata = std::fs::read(home.session_meta_path(&session.session_id)).expect("metadata");
    let index = std::fs::read(home.session_index_path()).expect("index");
    let original = std::fs::read(&session.runtime_state_path).expect("runtime");
    for invalid in invalid_snapshots() {
        assert!(store.save_runtime_state(&session.session_id, &invalid).is_err());
        assert_eq!(
            std::fs::read(&session.runtime_state_path).expect("saved runtime"),
            original
        );
        assert!(store.create_session_from_runtime(invalid.clone()).is_err());
        let yaml = serde_yaml::to_string(&invalid).expect("invalid fixture");
        std::fs::write(&session.runtime_state_path, &yaml).expect("write corrupt fixture");
        assert!(store.resume_session(&session.session_id).is_err());
        assert!(store.duplicate_session(&session.session_id).is_err());
        assert!(store.delete_session(&session.session_id).is_err());
        assert_eq!(
            std::fs::read_to_string(&session.runtime_state_path).expect("retained fixture"),
            yaml
        );
        assert_eq!(
            std::fs::read(home.session_meta_path(&session.session_id)).expect("retained metadata"),
            metadata
        );
        assert_eq!(std::fs::read(home.session_index_path()).expect("retained index"), index);
        std::fs::write(&session.runtime_state_path, &original).expect("restore test fixture");
    }
}

#[test]
fn new_remote_ssh_views_inherit_workspace_identity_without_starting_tasks() {
    let directory = tempfile::tempdir().expect("directory");
    let marker = directory.path().join("must-not-start");
    let (program, args) = marker_command(&marker);
    let mut state = snapshot(OWNER);
    state.workspaces[0].panels.clear();
    let mut board = Board::from_runtime_state(&state).expect("empty remote board");
    let workspace = board.workspaces[0].id;
    let panel_id = board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Ssh,
                command: Some(program),
                args,
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("new deferred view");
    let panel = board.panel_mut(panel_id).expect("panel");
    assert!(panel.wait_for_shutdown(Duration::from_secs(2)));
    assert_eq!(panel.remote_workspace(), Some(&reference(OWNER)));
    assert!(!marker.exists());
    assert!(panel.restart().is_err());
    let saved = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
    assert!(saved.to_yaml().is_ok());
    assert_eq!(saved.workspaces[0].panels[0].remote_workspace, Some(reference(OWNER)));
}
