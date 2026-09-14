use super::*;
use crate::{
    cloud_run::{ArtifactDigest, CloudProvider},
    remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason, RemoteWorkspaceState, RepositoryCheckpoint},
};
use std::sync::{Arc, Barrier};

mod fixture;
use fixture::{Fixture, OWNER, draft};

#[test]
fn competing_snapshot_and_revision_writes_require_refresh() {
    for conflict in [
        RemoteWorkspaceStoreError::SnapshotConflict,
        RemoteWorkspaceStoreError::RevisionConflict { expected: 1, actual: 2 },
    ] {
        assert_eq!(storage_error(&conflict), RemoteShellPanelError::StateChanged);
    }
}

#[test]
fn three_independent_intents_preserve_the_allocation_and_every_existing_field() {
    for provider in [CloudProvider::LocalDocker, CloudProvider::Azure, CloudProvider::RunPod] {
        let fixture = Fixture::new(provider, WorkerLifetime::Persistent, true);
        fixture.edit(|state| {
            state.checkpoint = Some(RepositoryCheckpoint {
                workspace_local_id: state.spec.workspace_local_id.clone(),
                base_commit: state.spec.repository.commit.clone(),
                manifest_digest: ArtifactDigest::parse_sha256("c".repeat(64)).expect("digest"),
                runtime_generation: state.spec.generation,
                generation: 1,
                captured_at_millis: 1,
                recovery_artifact: None,
            });
        });
        let initial = fixture.current();
        let counts = fixture.counts();
        let mut expected = initial.workspace().state().clone();
        for _ in 0..2 {
            let prepared = fixture.prepare();
            assert_eq!(fixture.current().workspace().state(), &expected, "preview is read-only");
            assert_eq!(prepared.environment(), &fixture.summary());
            let panel = prepared.panel().clone();
            let added = add_remote_shell_panel(&fixture.store, OWNER, &fixture.summary(), prepared).expect("save");
            assert_eq!(added.panel_id, panel.panel_local_id);
            expected.spec.panels.push(panel);
            assert_eq!(fixture.current().workspace().state(), &expected);
            assert_eq!(added.environment, fixture.summary());
            assert_eq!(fixture.current().workflow(), initial.workflow());
            assert_eq!(fixture.counts(), counts, "no allocation or creation grant");
        }
        let names: std::collections::HashSet<_> = expected
            .spec
            .panels
            .iter()
            .map(|panel| panel.tmux_session_name().expect("tmux identity"))
            .collect();
        assert_eq!(names.len(), 3);
        assert!(!fixture.directory.path().join("keys").exists());
    }
}

#[test]
fn abandoning_a_confirmation_saves_nothing() {
    let fixture = Fixture::ready();
    let initial = fixture.current();
    drop(fixture.prepare());
    assert_eq!(fixture.current(), initial);
}

#[test]
fn stale_and_duplicate_confirmations_never_append_a_second_panel() {
    let fixture = Fixture::ready();
    let expected = fixture.summary();
    let first = fixture.prepare();
    let stale = fixture.prepare();
    let first_id = first.panel().panel_local_id.clone();
    add_remote_shell_panel(&fixture.store, OWNER, &expected, first).expect("first save");
    let saved = fixture.current();
    assert_eq!(
        add_remote_shell_panel(&fixture.store, OWNER, &expected, stale).err(),
        Some(RemoteShellPanelError::StateChanged)
    );
    assert_eq!(fixture.current(), saved);
    assert_eq!(
        saved
            .workspace()
            .state()
            .spec
            .panels
            .iter()
            .filter(|p| p.panel_local_id == first_id)
            .count(),
        1
    );
}

#[test]
fn racing_controllers_append_only_the_winning_confirmation() {
    let fixture = Fixture::ready();
    let gate = Arc::new(Barrier::new(2));
    let joins: Vec<_> = [fixture.prepare(), fixture.prepare()]
        .into_iter()
        .map(|prepared| {
            let gate = gate.clone();
            let store = fixture.store.clone();
            let expected = fixture.summary();
            std::thread::spawn(move || {
                gate.wait();
                add_remote_shell_panel(&store, OWNER, &expected, prepared)
            })
        })
        .collect();
    let results: Vec<_> = joins.into_iter().map(|join| join.join().expect("controller")).collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results.into_iter().find_map(Result::err),
        Some(RemoteShellPanelError::StateChanged)
    );
    assert_eq!(fixture.summary().panel_count, 2);
}

#[test]
fn owner_selection_and_store_changes_refuse_without_saving() {
    let fixture = Fixture::ready();
    let initial = fixture.current();
    let expected = fixture.summary();
    let foreign = "00000000-0000-4000-8000-000000000002";
    assert_eq!(
        prepare_remote_shell_panel(&fixture.store, foreign, &expected, draft()).err(),
        Some(RemoteShellPanelError::ClientSessionMismatch)
    );
    assert_eq!(
        add_remote_shell_panel(&fixture.store, foreign, &expected, fixture.prepare()).err(),
        Some(RemoteShellPanelError::ClientSessionMismatch)
    );
    let mut changed = expected.clone();
    changed.revision += 1;
    assert_eq!(
        add_remote_shell_panel(&fixture.store, OWNER, &changed, fixture.prepare()).err(),
        Some(RemoteShellPanelError::StateChanged)
    );
    let other = Fixture::ready();
    let other_initial = other.current();
    assert_eq!(
        add_remote_shell_panel(&other.store, OWNER, &expected, fixture.prepare()).err(),
        Some(RemoteShellPanelError::StateChanged)
    );
    assert_eq!(fixture.current(), initial);
    assert_eq!(other.current(), other_initial);
}

