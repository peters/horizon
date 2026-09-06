use super::*;
use crate::remote_workspace::{
    RemoteCleanupIntent, RemoteCleanupReason, RemotePanelCommand, RemoteRuntimePhase, RemoteWorkspaceSpec,
};

fn command() -> RemotePanelCommand {
    RemotePanelCommand {
        program: "example-program".into(),
        args: ["", "$(private);", "æøå", "line\nbreak", "'quote'", "\\", "#{l:..}"]
            .map(String::from)
            .to_vec(),
    }
}

fn set_intent(fixture: &mut Fixture, edit: impl FnOnce(&mut RemoteWorkspaceSpec)) {
    let current = fixture.current();
    let mut next = current.workspace().state().clone();
    edit(&mut next.spec);
    fixture
        .store
        .replace_remote_workspace(current.workspace(), &next)
        .expect("edit intent");
    let observation = fixture.recovered.observation().expect("observation").clone();
    let identities = RemoteSshIdentityStore::new(&HorizonHome::from_root(fixture.directory.path().join("home")));
    fixture.recovered =
        recover_remote_workspace(&fixture.store, &identities, &Provider(observation), OWNER, "workspace")
            .expect("recover current intent");
}

#[test]
fn saved_intent_preserves_literal_arguments_and_repository_relative_directory() {
    for override_directory in [None, Some("nested space")] {
        let mut fixture = Fixture::new();
        set_intent(&mut fixture, |spec| {
            spec.working_directory = "workspace directory".into();
            spec.panels[0].command = Some(command());
            spec.panels[0].working_directory = override_directory.map(String::from);
        });
        let before = fixture.current();
        let key = std::fs::read(fixture.recovered.identity().private_key_path()).expect("key");
        let result = inspect_with(
            &fixture.store,
            &fixture.recovered,
            "terminal",
            Inspection::SavedIntent,
            |_, _, input| {
                let mut argv = vec![command().program];
                argv.extend(command().args);
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(input).expect("JSON"),
                    serde_json::json!({
                        "version": 1, "operation": "verify", "runtime": before.worker_request().expect("request").job_id,
                        "panel": "terminal", "directory": override_directory.unwrap_or("workspace directory"), "argv": argv
                    })
                );
                Ok(RUNNING.to_vec())
            },
        );
        assert_eq!(result, Ok(RemotePanelStatus::Running { pid: 123 }));
        assert_eq!(fixture.current(), before);
        assert_eq!(
            std::fs::read(fixture.recovered.identity().private_key_path()).expect("key"),
            key
        );
    }
}

#[test]
fn incomplete_or_unresolved_agent_intent_never_reaches_ssh() {
    let mut fixture = Fixture::new();
    assert_eq!(
        inspect_with(
            &fixture.store,
            &fixture.recovered,
            "terminal",
            Inspection::SavedIntent,
            |_, _, _| panic!("no SSH")
        ),
        Err(RemotePanelStatusError::UnsupportedIntent)
    );
    for (kind, handoff, resume) in [
        (PanelKind::Claude, None, None),
        (PanelKind::Command, Some("private task context"), None),
        (PanelKind::Claude, None, Some("retained-agent-session")),
    ] {
        set_intent(&mut fixture, |spec| {
            spec.panels[0].command = Some(command());
            spec.panels[0].kind = kind;
            spec.panels[0].task_handoff = handoff.map(String::from);
            spec.panels[0].agent_session_id = resume.map(String::from);
        });
        assert_eq!(
            inspect_with(
                &fixture.store,
                &fixture.recovered,
                "terminal",
                Inspection::SavedIntent,
                |_, _, _| panic!("no SSH")
            ),
            Err(RemotePanelStatusError::UnsupportedIntent)
        );
    }
}

#[test]
fn pending_management_invalidates_both_inspection_modes_without_ssh() {
    let mut fixture = Fixture::new();
    set_intent(&mut fixture, |spec| spec.panels[0].command = Some(command()));
    let before = fixture.current();
    let mut next = before.workspace().state().clone();
    let runtime = next.runtime.as_mut().expect("runtime");
    runtime.phase = RemoteRuntimePhase::Cancelling;
    runtime.cleanup = Some(RemoteCleanupIntent {
        reason: RemoteCleanupReason::Cancelled,
        requested_at_millis: 1,
    });
    fixture
        .store
        .replace_remote_workspace(before.workspace(), &next)
        .expect("management intent");
    for inspection in [Inspection::Status, Inspection::SavedIntent] {
        assert_eq!(
            inspect_with(
                &fixture.store,
                &fixture.recovered,
                "terminal",
                inspection,
                |_, _, _| panic!("no SSH")
            ),
            Err(RemotePanelStatusError::StateChanged)
        );
    }
    assert_eq!(fixture.current().workspace().state(), &next);
}

#[test]
fn verification_retains_shared_ownership_lifetime_and_late_result_gates() {
    let mut fixture = Fixture::new();
    set_intent(&mut fixture, |spec| spec.panels[0].command = Some(command()));
    assert_eq!(
        inspect_with(
            &fixture.store,
            &fixture.recovered,
            "absent",
            Inspection::SavedIntent,
            |_, _, _| panic!("no SSH")
        ),
        Err(RemotePanelStatusError::UnknownPanel)
    );
    let before = fixture.current();
    assert_eq!(
        inspect_with(
            &fixture.store,
            &fixture.recovered,
            "terminal",
            Inspection::SavedIntent,
            |_, _, _| Err(RemotePanelStatusError::QueryFailed)
        ),
        Err(RemotePanelStatusError::QueryFailed)
    );
    assert_eq!(fixture.current(), before);
    assert_eq!(
        inspect_with(
            &fixture.store,
            &fixture.recovered,
            "terminal",
            Inspection::SavedIntent,
            |_, _, _| {
                fixture.edit_intent();
                Ok(RUNNING.to_vec())
            }
        ),
        Err(RemotePanelStatusError::StateChanged)
    );
    assert_eq!(
        inspect_with(
            &fixture.store,
            &fixture.recovered,
            "terminal",
            Inspection::SavedIntent,
            |_, _, _| panic!("no SSH")
        ),
        Err(RemotePanelStatusError::StateChanged)
    );
    let fixture = Fixture::with_lifecycle(InteractiveWorkerLifecycle::Stopped);
    assert_eq!(
        inspect_with(
            &fixture.store,
            &fixture.recovered,
            "terminal",
            Inspection::SavedIntent,
            |_, _, _| panic!("no SSH")
        ),
        Err(RemotePanelStatusError::WorkerUnavailable)
    );
}
