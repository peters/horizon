use super::*;
use crate::remote_provider_config::RemoteProviderConfig;

#[test]
fn public_check_refuses_unsupported_and_unconfigured_providers_without_writes() {
    let fixture = Fixture::new();
    let before = fixture.current();
    for provider in [CloudProvider::LocalDocker, CloudProvider::Azure, CloudProvider::RunPod] {
        let mut expected = before.workspace().environment_summary();
        expected.provider = provider;
        let result =
            confirm_configured_remote_environment_stop(&fixture.store, &RemoteProviderConfig::default(), &expected);
        // Azure and (on Linux) RunPod are dispatched by their named profile: an unconfigured
        // profile is a configuration error before any admission, credential or client work.
        if provider == CloudProvider::Azure || (cfg!(target_os = "linux") && provider == CloudProvider::RunPod) {
            assert!(matches!(result, Err(ConfiguredStopConfirmationError::Configuration(_))));
        } else {
            assert_eq!(result, Err(ConfiguredStopConfirmationError::UnsupportedProvider));
        }
        assert_eq!(fixture.current(), before);
    }
}

#[cfg(target_os = "linux")]
mod runpod {
    use super::*;
    use crate::cloud_run::{
        interactive_worker::{InteractiveWorkerLifecycle, InteractiveWorkerSshEndpoint},
        interactive_worker_stop::{
            InteractiveWorkerStopExpectation, InteractiveWorkerStopObservation as Observation,
            InteractiveWorkerStopObserver,
        },
        runpod::{RunPodApiKey, RunPodNetworkVolumeExpectation, RunPodProfile},
    };
    use crate::remote_workspace::stop::configured_confirmation::runpod_with;

    struct RunPodFixture {
        directory: tempfile::TempDir,
        store: CloudWorkflowStore,
        profile: RunPodProfile,
    }

