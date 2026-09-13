use super::*;

#[cfg(not(target_os = "linux"))]
#[test]
fn unsupported_platform_keeps_the_explicit_boundary() {
    let _entry = refresh_configured_runpod_connection;
    assert!(Error::UnsupportedProvider.to_string().contains("Linux"));
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::{
        HorizonHome,
        cloud_run::{
            WorkerLifetime,
            interactive_worker::{
                InteractiveWorkerIdentity, InteractiveWorkerLifecycle, InteractiveWorkerLifetime,
                InteractiveWorkerSshEndpoint, InteractiveWorkerStatus,
            },
            runpod::RunPodNetworkVolumeExpectation,
        },
        remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason, RemoteRuntimePhase, RemoteWorkspaceState},
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use std::cell::RefCell;

    const OWNER: &str = "00000000-0000-4000-8000-000000000001";
    struct Fixture {
        directory: tempfile::TempDir,
        store: CloudWorkflowStore,
        profile: RunPodProfile,
    }
    impl Fixture {
        fn new(network: bool, pin: bool) -> Self {
            let directory = tempfile::tempdir().expect("private fixture");
            let store = CloudWorkflowStore::open_path(directory.path().join("control/store.sqlite3")).expect("store");
            let state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({"version":1,"spec":{
                "workspace_local_id":"workspace","working_directory":".","generation":0,"panels":[],
                "target":{"provider":"run_pod","profile":"development","disk_gib":20,"lifetime":"persistent",
                    "image":format!("example/worker@sha256:{}","a".repeat(64))},
                "repository":{"repository":"example/project","commit":"b".repeat(40)}}}))
            .expect("state");
            let saved = store.create_remote_workspace(OWNER, &state).expect("workspace");
            let allocation = store.allocate_remote_runtime(&saved, i64::MAX).expect("allocation");
            if network {
                store
                    .record_remote_network_volume_selection(
                        &allocation,
                        &RunPodNetworkVolumeExpectation {
                            volume_id: "synthetic-volume".into(),
                            data_center_id: "synthetic-dc".into(),
                            minimum_size_gb: 10,
                        },
                    )
                    .expect("HPS selection");
            }
            let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
            blob.extend([7; 32]);
            let key = format!("ssh-ed25519 {}", STANDARD.encode(blob));
            let reserved = store.reserve_remote_worker_request(&allocation, &key).expect("request");
            let request = reserved.worker_request().expect("request");
            store
                .record_remote_worker_recovery(
                    &reserved,
                    Some(&InteractiveWorkerStatus {
                        worker: InteractiveWorker {
                            identity: InteractiveWorkerIdentity {
                                provider: CloudProvider::RunPod,
                                workflow_id: request.workflow_id,
                                job_id: request.job_id,
                                resource_id: "synthetic-worker".into(),
                            },
                            target: request.target,
                            ssh_public_key: request.ssh_public_key,
                            lifetime: InteractiveWorkerLifetime::Persistent,
                        },
                        lifecycle: if pin {
                            InteractiveWorkerLifecycle::Ready
                        } else {
                            InteractiveWorkerLifecycle::Provisioning
                        },
                        ssh: pin.then_some(InteractiveWorkerSshEndpoint {
                            host: "203.0.113.9".into(),
                            port: 2222,
                            username: "root".into(),
                            host_key: key,
                        }),
                    }),
                )
                .expect("retained worker");
            let profile = serde_json::from_value(
                serde_json::json!({"name":"development","gpu_type_ids":["synthetic-gpu"],
                "gpu_count":1,"ports":["22/tcp"],"volume_gib":10,"data_center_id":"synthetic-dc"}),
            )
            .expect("profile");
            Self {
                directory,
                store,
                profile,
            }
        }
        fn current(&self) -> StoredRemoteAllocation {
            self.store
                .load_remote_allocation(OWNER, "workspace")
                .expect("load")
                .expect("allocation")
        }
        fn phase(&self, phase: RemoteRuntimePhase) {
            if matches!(
                phase,
                RemoteRuntimePhase::Stopped { .. } | RemoteRuntimePhase::Starting { .. }
            ) {
                let stopped = self
                    .store
                    .record_remote_stop_phase(
                        &self.current(),
                        RemoteRuntimePhase::Stopped {
                            requested_at_millis: 1,
                            observed_at_millis: 2,
                        },
                    )
                    .expect("Stopped");
                if matches!(phase, RemoteRuntimePhase::Starting { .. }) {
                    self.store
                        .record_remote_start_phase(&stopped, phase)
                        .expect("Start intent");
                }
            } else {
                let current = self.current();
                let mut state = current.workspace().state().clone();
                state.runtime.as_mut().expect("runtime").phase = phase;
                self.store
                    .replace_remote_workspace(current.workspace(), &state)
                    .expect("phase");
            }
        }
        fn drift(&self, binding: bool) {
            if binding {
                // Corrupt only the synthetic selection, restoring the required schema atomically.
                let mut db = rusqlite::Connection::open(self.store.path()).expect("fixture writer");
                let tx = db.transaction().expect("transaction");
                let trigger: String = tx
                    .query_row(
                        "SELECT sql FROM sqlite_schema WHERE name='remote_network_volume_selections_no_update'",
                        [],
                        |row| row.get(0),
                    )
                    .expect("trigger");
                tx.execute_batch("DROP TRIGGER remote_network_volume_selections_no_update")
                    .expect("fault seam");
                tx.execute("UPDATE remote_network_volume_selections SET volume_id='changed-volume' WHERE workspace_local_id='workspace'", []).expect("drift");
                tx.execute_batch(&trigger).expect("restore schema");
                tx.commit().expect("fixture commit");
            } else {
                let current = self.current();
                let mut workflow = current.workflow().workflow().clone();
                workflow.updated_at_millis += 1;
                self.store
                    .replace(current.workflow(), &workflow)
                    .expect("concurrent change");
            }
        }
    }

    #[test]
    fn public_entry_refuses_foreign_or_unconfigured_and_missing_identity_without_creation() {
        let f = Fixture::new(true, true);
        let before = f.current();
        let home = HorizonHome::from_root(f.directory.path().join("absent-identity-home"));
        let identities = RemoteSshIdentityStore::new(&home);
        for provider in [CloudProvider::Azure, CloudProvider::LocalDocker, CloudProvider::RunPod] {
            let mut expected = before.workspace().environment_summary();
            expected.provider = provider;
            let result = refresh_configured_runpod_connection(
                &f.store,
                &identities,
                &RemoteProviderConfig::default(),
                &expected,
            );
            if provider == CloudProvider::RunPod {
                assert!(matches!(result, Err(Error::Configuration(_))));
            } else {
                assert_eq!(result, Err(Error::UnsupportedProvider));
            }
        }
        let config = RemoteProviderConfig {
            runpod: vec![f.profile.clone()],
            ..Default::default()
        };
        assert_eq!(
            refresh_configured_runpod_connection(
                &f.store,
                &identities,
                &config,
                &before.workspace().environment_summary()
            ),
            Err(RefreshError::IdentityUnavailable.into())
        );
        assert!(!home.root().exists());
        assert_eq!(f.current(), before);
    }

    #[test]
    fn admission_refuses_before_identity_credentials_or_refresh() {
        for fault in 0..11 {
            let mut f = Fixture::new(fault != 0, fault != 1);
            match fault {
                2 => f.profile.name = "foreign-profile".into(),
                3 => f.profile.data_center_id = Some("foreign-dc".into()),
                4 => f.profile.ports.clear(),
                9 => {
                    let current = f.current();
                    let mut state = current.workspace().state().clone();
                    state.runtime.as_mut().expect("runtime").cleanup = Some(RemoteCleanupIntent {
                        reason: RemoteCleanupReason::Cancelled,
                        requested_at_millis: 1,
                    });
                    f.store
                        .replace_remote_workspace(current.workspace(), &state)
                        .expect("cleanup");
                }
                10 => {
                    f.store
                        .record_remote_stop_phase(&f.current(), RemoteRuntimePhase::Stopping { requested_at_millis: 1 })
                        .expect("Stop intent");
                }
                _ => {}
            }
            let before = f.current();
            let mut expected = before.workspace().environment_summary();
            match fault {
                5 => expected.revision += 1,
                6 => expected.owning_session_id = "00000000-0000-4000-8000-000000000002".into(),
                7 => expected.worker_identity.as_mut().expect("worker").resource_id = "foreign-worker".into(),
                8 => expected.lifetime = WorkerLifetime::TimeLimited { seconds: 900 },
                _ => {}
            }
            let result = refresh_with::<()>(
                &f.store,
                &f.profile,
                &expected,
                |_| panic!("no identity for {fault}"),
                |_| panic!("no credentials"),
                |(), _| panic!("no refresh"),
            );
            assert!(result.is_err(), "fault {fault}");
            assert_eq!(f.current(), before);
        }
    }

    #[test]
    fn recovery_precedes_credentials_and_failures_never_dispatch() {
        for missing_identity in [false, true] {
            let f = Fixture::new(true, true);
            let before = f.current();
            let calls = RefCell::new(Vec::new());
            let result = refresh_with::<()>(
                &f.store,
                &f.profile,
                &before.workspace().environment_summary(),
                |_| {
                    calls.borrow_mut().push("identity");
                    if missing_identity {
                        Err(RefreshError::IdentityUnavailable.into())
                    } else {
                        Ok(())
                    }
                },
                |_| {
                    calls.borrow_mut().push("credential");
                    Err(Error::CredentialUnavailable)
                },
                |(), _| panic!("no provider or SSH"),
            );
            let expected = if missing_identity {
                RefreshError::IdentityUnavailable.into()
            } else {
                Error::CredentialUnavailable
            };
            assert_eq!(result, Err(expected));
            assert_eq!(
                *calls.borrow(),
                if missing_identity {
                    vec!["identity"]
                } else {
                    vec!["identity", "credential"]
                }
            );
            assert_eq!(f.current(), before);
        }
    }

    #[test]
    fn allocation_and_hps_drift_win_even_when_identity_credentials_or_refresh_fail() {
        for stage in 0..3 {
            for binding in [false, true] {
                for failure in [false, true] {
                    let f = Fixture::new(true, true);
                    let expected = f.current().workspace().environment_summary();
                    let calls = RefCell::new(Vec::new());
                    let attempt = |step, name| {
                        calls.borrow_mut().push(name);
                        if stage == step {
                            f.drift(binding);
                            if failure {
                                return Err(());
                            }
                        }
                        Ok(())
                    };
                    let result = refresh_with(
                        &f.store,
                        &f.profile,
                        &expected,
                        |_| attempt(0, "identity").map_err(|()| RefreshError::IdentityUnavailable.into()),
                        |_| attempt(1, "credential").map_err(|()| Error::CredentialUnavailable),
                        |(), allocation| {
                            attempt(2, "refresh").map_err(|()| RefreshError::AuthenticationFailed)?;
                            Ok(allocation.clone())
                        },
                    );
                    assert_eq!(result, Err(RefreshError::StateChanged.into()));
                    assert_eq!(*calls.borrow(), ["identity", "credential", "refresh"][..=stage]);
                }
            }
        }
    }

    #[test]
    fn changed_and_unchanged_coordinates_preserve_original_identity_phase_and_intent() {
        for phase in [
            RemoteRuntimePhase::Ready,
            RemoteRuntimePhase::Reconciling,
            RemoteRuntimePhase::Stopped {
                requested_at_millis: 1,
                observed_at_millis: 2,
            },
            RemoteRuntimePhase::Starting { requested_at_millis: 3 },
        ] {
            for changed in [false, true] {
                let f = Fixture::new(true, true);
                f.phase(phase);
                let before = f.current();
                let expected = before.workspace().environment_summary();
                let calls = RefCell::new(Vec::new());
                let result = refresh_with(
                    &f.store,
                    &f.profile,
                    &expected,
                    |worker| {
                        calls.borrow_mut().push("identity");
                        assert_eq!(
                            &worker.ssh_public_key,
                            &before.worker_request().expect("request").ssh_public_key
                        );
                        Ok(())
                    },
                    |admitted| {
                        calls.borrow_mut().push("credential");
                        assert_eq!(admitted.allocation, before);
                        Ok(())
                    },
                    |(), allocation| {
                        calls.borrow_mut().push("refresh");
                        let mut ssh = allocation
                            .workspace()
                            .state()
                            .runtime
                            .as_ref()
                            .expect("runtime")
                            .ssh
                            .clone()
                            .expect("pin");
                        if changed {
                            ssh.port += 1;
                        }
                        f.store.record_remote_endpoint_refresh(allocation, &ssh)
                    },
                )
                .expect("saved refresh");
                let mut permitted = expected;
                permitted.revision += u64::from(changed);
                assert_eq!(result.saved, permitted);
                assert_eq!(*calls.borrow(), ["identity", "credential", "refresh"]);
                let mut state = before.workspace().state().clone();
                if changed {
                    state.runtime.as_mut().expect("runtime").ssh.as_mut().expect("pin").port += 1;
                }
                assert_eq!(f.current().workspace().state(), &state);
                assert_eq!(f.current().workflow(), before.workflow());
            }
        }
    }

    #[test]
    fn typed_summary_refuses_revision_overflow_and_identity_or_phase_changes() {
        let f = Fixture::new(true, true);
        let expected = f.current().workspace().environment_summary();
        for fault in 0..8 {
            let mut saved = expected.clone();
            match fault {
                0 => saved.revision += 2,
                1 => saved.owning_session_id.push_str("-changed"),
                2 => saved.saved_phase = Some(RemoteRuntimePhase::Ready),
                3 => saved.worker_identity = None,
                4 => saved.generation += 1,
                5 => saved.profile.push_str("-changed"),
                6 => saved.panel_count += 1,
                _ => saved.workflow_id = None,
            }
            assert_eq!(
                checked_summary(&expected, saved),
                Err(RefreshError::StateChanged.into())
            );
        }
        let mut maximum = expected;
        maximum.revision = u64::MAX;
        assert!(checked_summary(&maximum, maximum.clone()).is_ok());
        let mut wrapped = maximum.clone();
        wrapped.revision = 0;
        assert_eq!(
            checked_summary(&maximum, wrapped),
            Err(RefreshError::StateChanged.into())
        );
    }
}