#[test]
fn invalid_commands_directories_and_panel_limit_leave_the_record_unchanged() {
    let fixture = Fixture::ready();
    for invalid in [
        RemoteShellPanelDraft {
            command: RemotePanelCommand {
                program: String::new(),
                args: vec![],
            },
            working_directory: None,
        },
        RemoteShellPanelDraft {
            command: RemotePanelCommand {
                program: "/bin/sh".into(),
                args: vec!["bad\0arg".into()],
            },
            working_directory: None,
        },
        RemoteShellPanelDraft {
            working_directory: Some("../outside".into()),
            ..draft()
        },
    ] {
        let initial = fixture.current();
        assert_eq!(
            prepare_remote_shell_panel(&fixture.store, OWNER, &fixture.summary(), invalid).err(),
            Some(RemoteShellPanelError::InvalidIntent)
        );
        assert_eq!(fixture.current(), initial);
    }
    fixture.edit(|state| {
        let original = state.spec.panels[0].clone();
        for index in 1..256 {
            state.spec.panels.push(RemotePanelBinding {
                panel_local_id: format!("existing-{index}"),
                ..original.clone()
            });
        }
    });
    let initial = fixture.current();
    assert_eq!(
        prepare_remote_shell_panel(&fixture.store, OWNER, &fixture.summary(), draft()).err(),
        Some(RemoteShellPanelError::InvalidIntent)
    );
    assert_eq!(fixture.current(), initial);
}

#[test]
fn missing_pin_and_nonpersistent_workers_are_ineligible() {
    for fixture in [
        Fixture::new(CloudProvider::LocalDocker, WorkerLifetime::Persistent, false),
        Fixture::new(
            CloudProvider::LocalDocker,
            WorkerLifetime::TimeLimited { seconds: 60 },
            true,
        ),
    ] {
        assert_eq!(
            prepare_remote_shell_panel(&fixture.store, OWNER, &fixture.summary(), draft()).err(),
            Some(RemoteShellPanelError::Unavailable)
        );
    }
}

#[test]
fn management_phases_and_cleanup_conflict_even_with_retained_trust() {
    for phase in [
        RemoteRuntimePhase::Materializing,
        RemoteRuntimePhase::Checkpointing,
        RemoteRuntimePhase::Cancelling,
        RemoteRuntimePhase::Deleting,
        RemoteRuntimePhase::Failed,
    ] {
        let fixture = Fixture::ready();
        fixture.edit(|state| {
            let runtime = state.runtime.as_mut().expect("runtime");
            if matches!(phase, RemoteRuntimePhase::Cancelling | RemoteRuntimePhase::Deleting) {
                runtime.cleanup = Some(RemoteCleanupIntent {
                    reason: RemoteCleanupReason::WorkspaceRemoved,
                    requested_at_millis: 1,
                });
            }
            runtime.phase = phase;
        });
        assert_eq!(
            prepare_remote_shell_panel(&fixture.store, OWNER, &fixture.summary(), draft()).err(),
            Some(RemoteShellPanelError::Unavailable)
        );
    }
    let fixture = Fixture::ready();
    fixture.edit(|state| {
        state.runtime.as_mut().expect("runtime").cleanup = Some(RemoteCleanupIntent {
            reason: RemoteCleanupReason::WorkspaceRemoved,
            requested_at_millis: 1,
        });
    });
    assert_eq!(
        prepare_remote_shell_panel(&fixture.store, OWNER, &fixture.summary(), draft()).err(),
        Some(RemoteShellPanelError::Unavailable)
    );
}

#[test]
fn stop_and_delete_after_preview_invalidate_the_confirmation() {
    for delete in [false, true] {
        let fixture = Fixture::ready();
        let expected = fixture.summary();
        let prepared = fixture.prepare();
        if delete {
            fixture
                .store
                .record_remote_delete_phase(
                    &fixture.current(),
                    RemoteRuntimePhase::DeleteRequested { requested_at_millis: 1 },
                )
                .expect("delete intent");
        } else {
            fixture
                .store
                .record_remote_stop_phase(
                    &fixture.current(),
                    RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
                )
                .expect("stop intent");
        }
        let retained = fixture.current();
        assert_eq!(
            add_remote_shell_panel(&fixture.store, OWNER, &expected, prepared).err(),
            Some(RemoteShellPanelError::StateChanged)
        );
        assert_eq!(
            prepare_remote_shell_panel(&fixture.store, OWNER, &fixture.summary(), draft()).err(),
            Some(RemoteShellPanelError::Unavailable)
        );
        assert_eq!(fixture.current(), retained);
    }
}

#[test]
fn stopped_and_starting_workers_cannot_accept_new_intent() {
    let fixture = Fixture::ready();
    let stopped = fixture
        .store
        .record_remote_stop_phase(
            &fixture.current(),
            RemoteRuntimePhase::Stopped {
                requested_at_millis: 1,
                observed_at_millis: 2,
            },
        )
        .expect("stopped");
    assert_eq!(
        prepare_remote_shell_panel(&fixture.store, OWNER, &fixture.summary(), draft()).err(),
        Some(RemoteShellPanelError::Unavailable)
    );
    fixture
        .store
        .record_remote_start_phase(&stopped, RemoteRuntimePhase::Starting { requested_at_millis: 3 })
        .expect("start intent");
    assert_eq!(
        prepare_remote_shell_panel(&fixture.store, OWNER, &fixture.summary(), draft()).err(),
        Some(RemoteShellPanelError::Unavailable)
    );
}
