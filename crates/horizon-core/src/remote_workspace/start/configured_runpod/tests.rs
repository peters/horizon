use super::*;

#[cfg(not(target_os = "linux"))]
#[test]
fn non_linux_start_is_explicitly_unsupported_without_storage_access() {
    let _entry = start_configured_runpod_environment;
    assert!(
        ConfiguredRunPodStartError::UnsupportedProvider
            .to_string()
            .contains("Linux")
    );
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::{
        cloud_run::{
            StoredRemoteAllocation,
            interactive_worker::{
                InteractiveWorkerIdentity, InteractiveWorkerLifecycle, InteractiveWorkerLifetime,
                InteractiveWorkerSshEndpoint,
            },
            runpod::RunPodNetworkVolumeExpectation,
        },
        remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason, RemoteWorkspaceState},
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use std::sync::atomic::AtomicUsize;

    const OWNER: &str = "00000000-0000-4000-8000-000000000001";
    type Error = ConfiguredRunPodStartError;

    struct Fixture {
        _directory: tempfile::TempDir,
        store: CloudWorkflowStore,
        profile: RunPodProfile,
    }

    impl Fixture {
        fn new(network: bool, pin: bool) -> Self {
            let directory = tempfile::tempdir().expect("private fixture");
            let store = CloudWorkflowStore::open_path(directory.path().join("control/store.sqlite3")).expect("store");
            let state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
                "version":1, "spec":{
                    "workspace_local_id":"workspace", "working_directory":".", "generation":0, "panels":[],
                    "target":{"provider":"run_pod", "profile":"development", "disk_gib":20,
                        "lifetime":"persistent", "image":format!("example/worker@sha256:{}", "a".repeat(64))},
                    "repository":{"repository":"example/project", "commit":"b".repeat(40)}
                }
            }))
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
                    .expect("selection");
            }
            let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
            blob.extend([7; 32]);
            let key = format!("ssh-ed25519 {}", STANDARD.encode(blob));
            let reserved = store.reserve_remote_worker_request(&allocation, &key).expect("request");
            let request = reserved.worker_request().expect("public request");
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
            let profile = serde_json::from_value(serde_json::json!({
                "name":"development", "gpu_type_ids":["synthetic-gpu"], "gpu_count":1,
                "ports":["22/tcp"], "volume_gib":10, "data_center_id":"synthetic-dc"
            }))
            .expect("profile");
            Self {
                _directory: directory,
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

        fn stopped(&self) {
            let intent = self
                .store
                .record_remote_stop_phase(&self.current(), RemoteRuntimePhase::Stopping { requested_at_millis: 1 })
                .expect("Stop intent");
            self.store
                .record_remote_stop_phase(
                    &intent,
                    RemoteRuntimePhase::Stopped {
                        requested_at_millis: 1,
                        observed_at_millis: 2,
                    },
                )
                .expect("Stopped");
        }

        fn phase(&self) -> RemoteRuntimePhase {
            self.current()
                .workspace()
                .state()
                .runtime
                .as_ref()
                .expect("runtime")
                .phase
        }

        fn start(&self, provider: &Starter) -> Result<ConfiguredStart, Error> {
            start_with(
                &self.store,
                &self.profile,
                &self.current().workspace().environment_summary(),
                |_| Ok(provider),
            )
        }

        fn drift(&self, binding: bool) {
            if binding {
                // Inject corruption atomically, then restore the exact required schema.
                let mut db = rusqlite::Connection::open(self.store.path()).expect("fixture writer");
                let tx = db.transaction().expect("transaction");
                let trigger: String = tx
                    .query_row(
                        "SELECT sql FROM sqlite_schema WHERE name='remote_network_volume_selections_no_update'",
                        [],
                        |r| r.get(0),
                    )
                    .expect("trigger");
                tx.execute_batch("DROP TRIGGER remote_network_volume_selections_no_update")
                    .expect("fault seam");
                tx.execute("UPDATE remote_network_volume_selections SET volume_id='changed-volume' WHERE workspace_local_id='workspace'", []).expect("drift");
                tx.execute_batch(&trigger).expect("restore schema");
                tx.commit().expect("commit fixture");
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

    type Script<'a> =
        Box<dyn Fn(&InteractiveWorker) -> Result<InteractiveWorkerStart, &'static str> + Send + Sync + 'a>;
    struct Starter<'a> {
        script: Script<'a>,
        calls: AtomicUsize,
    }
    impl<'a> Starter<'a> {
        fn new(
            script: impl Fn(&InteractiveWorker) -> Result<InteractiveWorkerStart, &'static str> + Send + Sync + 'a,
        ) -> Self {
            Self {
                script: Box::new(script),
                calls: AtomicUsize::new(0),
            }
        }
    }
    impl InteractiveWorkerProvider for &Starter<'_> {
        type Error = std::io::Error;
        fn provider(&self) -> CloudProvider {
            CloudProvider::RunPod
        }
        fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
            panic!("no create")
        }
        fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
            panic!("no inspect")
        }
        fn reconcile_worker(
            &self,
            _: &InteractiveWorkerRequest,
        ) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
            panic!("no reconcile")
        }
        fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
            panic!("no delete")
        }
    }
    impl InteractiveWorkerStartProvider for &Starter<'_> {
        fn start_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerStart, Self::Error> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            (self.script)(worker).map_err(std::io::Error::other)
        }
    }
    fn answer(worker: &InteractiveWorker, running: bool) -> InteractiveWorkerStart {
        let status = InteractiveWorkerStatus {
            worker: worker.clone(),
            lifecycle: InteractiveWorkerLifecycle::Provisioning,
            ssh: None,
        };
        if running {
            InteractiveWorkerStart::AlreadyRunning(status)
        } else {
            InteractiveWorkerStart::Started(status)
        }
    }

    #[test]
    fn public_entry_refuses_other_providers_and_missing_profile_without_writes() {
        let fixture = Fixture::new(true, true);
        fixture.stopped();
        let before = fixture.current();
        for kind in [CloudProvider::LocalDocker, CloudProvider::Azure, CloudProvider::RunPod] {
            let mut expected = before.workspace().environment_summary();
            expected.provider = kind;
            let result =
                start_configured_runpod_environment(&fixture.store, &RemoteProviderConfig::default(), &expected);
            if kind == CloudProvider::RunPod {
                assert!(matches!(result, Err(Error::Configuration(_))));
            } else {
                assert_eq!(result, Err(Error::UnsupportedProvider));
            }
            assert_eq!(fixture.current(), before);
        }
    }

    #[test]
    fn admission_precedes_credentials_for_missing_hps_pin_profile_phase_or_identity() {
        for fault in 0..12 {
            let mut fixture = Fixture::new(fault != 0, fault != 1);
            if !matches!(fault, 2 | 6 | 11) {
                fixture.stopped();
            }
            match fault {
                3 => fixture.profile.name = "foreign-profile".into(),
                4 => fixture.profile.data_center_id = Some("foreign-dc".into()),
                5 => fixture.profile.ports.clear(),
                6 => {
                    let current = fixture.current();
                    let mut state = current.workspace().state().clone();
                    state.runtime.as_mut().expect("runtime").cleanup = Some(RemoteCleanupIntent {
                        reason: RemoteCleanupReason::Cancelled,
                        requested_at_millis: 1,
                    });
                    fixture
                        .store
                        .replace_remote_workspace(current.workspace(), &state)
                        .expect("management intent");
                }
                11 => {
                    fixture
                        .store
                        .record_remote_stop_phase(
                            &fixture.current(),
                            RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
                        )
                        .expect("pending Stop");
                }
                _ => {}
            }
            let before = fixture.current();
            let mut expected = before.workspace().environment_summary();
            match fault {
                7 => expected.revision += 1,
                8 => expected.owning_session_id = "00000000-0000-4000-8000-000000000002".into(),
                9 => expected.worker_identity.as_mut().expect("identity").resource_id = "foreign-worker".into(),
                10 => expected.lifetime = crate::cloud_run::WorkerLifetime::TimeLimited { seconds: 900 },
                _ => {}
            }
            let result = start_with(
                &fixture.store,
                &fixture.profile,
                &expected,
                |_| -> Result<&Starter<'_>, Error> { panic!("credential accessed for fault {fault}") },
            );
            assert!(result.is_err(), "fault {fault}");
            assert_eq!(fixture.current(), before);
        }
    }

    #[test]
    fn actual_intent_precedes_one_start_and_retries_preserve_request_time() {
        for retry in [false, true] {
            for running in [false, true] {
                let fixture = Fixture::new(true, true);
                fixture.stopped();
                if retry {
                    fixture
                        .store
                        .record_remote_start_phase(
                            &fixture.current(),
                            RemoteRuntimePhase::Starting { requested_at_millis: 3 },
                        )
                        .expect("prior intent");
                }
                let before = fixture.current();
                let fake = Starter::new(|worker| {
                    let RemoteRuntimePhase::Starting { requested_at_millis } = fixture.phase() else {
                        panic!("intent missing")
                    };
                    assert!(requested_at_millis >= 3);
                    if retry {
                        assert_eq!(requested_at_millis, 3);
                    }
                    assert_eq!(
                        before
                            .workspace()
                            .state()
                            .runtime
                            .as_ref()
                            .expect("runtime")
                            .worker
                            .as_ref(),
                        Some(worker)
                    );
                    Ok(answer(worker, running))
                });
                let result = fixture.start(&fake).expect("verified Start");
                assert_eq!(
                    result.saved.revision,
                    before.workspace().revision() + if retry { 1 } else { 2 }
                );
                assert_eq!(result.saved.saved_phase, Some(RemoteRuntimePhase::Reconciling));
                assert_eq!(result.already_running, running);
                assert_eq!(
                    fixture
                        .current()
                        .workspace()
                        .state()
                        .runtime
                        .as_ref()
                        .expect("runtime")
                        .worker,
                    before.workspace().state().runtime.as_ref().expect("runtime").worker
                );
                assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
                assert_eq!(
                    fixture.start(&fake),
                    Err(Error::Start(RemoteWorkspaceStartError::NotStopped))
                );
                assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
            }
        }
    }

    #[test]
    fn errors_absence_foreign_worker_and_endpoint_keep_intent_and_redact_provider_payload() {
        for fault in 0..4 {
            let fixture = Fixture::new(true, true);
            fixture.stopped();
            let before = fixture.current();
            let fake = Starter::new(|worker| match fault {
                0 => Err("private-provider-marker"),
                1 => Ok(InteractiveWorkerStart::AlreadyAbsent),
                _ => {
                    let InteractiveWorkerStart::Started(mut status) = answer(worker, false) else {
                        unreachable!()
                    };
                    if fault == 2 {
                        status.worker.identity.resource_id = "foreign-worker".into();
                    } else {
                        let mut pin = before
                            .workspace()
                            .state()
                            .runtime
                            .as_ref()
                            .expect("runtime")
                            .ssh
                            .clone()
                            .expect("pin");
                        pin.port += 1;
                        status.ssh = Some(pin);
                    }
                    Ok(InteractiveWorkerStart::Started(status))
                }
            });
            let error = fixture.start(&fake).expect_err("unconfirmed");
            assert!(!format!("{error:?} {error}").contains("private-provider-marker"));
            assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
            assert!(matches!(fixture.phase(), RemoteRuntimePhase::Starting { .. }));
            assert_eq!(
                fixture
                    .current()
                    .workspace()
                    .state()
                    .runtime
                    .as_ref()
                    .expect("runtime")
                    .worker,
                before.workspace().state().runtime.as_ref().expect("runtime").worker
            );
        }
    }

    #[test]
    fn credentials_and_drift_before_or_during_dispatch_never_false_complete() {
        for during in [false, true] {
            for binding in [false, true] {
                let fixture = Fixture::new(true, true);
                fixture.stopped();
                let fake = Starter::new(|worker| {
                    fixture.drift(binding);
                    Ok(answer(worker, false))
                });
                let expected = fixture.current().workspace().environment_summary();
                let result = start_with(&fixture.store, &fixture.profile, &expected, |_| {
                    if !during {
                        fixture.drift(binding);
                    }
                    Ok(&fake)
                });
                assert_eq!(result, Err(Error::Start(RemoteWorkspaceStartError::StateChanged)));
                assert_eq!(fake.calls.load(Ordering::SeqCst), usize::from(during));
                assert!(if during {
                    matches!(fixture.phase(), RemoteRuntimePhase::Starting { .. })
                } else {
                    matches!(fixture.phase(), RemoteRuntimePhase::Stopped { .. })
                });
            }
        }
        let fixture = Fixture::new(true, true);
        fixture.stopped();
        let before = fixture.current();
        let error = start_with(
            &fixture.store,
            &fixture.profile,
            &before.workspace().environment_summary(),
            |_| -> Result<&Starter<'_>, Error> { Err(Error::CredentialUnavailable) },
        );
        assert_eq!(error, Err(Error::CredentialUnavailable));
        assert_eq!(fixture.current(), before);
    }
}
