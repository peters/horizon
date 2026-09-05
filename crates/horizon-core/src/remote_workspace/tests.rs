use super::*;
use crate::cloud_run::{
    CloudProvider, WorkerLifetime,
    interactive_worker::{InteractiveWorkerIdentity, InteractiveWorkerLease, InteractiveWorkerLifetime},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;

fn panel(local_id: &str) -> RemotePanelBinding {
    RemotePanelBinding {
        panel_local_id: local_id.into(),
        kind: PanelKind::Shell,
        command: None,
        working_directory: None,
        task_handoff: None,
        agent_session_id: None,
    }
}

fn spec() -> RemoteWorkspaceSpec {
    RemoteWorkspaceSpec {
        workspace_local_id: "workspace-one".into(),
        target: WorkerTarget {
            provider: CloudProvider::RunPod,
            profile: "gpu-development".into(),
            image: format!("registry.example/worker@sha256:{}", "a".repeat(64)),
            disk_gib: 20,
            lifetime: WorkerLifetime::TimeLimited { seconds: 900 },
            max_hourly_cost_micros: Some(100_000),
        },
        repository: GitSource {
            repository: "owner/project".into(),
            commit: GitCommitSha::parse("b".repeat(40)).expect("commit"),
            branch: None,
        },
        working_directory: "crates/core".into(),
        generation: 2,
        panels: vec![panel("panel-one"), panel("panel-two")],
    }
}

fn key() -> String {
    let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
    blob.extend([7; 32]);
    format!("ssh-ed25519 {}", STANDARD.encode(blob))
}

fn active() -> RemoteWorkspaceState {
    let spec = spec();
    let workflow_id = CloudWorkflowId::new();
    let job_id = CloudJobId::new();
    let worker = InteractiveWorker {
        identity: InteractiveWorkerIdentity {
            provider: spec.target.provider,
            workflow_id,
            job_id,
            resource_id: "exact-resource".into(),
        },
        target: spec.target.clone(),
        ssh_public_key: key(),
        lifetime: InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
            terminate_after: "2020-01-01T00:00:00Z".into(),
        }),
    };
    RemoteWorkspaceState {
        version: REMOTE_WORKSPACE_STATE_VERSION,
        runtime: Some(RemoteRuntimeGeneration {
            workspace_local_id: spec.workspace_local_id.clone(),
            generation: spec.generation,
            workflow_id,
            job_id,
            phase: RemoteRuntimePhase::Ready,
            worker: Some(worker),
            ssh: Some(InteractiveWorkerSshEndpoint {
                host: "127.0.0.1".into(),
                port: 2222,
                username: "root".into(),
                host_key: key(),
            }),
            cleanup: None,
        }),
        checkpoint: Some(RepositoryCheckpoint {
            workspace_local_id: spec.workspace_local_id.clone(),
            base_commit: spec.repository.commit.clone(),
            manifest_digest: ArtifactDigest::parse_sha256("c".repeat(64)).expect("digest"),
            runtime_generation: 1,
            generation: 12,
            captured_at_millis: 1_000,
            recovery_artifact: Some(ArtifactDigest::parse_sha256("d".repeat(64)).expect("recovery digest")),
        }),
        spec,
    }
}

#[test]
fn round_trip_preserves_durable_work_after_runtime_disposal() {
    let mut state = active();
    state.spec.panels[0].kind = PanelKind::Claude;
    state.spec.panels[0].agent_session_id = Some("session-one".into());
    state.spec.panels[0].task_handoff = Some("Continue the saved task\nwith its final checkpoint".into());
    for runtime_present in [true, false] {
        if !runtime_present {
            state.runtime = None;
        }
        let encoded = serde_json::to_string(&state).expect("encode");
        assert_eq!(
            serde_json::from_str::<RemoteWorkspaceState>(&encoded).expect("decode"),
            state
        );
        assert_eq!(state.checkpoint.as_ref().expect("checkpoint").generation, 12);
    }
    let dormant = RemoteWorkspaceState::new(spec()).expect("dormant specification");
    assert!(dormant.runtime.is_none());
}

