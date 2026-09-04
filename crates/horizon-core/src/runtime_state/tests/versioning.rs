use super::*;
use crate::{HorizonHome, SessionStore, ssh::SshConnection};
use serde_json::{Value, json};

#[test]
fn supported_versions_preserve_local_ssh_and_agent_snapshots_during_migration() {
    let original = mixed_runtime_state();
    let expected = serde_json::to_value(&original).expect("snapshot value");
    let base_yaml = original.to_yaml().expect("tagged YAML snapshot");
    let version_header = format!("version: {RUNTIME_STATE_VERSION}\n");
    assert!(base_yaml.starts_with(&version_header));
    let directory = tempfile::tempdir().expect("isolated runtime directory");
    let path = directory.path().join("runtime.yaml");

    for version in std::iter::once(None).chain((0..=RUNTIME_STATE_VERSION).map(Some)) {
        let header = version.map_or_else(String::new, |version| format!("version: {version}\n"));
        let yaml = base_yaml.replacen(&version_header, &header, 1);
        std::fs::write(&path, &yaml).expect("write fixture");

        let loaded = RuntimeState::load(&path).expect("supported version").expect("state");

        assert_eq!(serde_json::to_value(&loaded).expect("migrated value"), expected);
        assert_eq!(std::fs::read_to_string(&path).expect("unchanged fixture"), yaml);
    }
}

#[test]
fn future_versions_are_rejected_by_read_and_write_serializers() {
    for version in [RUNTIME_STATE_VERSION + 1, u32::MAX] {
        let value = future_snapshot(version);
        let yaml = serde_yaml::to_string(&value).expect("future fixture");
        let error = serde_yaml::from_str::<RuntimeState>(&yaml).expect_err("newer YAML version");
        assert!(error.to_string().contains("unsupported runtime state version"));
        assert!(serde_json::from_value::<RuntimeState>(value).is_err());

        let state = RuntimeState {
            version,
            ..RuntimeState::default()
        };
        assert!(state.to_yaml().is_err());
        assert!(serde_yaml::to_string(&state).is_err());
        assert!(serde_json::to_vec(&state).is_err());
    }
}

#[test]
fn malformed_or_duplicate_versions_cannot_fall_back_to_current() {
    for yaml in [
        "version: -1\n",
        "version: null\n",
        "version: future\n",
        "version: 1.5\n",
        "version: 4294967296\n",
        "version: 2\nversion: 3\n",
        "version: 3\nversion: 2\n",
    ] {
        assert!(serde_yaml::from_str::<RuntimeState>(yaml).is_err(), "{yaml}");
    }
}

#[test]
fn rejecting_a_future_session_preserves_runtime_metadata_index_and_transcripts() {
    let directory = tempfile::tempdir().expect("isolated session directory");
    let home = HorizonHome::from_root(directory.path().to_path_buf());
    let store = SessionStore::new(home.clone(), home.config_path());
    let session = store.create_new_session(&Config::default()).expect("create fixture");
    let yaml = serde_yaml::to_string(&future_snapshot(RUNTIME_STATE_VERSION + 1)).expect("future fixture");
    std::fs::write(&session.runtime_state_path, yaml).expect("write future runtime");
    let transcript = session.transcript_root.join("saved-panel.bin");
    std::fs::write(&transcript, b"retained terminal history").expect("write transcript");
    let paths = [
        session.runtime_state_path.clone(),
        home.session_meta_path(&session.session_id),
        home.session_index_path(),
        transcript,
    ];
    let original_bytes = paths.map(|path| {
        let bytes = std::fs::read(&path).expect("fixture bytes");
        (path, bytes)
    });

    assert!(RuntimeState::load(&session.runtime_state_path).is_err());
    assert!(store.prepare_startup(&Config::default()).is_err());
    assert!(store.resume_session(&session.session_id).is_err());
    assert!(store.take_over_session(&session.session_id).is_err());
    assert!(store.duplicate_session(&session.session_id).is_err());
    assert!(store.delete_session(&session.session_id).is_err());

    for (path, bytes) in original_bytes {
        assert_eq!(std::fs::read(path).expect("retained fixture"), bytes);
    }
    assert_eq!(store.list_profile_sessions().expect("unchanged index").len(), 1);
}

#[test]
fn saving_an_unsupported_snapshot_preserves_the_existing_session() {
    let directory = tempfile::tempdir().expect("isolated session directory");
    let home = HorizonHome::from_root(directory.path().to_path_buf());
    let store = SessionStore::new(home.clone(), home.config_path());
    let session = store.create_new_session(&Config::default()).expect("create fixture");
    let paths = [
        session.runtime_state_path.clone(),
        home.session_meta_path(&session.session_id),
        home.session_index_path(),
    ];
    let original_bytes = paths.map(|path| {
        let bytes = std::fs::read(&path).expect("fixture bytes");
        (path, bytes)
    });
    let invalid = RuntimeState {
        version: RUNTIME_STATE_VERSION + 1,
        ..session.runtime_state
    };

    assert!(store.save_runtime_state(&session.session_id, &invalid).is_err());

    for (path, bytes) in original_bytes {
        assert_eq!(std::fs::read(path).expect("retained fixture"), bytes);
    }
}

fn future_snapshot(version: u32) -> Value {
    json!({
        "version": version,
        "workspaces": [],
        "future_remote_state": {"workspace": "saved-remote", "checkpoint": "retained-work"},
    })
}

fn mixed_runtime_state() -> RuntimeState {
    RuntimeState {
        window: Some(WindowConfig {
            width: 1440.0,
            height: 900.0,
            x: Some(50.0),
            y: Some(75.0),
        }),
        canvas_view: Some(CanvasViewState::new([24.0, -12.0], 1.5)),
        active_workspace_local_id: Some("workspace".to_string()),
        focused_panel_local_id: Some("ssh-panel".to_string()),
        detached_workspaces: vec![DetachedWorkspaceState {
            workspace_local_id: "workspace".to_string(),
            window: WindowConfig::default(),
        }],
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "Retained work".to_string(),
            cwd: Some("/synthetic/repository".to_string()),
            position: Some([10.0, 20.0]),
            layout: Some(WorkspaceLayout::Grid),
            panels: vec![
                PanelState {
                    local_id: "local-panel".to_string(),
                    command: Some("synthetic-shell".to_string()),
                    args: vec!["--isolated".to_string()],
                    cwd: Some("/synthetic/repository/nested".to_string()),
                    ..PanelState::default()
                },
                PanelState {
                    local_id: "ssh-panel".to_string(),
                    kind: PanelKind::Ssh,
                    ssh_connection: Some(SshConnection {
                        host: "worker.example.invalid".to_string(),
                        user: Some("developer".to_string()),
                        port: Some(2222),
                        remote_command: Some("tmux attach -t saved".to_string()),
                        ..SshConnection::default()
                    }),
                    ..PanelState::default()
                },
                PanelState {
                    local_id: "agent-panel".to_string(),
                    kind: PanelKind::Pi,
                    resume: PanelResume::Session {
                        session_id: "saved-session".to_string(),
                    },
                    session_binding: Some(AgentSessionBinding::new(
                        PanelKind::Pi,
                        "saved-session".to_string(),
                        Some("/synthetic/repository".to_string()),
                        Some("Continue saved task".to_string()),
                        Some(42),
                    )),
                    ..PanelState::default()
                },
            ],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    }
}
