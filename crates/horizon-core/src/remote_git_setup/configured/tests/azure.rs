//! Azure admission uses synthetic saved metadata; no Azure, SSH or PAT exchange is run.

use super::*;
use crate::{
    cloud_run::azure::AzureDiskSku,
    remote_ssh_identity::RemoteSshIdentityError,
    remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason, RemoteRuntimePhase},
};
use std::sync::atomic::{AtomicUsize, Ordering};

fn fixture() -> Fixture {
    Fixture::new(CloudProvider::Azure, false)
}

fn corrupt_binding(connection: &rusqlite::Connection, sql: &str) {
    let triggers: Vec<String> = connection
        .prepare("SELECT sql FROM sqlite_schema WHERE name IN ('remote_provider_bindings_no_update', 'remote_provider_bindings_no_delete') ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    connection.execute_batch("DROP TRIGGER remote_provider_bindings_no_update; DROP TRIGGER remote_provider_bindings_no_delete; PRAGMA ignore_check_constraints=ON").unwrap();
    connection.execute_batch(sql).unwrap();
    for trigger in triggers {
        connection.execute_batch(&trigger).unwrap();
    }
    connection.execute_batch("PRAGMA ignore_check_constraints=OFF").unwrap();
}

#[test]
fn preview_reuses_exact_saved_source_modes_and_binding_without_identity_or_writes() {
    let f = fixture();
    let before = f.current();
    let home = HorizonHome::from_root(f.directory.path().join("home"));
    let reader = CloudWorkflowStore::open_read_only(&home).unwrap();
    for mode in [
        RemoteGitCredentialMode::UseInstalled,
        RemoteGitCredentialMode::InstallFirst,
    ] {
        let prepared = prepare_configured_remote_git_setup(&reader, &f.config, request(&f.summary()), mode).unwrap();
        assert_eq!(prepared.repository(), "fixture/project");
        assert_eq!(prepared.commit(), "a".repeat(40));
        assert_eq!(prepared.work_branch(), "work/one");
        assert_eq!(prepared.credential_mode(), mode);
        assert_eq!(prepared.environment(), &f.summary());
        assert_eq!(prepared.allocation, before);
        assert!(prepared.selection.is_none());
    }
    assert_eq!(f.current(), before);
    assert!(!home.root().join("remote-ssh-identities").exists());
    // Production construction is lazy and does not invoke az, ARM or the host-key source.
    let client = super::super::azure::client(&reader, &f.config, &before).unwrap();
    assert_eq!(client.provider(), CloudProvider::Azure);
    assert_eq!(f.current(), before);
}

#[test]
fn every_frozen_profile_field_rejects_preview_and_consumed_confirmation_drift() {
    for field in 0..8 {
        let f = fixture();
        let prepared = preview(&f, RemoteGitCredentialMode::UseInstalled);
        let before = f.current();
        let mut config = f.config.clone();
        let profile = &mut config.azure[0];
        match field {
            0 => profile.name.push_str("-changed"),
            1 => profile.subscription_id = "22222222-2222-4222-8222-222222222222".into(),
            2 => profile.location = "westeurope".into(),
            3 => profile.vm_size = "Standard_D2s_v3".into(),
            4 => profile.image_pull_identity_id.push_str("-changed"),
            5 => profile.declared_hourly_cost_micros += 1,
            6 => profile.registry_login_server = "different.azurecr.io".into(),
            _ => profile.disk_sku = AzureDiskSku::PremiumLrs,
        }
        assert!(
            prepare(
                &f.store,
                &config,
                request(&f.summary()),
                RemoteGitCredentialMode::UseInstalled
            )
            .is_err()
        );
        assert!(matches!(
            submit_configured_remote_git_setup(
                &f.store,
                &identities(&f),
                &config,
                request(&f.summary()),
                prepared,
                None,
            ),
            Err(ConfiguredRemoteGitSetupError::Configuration(_)
                | ConfiguredRemoteGitSetupError::InvalidBinding
                | ConfiguredRemoteGitSetupError::StateChanged)
        ));
        assert_eq!(f.current(), before);
        assert!(!f.directory.path().join("missing-keys").exists());
    }
}

#[test]
fn missing_or_corrupt_cpu_provenance_is_never_backfilled_from_current_profile() {
    for sql in [
        "DELETE FROM remote_provider_bindings",
        "UPDATE remote_provider_bindings SET profile_digest = 'malformed'",
    ] {
        let f = fixture();
        let before = f.current();
        let home = HorizonHome::from_root(f.directory.path().join("home"));
        let connection = rusqlite::Connection::open(home.cloud_workflow_store_path()).unwrap();
        corrupt_binding(&connection, sql);
        let binding_rows = || {
            connection
                .query_row(
                    "SELECT count(*), coalesce(max(profile_digest), '') FROM remote_provider_bindings",
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .unwrap()
        };
        let broken = binding_rows();
        assert!(
            prepare(
                &f.store,
                &f.config,
                request(&f.summary()),
                RemoteGitCredentialMode::UseInstalled
            )
            .is_err()
        );
        assert!(matches!(
            inspect_configured_remote_git_setup(&f.store, &identities(&f), &f.config, request(&f.summary())),
            Err(ConfiguredRemoteGitSetupError::InvalidBinding
                | ConfiguredRemoteGitSetupError::Recovery(RemoteWorkspaceRecoveryError::StorageUnavailable))
        ));
        assert_eq!(binding_rows(), broken);
        assert_eq!(f.current(), before);
        assert!(!home.root().join("remote-ssh-identities").exists());
    }
}

#[test]
fn saved_handle_branch_trust_and_management_admission_fail_before_credentials() {
    for case in 0..6 {
        let f = fixture();
        let original = f.summary();
        let change = |state: &mut RemoteWorkspaceState| {
            let runtime = state.runtime.as_mut().unwrap();
            match case {
                0 => runtime.worker.as_mut().unwrap().identity.resource_id = "wrong-worker".into(),
                1 => runtime.ssh = None,
                2 => state.spec.repository.branch = None,
                3 => runtime.phase = RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
                4 => {
                    runtime.phase = RemoteRuntimePhase::Stopped {
                        requested_at_millis: 1,
                        observed_at_millis: 2,
                    }
                }
                _ => {
                    runtime.cleanup = Some(RemoteCleanupIntent {
                        requested_at_millis: 1,
                        reason: RemoteCleanupReason::Cancelled,
                    });
                }
            }
        };
        if case < 3 {
            // Public replacement correctly forbids rewriting retained identity, trust or source.
            // Seed only this disposable database to test admission of malformed saved input.
            let mut state = f.current().workspace().state().clone();
            change(&mut state);
            state.validate().unwrap();
            let bytes = serde_json::to_vec(&serde_json::json!({"session_id": OWNER, "state": state})).unwrap();
            let home = HorizonHome::from_root(f.directory.path().join("home"));
            rusqlite::Connection::open(home.cloud_workflow_store_path())
                .unwrap()
                .execute("UPDATE remote_workspaces SET snapshot = ?1", [bytes])
                .unwrap();
        } else if case == 4 {
            f.store
                .record_remote_stop_phase(
                    &f.current(),
                    RemoteRuntimePhase::Stopped {
                        requested_at_millis: 1,
                        observed_at_millis: 2,
                    },
                )
                .unwrap();
        } else {
            f.edit(change);
        }
        let home = HorizonHome::from_root(f.directory.path().join("home"));
        let connection = rusqlite::Connection::open(home.cloud_workflow_store_path()).unwrap();
        let snapshot = || {
            connection
                .query_row("SELECT snapshot FROM remote_workspaces", [], |row| {
                    row.get::<_, Vec<u8>>(0)
                })
                .unwrap()
        };
        let before = snapshot();
        let summary = match f.store.load_remote_allocation(OWNER, "workspace") {
            Ok(Some(allocation)) => allocation.workspace().environment_summary(),
            Err(_) if case < 3 => original, // Corrupt input can be rejected by the store before Git admission.
            other => panic!("unexpected fixture recovery: {other:?}"),
        };
        assert!(
            prepare(
                &f.store,
                &f.config,
                request(&summary),
                RemoteGitCredentialMode::UseInstalled
            )
            .is_err()
        );
        assert_eq!(snapshot(), before);
    }
    let f = fixture();
    let summary = f.summary();
    let foreign = ConfiguredRemoteGitSetupRequest {
        expected: &summary,
        client_session_id: "foreign",
    };
    assert!(matches!(
        prepare(&f.store, &f.config, foreign, RemoteGitCredentialMode::UseInstalled),
        Err(ConfiguredRemoteGitSetupError::ClientSessionMismatch)
    ));
}

#[test]
fn missing_private_identity_prevents_both_modes_and_manual_inspection_without_replacement() {
    let f = fixture();
    let before = f.current();
    let token = RepositoryPat::new("synthetic_repository_pat").unwrap();
    for mode in [
        RemoteGitCredentialMode::UseInstalled,
        RemoteGitCredentialMode::InstallFirst,
    ] {
        let prepared = preview(&f, mode);
        let result = submit_configured_remote_git_setup(
            &f.store,
            &identities(&f),
            &f.config,
            request(&f.summary()),
            prepared,
            (mode == RemoteGitCredentialMode::InstallFirst).then_some(&token),
        );
        assert_eq!(
            result,
            Err(ConfiguredRemoteGitSetupError::Recovery(
                RemoteWorkspaceRecoveryError::Identity(RemoteSshIdentityError::Missing),
            ))
        );
    }
    assert_eq!(
        inspect_configured_remote_git_setup(&f.store, &identities(&f), &f.config, request(&f.summary())),
        Err(ConfiguredRemoteGitSetupError::Recovery(
            RemoteWorkspaceRecoveryError::Identity(RemoteSshIdentityError::Missing),
        )),
    );
    assert_eq!(f.current(), before);
    assert!(!f.directory.path().join("missing-keys").exists());
}

struct Provider {
    status: Option<InteractiveWorkerStatus>,
    inspections: AtomicUsize,
}

impl InteractiveWorkerProvider for Provider {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        CloudProvider::Azure
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("repository preparation cannot ensure workers")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("repository preparation cannot reconcile missing workers")
    }
    fn inspect_worker(&self, expected: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        self.inspections.fetch_add(1, Ordering::SeqCst);
        if let Some(status) = &self.status {
            assert_eq!(status.worker, *expected);
        }
        Ok(self.status.clone())
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("repository preparation cannot delete workers")
    }
}

#[test]
fn shared_git_admission_inspects_only_retained_worker_and_never_replaces_pin() {
    let f = Fixture::build(
        CloudProvider::Azure,
        false,
        WorkerLifetime::Persistent,
        true,
        Some("work/one"),
        true,
    );
    let before = f.current();
    let runtime = before.workspace().state().runtime.as_ref().unwrap();
    let identities = RemoteSshIdentityStore::new(&HorizonHome::from_root(f.directory.path().join("home")));
    for case in 0..4 {
        let mut status = InteractiveWorkerStatus {
            worker: runtime.worker.clone().unwrap(),
            lifecycle: InteractiveWorkerLifecycle::Ready,
            ssh: runtime.ssh.clone(),
        };
        if case == 1 {
            status.ssh.as_mut().unwrap().host_key = runtime.ssh_public_key.clone().unwrap();
        }
        if case == 2 {
            status.lifecycle = InteractiveWorkerLifecycle::Stopped;
        }
        let provider = Provider {
            status: (case != 3).then_some(status),
            inspections: AtomicUsize::new(0),
        };
        let mut exchanges = 0;
        let result = crate::remote_git_setup::operate_with(
            &f.store,
            &identities,
            &provider,
            &before,
            "work/one",
            |_, recovered, bytes| {
                exchanges += 1;
                let input = crate::repository_git::GitPreparation::decode(bytes).unwrap();
                assert_eq!(input.source, before.workspace().state().spec.repository);
                assert_eq!(input.work_branch, "work/one");
                assert_eq!(recovered.allocation(), &before);
                Ok(RemoteGitSubmission::Submitted)
            },
        );
        assert_eq!(exchanges, usize::from(case == 0));
        assert_eq!(result.is_ok(), case == 0);
        assert_eq!(provider.inspections.load(Ordering::SeqCst), 1);
        assert_eq!(f.current(), before);
    }
}