#[test]
fn decoding_rejects_schema_ownership_and_generation_drift() {
    let valid = serde_json::to_value(active()).expect("snapshot");
    for (pointer, replacement) in [
        ("/version", json!(2)),
        ("/runtime/workspace_local_id", json!("another-workspace")),
        ("/runtime/generation", json!(3)),
        ("/runtime/worker/identity/workflow_id", json!(CloudWorkflowId::new())),
        ("/runtime/worker/identity/job_id", json!(CloudJobId::new())),
        ("/runtime/worker/target/disk_gib", json!(200)),
        ("/runtime/ssh/host_key", json!("unverified-key")),
        ("/checkpoint/workspace_local_id", json!("another-workspace")),
        ("/checkpoint/runtime_generation", json!(3)),
        ("/checkpoint/generation", json!(0)),
        ("/checkpoint/base_commit", json!("e".repeat(40))),
        ("/checkpoint/captured_at_millis", json!(-1)),
    ] {
        let mut altered = valid.clone();
        *altered.pointer_mut(pointer).expect("field") = replacement;
        assert!(
            serde_json::from_value::<RemoteWorkspaceState>(altered).is_err(),
            "{pointer}"
        );
    }
    let mut unknown = valid;
    unknown["credentials"] = json!("test-token");
    assert!(serde_json::from_value::<RemoteWorkspaceState>(unknown).is_err());
}

#[test]
fn panel_identities_cannot_alias_tmux_sessions_or_use_non_terminal_kinds() {
    let mut spec = spec();
    let names: Vec<_> = spec
        .panels
        .iter()
        .map(|panel| panel.tmux_session_name().expect("name"))
        .collect();
    assert_ne!(names[0], names[1]);
    assert_eq!(names[0], "horizon-panel-panel-one");
    spec.panels[1].panel_local_id = spec.panels[0].panel_local_id.clone();
    assert_eq!(spec.validate(), Err(RemoteWorkspaceError::DuplicatePanel));
    for bad_id in ["", "panel:0", "panel.0", "panel/one", "panel;two", "panel\nthree"] {
        assert!(panel(bad_id).tmux_session_name().is_err());
    }
    for kind in [
        PanelKind::Ssh,
        PanelKind::Browser,
        PanelKind::Editor,
        PanelKind::GitChanges,
        PanelKind::Usage,
    ] {
        let mut panel = panel("valid");
        panel.kind = kind;
        assert_eq!(panel.validate(), Err(RemoteWorkspaceError::InvalidPanel("kind")));
    }
}

#[test]
fn source_paths_and_launch_data_are_validated_without_echoing_payloads() {
    for path in [
        "/absolute",
        "../escape",
        "nested/../escape",
        "nested//dir",
        "./nested",
        "C:/repo",
        "nested\\dir",
        "nested\0dir",
    ] {
        let mut spec = spec();
        spec.working_directory = path.into();
        assert!(spec.validate().is_err(), "{path:?}");
    }
    let mut spec = spec();
    spec.repository.repository = "https://user:test-token@example.invalid/repo".into();
    assert_eq!(spec.validate(), Err(RemoteWorkspaceError::InvalidSpec("repository")));
    let mut panel = panel("command");
    panel.kind = PanelKind::Command;
    assert!(panel.validate().is_err());
    panel.command = Some(RemotePanelCommand {
        program: "printf".into(),
        args: vec!["literal ; $(text)".into(), "line one\nline two".into()],
    });
    assert_eq!(panel.validate(), Ok(()));
    panel.agent_session_id = Some("session".into());
    assert_eq!(
        panel.validate(),
        Err(RemoteWorkspaceError::InvalidPanel("agent session"))
    );
    panel.agent_session_id = None;
    panel.command.as_mut().expect("command").args = vec!["x".repeat(40_000); 2];
    assert_eq!(panel.validate(), Err(RemoteWorkspaceError::InvalidPanel("command")));
}

#[test]
fn partial_creation_and_expired_handles_remain_recoverable_with_explicit_cleanup() {
    let mut state = active();
    assert_eq!(state.validate(), Ok(()), "expired handles remain loadable for cleanup");
    let runtime = state.runtime.as_mut().expect("runtime");
    runtime.phase = RemoteRuntimePhase::Deleting;
    assert!(runtime.validate_for(&state.spec).is_err());
    runtime.cleanup = Some(RemoteCleanupIntent {
        reason: RemoteCleanupReason::ApplicationExit,
        requested_at_millis: 2_000,
    });
    assert_eq!(runtime.validate_for(&state.spec), Ok(()));
    runtime.worker = None;
    runtime.ssh = None;
    assert!(runtime.validate_for(&state.spec).is_err());
    runtime.phase = RemoteRuntimePhase::Reconciling;
    assert_eq!(runtime.validate_for(&state.spec), Ok(()));
    runtime.phase = RemoteRuntimePhase::Ready;
    assert!(runtime.validate_for(&state.spec).is_err());
}
