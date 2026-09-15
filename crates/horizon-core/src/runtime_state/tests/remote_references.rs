use super::*;
use crate::cloud_run::{CloudProvider, CloudWorkflowStore, GitCommitSha, GitSource, WorkerLifetime, WorkerTarget};
use crate::remote_workspace::{RemotePanelBinding, RemoteWorkspaceSpec, RemoteWorkspaceState};
use crate::{HorizonHome, SessionStore};
use serde_json::{Value, json};

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
    let mut board = Board::from_runtime_state(&copy.runtime_state).expect("foreign view");
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