    impl RunPodFixture {
        fn new(network: bool, lifetime: WorkerLifetime, intent: bool, pin: bool) -> Self {
            let directory = tempfile::tempdir().expect("fixture");
            let store = CloudWorkflowStore::open_path(directory.path().join("control/store.sqlite3")).expect("store");
            let mut state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
                "version":1, "spec":{
                    "workspace_local_id":"workspace", "working_directory":".", "generation":0, "panels":[],
                    "target":{"provider":"local_docker", "profile":"development", "disk_gib":20,
                        "lifetime":"persistent", "image":format!("example/worker@sha256:{}", "a".repeat(64))},
                    "repository":{"repository":"example/project", "commit":"b".repeat(40)}
                }
            }))
            .expect("state");
            state.spec.target.lifetime = lifetime;
            state.spec.target.provider = CloudProvider::RunPod;
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
                    .expect("saved selection");
            }
            let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
            blob.extend([7; 32]);
            let key = format!("ssh-ed25519 {}", STANDARD.encode(blob));
            let reserved = store
                .reserve_remote_worker_request(&allocation, &key)
                .expect("public request only");
            let request = reserved.worker_request().expect("request");
            let lease = if lifetime == WorkerLifetime::Persistent {
                InteractiveWorkerLifetime::Persistent
            } else {
                InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
                    terminate_after: (time::OffsetDateTime::now_utc() + time::Duration::seconds(900))
                        .format(&time::format_description::well_known::Rfc3339)
                        .expect("bounded lease"),
                })
            };
            let status = InteractiveWorkerStatus {
                worker: InteractiveWorker {
                    identity: InteractiveWorkerIdentity {
                        provider: CloudProvider::RunPod,
                        workflow_id: request.workflow_id,
                        job_id: request.job_id,
                        resource_id: "synthetic-worker".into(),
                    },
                    target: request.target,
                    ssh_public_key: request.ssh_public_key,
                    lifetime: lease,
                },
                lifecycle: if pin {
                    InteractiveWorkerLifecycle::Ready
                } else {
                    InteractiveWorkerLifecycle::Provisioning
                },
                ssh: pin.then_some(InteractiveWorkerSshEndpoint {
                    host: "127.0.0.1".into(),
                    port: 2222,
                    username: "root".into(),
                    host_key: key,
                }),
            };
            let retained = store
                .record_remote_worker_recovery(&reserved, Some(&status))
                .expect("synthetic retained public observation");
            if intent {
                store
                    .record_remote_stop_phase(&retained, RemoteRuntimePhase::Stopping { requested_at_millis: 1 })
                    .expect("existing intent, no provider Stop");
            }
            let profile = serde_json::from_value(serde_json::json!({
                "name":"development", "gpu_type_ids":["synthetic-gpu"], "gpu_count":1,
                "ports":["22/tcp"], "volume_gib":10, "data_center_id":"synthetic-dc"
            }))
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

        fn check(&self, observer: &Observer) -> Result<ConfiguredStopConfirmation, ConfiguredStopConfirmationError> {
            runpod_with(
                &self.store,
                &self.profile,
                &self.current().workspace().environment_summary(),
                key,
                |_, allocation| confirm_remote_workspace_stop(&self.store, observer, allocation),
            )
        }
    }

    struct Observer {
        result: Result<Observation, &'static str>,
        calls: Mutex<usize>,
        network: bool,
    }

    impl InteractiveWorkerProvider for Observer {
        type Error = std::io::Error;
        fn provider(&self) -> CloudProvider {
            CloudProvider::RunPod
        }
        fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
            panic!("no create")
        }
        fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
            panic!("no generic inspection")
        }
        fn reconcile_worker(
            &self,
            _: &InteractiveWorkerRequest,
        ) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
            panic!("no setup recovery")
        }
        fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
            panic!("no delete")
        }
    }

    impl InteractiveWorkerStopObserver for Observer {
        fn observe_worker_stop(
            &self,
            expected: InteractiveWorkerStopExpectation<'_>,
        ) -> Result<Observation, Self::Error> {
            assert_eq!(expected.network_volume.is_some(), self.network);
            assert!(expected.ssh.is_complete());
            *self.calls.lock().expect("calls") += 1;
            self.result.map_err(std::io::Error::other)
        }
    }

    fn key() -> Result<RunPodApiKey, ConfiguredStopConfirmationError> {
        RunPodApiKey::new("synthetic-provider-credential")
            .map_err(|_| ConfiguredStopConfirmationError::CredentialUnavailable)
    }

    #[test]
    fn typed_observations_use_actual_coordinator_without_private_key_or_replay() {
        for network in [false, true] {
            let fixture = RunPodFixture::new(network, WorkerLifetime::Persistent, true, true);
            let original = fixture.current();
            for result in [
                Ok(Observation::Pending),
                Ok(Observation::Absent),
                Err("private-provider-marker"),
                Ok(Observation::RetainedStopped),
            ] {
                let observer = Observer {
                    result,
                    calls: Mutex::new(0),
                    network,
                };
                let before = fixture.current();
                let checked = fixture.check(&observer);
                if let Ok(observation) = result {
                    let checked = checked.expect("observation");
                    assert_eq!(checked.observation, observation);
                    assert_eq!(checked.saved, fixture.current().workspace().environment_summary());
                    if observation == Observation::RetainedStopped {
                        let after = fixture.current();
                        assert_eq!(after.workflow(), before.workflow());
                        let mut permitted = before.workspace().state().clone();
                        permitted.runtime.as_mut().expect("runtime").phase =
                            after.workspace().state().runtime.as_ref().expect("runtime").phase;
                        assert_eq!(after.workspace().state(), &permitted);
                        assert_eq!(after.workspace().revision(), before.workspace().revision() + 1);
                    } else {
                        assert_eq!(fixture.current(), before);
                    }
                } else {
                    let error = checked.expect_err("unverified");
                    assert_eq!(error, ConfiguredStopConfirmationError::Stop(Error::ProviderUnavailable));
                    assert!(!format!("{error:?} {error}").contains("private-provider-marker"));
                    assert_eq!(fixture.current(), before);
                }
                assert_eq!(*observer.calls.lock().expect("calls"), 1);
            }
            let stopped = fixture.current();
            for result in [Observation::Pending, Observation::Absent, Observation::RetainedStopped] {
                fixture
                    .check(&Observer {
                        result: Ok(result),
                        calls: Mutex::new(0),
                        network,
                    })
                    .expect("repeat check");
                assert_eq!(fixture.current(), stopped, "original Stop times never renewed");
            }
            assert_eq!(original.workflow(), stopped.workflow());
            assert_eq!(
                fixture.directory.path().read_dir().expect("fixture root").count(),
                1,
                "only the control store; no private identity"
            );
        }
    }

    #[test]
    fn admission_refusals_precede_credentials_and_provider_calls() {
        for fault in 0..9 {
            let mut fixture = RunPodFixture::new(
                fault == 5,
                if fault == 1 {
                    WorkerLifetime::TimeLimited { seconds: 900 }
                } else {
                    WorkerLifetime::Persistent
                },
                ![0, 3, 6].contains(&fault),
                fault != 2,
            );
            if fault == 3 {
                let current = fixture.current();
                let mut state = current.workspace().state().clone();
                let runtime = state.runtime.as_mut().expect("runtime");
                runtime.cleanup = Some(RemoteCleanupIntent {
                    reason: RemoteCleanupReason::Cancelled,
                    requested_at_millis: 1,
                });
                fixture
                    .store
                    .replace_remote_workspace(current.workspace(), &state)
                    .expect("valid refusal fixture");
            }
            if fault == 6 {
                fixture
                    .store
                    .record_remote_stop_phase(
                        &fixture.current(),
                        RemoteRuntimePhase::Stopping {
                            requested_at_millis: i64::MAX,
                        },
                    )
                    .expect("future intent");
            }
            if fault == 4 {
                fixture.profile.name = "other-profile".into();
            }
            if fault == 5 {
                fixture.profile.data_center_id = Some("other-dc".into());
            }
            if fault == 7 {
                fixture.profile.volume_gib = 0;
            }
            let before = fixture.current();
            let mut expected = before.workspace().environment_summary();
            if fault == 8 {
                expected.panel_count += 1;
            }
            assert!(
                runpod_with(
                    &fixture.store,
                    &fixture.profile,
                    &expected,
                    || panic!("must not read credentials"),
                    |_, _| panic!("must not query provider")
                )
                .is_err(),
                "fault {fault}"
            );
            assert_eq!(fixture.current(), before);
        }
    }

    fn drift(fixture: &RunPodFixture, selection: bool) {
        if selection {
            let mut connection = rusqlite::Connection::open(fixture.store.path()).expect("fixture database");
            let transaction = connection.transaction().expect("fixture transaction");
            let trigger: String = transaction
                .query_row(
                    "SELECT sql FROM sqlite_schema WHERE name='remote_network_volume_selections_no_update'",
                    [],
                    |row| row.get(0),
                )
                .expect("trigger");
            transaction
                .execute_batch("DROP TRIGGER remote_network_volume_selections_no_update")
                .expect("fixture only");
            transaction.execute("UPDATE remote_network_volume_selections SET volume_id='changed-volume' WHERE workspace_local_id='workspace'", []).expect("selection drift");
            transaction.execute_batch(&trigger).expect("restore exact schema");
            transaction.commit().expect("atomic fixture");
        } else {
            let current = fixture.current();
            let mut workflow = current.workflow().workflow().clone();
            workflow.updated_at_millis += 1;
            fixture
                .store
                .replace(current.workflow(), &workflow)
                .expect("workflow-only drift");
        }
    }

    #[test]
    fn full_workflow_and_hps_drift_are_rejected_at_both_callbacks_even_on_error() {
        for selection in [false, true] {
            for credentials in [false, true] {
                for failure in [false, true] {
                    let fixture = RunPodFixture::new(true, WorkerLifetime::Persistent, true, true);
                    let before = fixture.current();
                    let observer = Observer {
                        result: Ok(Observation::Pending),
                        calls: Mutex::new(0),
                        network: true,
                    };
                    let result = runpod_with(
                        &fixture.store,
                        &fixture.profile,
                        &before.workspace().environment_summary(),
                        || {
                            if credentials {
                                drift(&fixture, selection);
                            }
                            if failure {
                                Err(ConfiguredStopConfirmationError::CredentialUnavailable)
                            } else {
                                key()
                            }
                        },
                        |_, allocation| {
                            let result = confirm_remote_workspace_stop(&fixture.store, &observer, allocation);
                            drift(&fixture, selection);
                            result
                        },
                    );
                    if failure && !credentials {
                        assert_eq!(result, Err(ConfiguredStopConfirmationError::CredentialUnavailable));
                        assert_eq!(fixture.current(), before);
                    } else {
                        assert_eq!(result, Err(ConfiguredStopConfirmationError::Stop(Error::StateChanged)));
                    }
                    assert_eq!(
                        *observer.calls.lock().expect("calls"),
                        usize::from(!credentials && !failure)
                    );
                }
            }
        }
    }

    #[test]
    fn missing_credential_retains_exact_intent_and_has_fixed_diagnostics() {
        let fixture = RunPodFixture::new(true, WorkerLifetime::Persistent, true, true);
        let before = fixture.current();
        let error = runpod_with(
            &fixture.store,
            &fixture.profile,
            &before.workspace().environment_summary(),
            || Err(ConfiguredStopConfirmationError::CredentialUnavailable),
            |_, _| panic!("no query"),
        )
        .expect_err("credential missing");
        assert_eq!(error, ConfiguredStopConfirmationError::CredentialUnavailable);
        assert!(!format!("{error:?} {error}").contains("synthetic-provider-credential"));
        assert_eq!(fixture.current(), before);
    }
}
