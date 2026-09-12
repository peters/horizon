use super::*;
use crate::remote_provider_config::RemoteProviderConfig;

#[test]
fn public_runpod_stop_refuses_unsupported_or_missing_named_profiles_without_writes() {
    let fixture = Fixture::new();
    let before = fixture.current();
    for provider in [CloudProvider::LocalDocker, CloudProvider::Azure, CloudProvider::RunPod] {
        let mut expected = before.workspace().environment_summary();
        expected.provider = provider;
        let result = stop_configured_runpod_environment(&fixture.store, &RemoteProviderConfig::default(), &expected);
        if cfg!(target_os = "linux") && provider == CloudProvider::RunPod {
            assert!(matches!(result, Err(ConfiguredRunPodStopError::Configuration(_))));
        } else {
            assert_eq!(result, Err(ConfiguredRunPodStopError::UnsupportedProvider));
        }
        assert_eq!(fixture.current(), before);
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::cloud_run::{
        interactive_worker::{InteractiveWorkerLifecycle, InteractiveWorkerSshEndpoint},
        runpod::{RunPodApiKey, RunPodNetworkVolumeExpectation, RunPodProfile},
    };
    use crate::remote_workspace::RemoteEnvironmentSummary;
    use crate::remote_workspace::stop::configured_runpod::stop_with;

    type Rejected = ConfiguredRunPodStopError;

    struct Retained {
        directory: tempfile::TempDir,
        store: CloudWorkflowStore,
        profile: RunPodProfile,
    }

    impl Retained {
        fn new(network: bool, pin: bool, timed: bool) -> Self {
            let directory = tempfile::tempdir().expect("private fixture");
            let store = CloudWorkflowStore::open_path(directory.path().join("control/store.sqlite3")).expect("store");
            let mut state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
                "version":1, "spec":{
                    "workspace_local_id":"workspace", "working_directory":".", "generation":0, "panels":[],
                    "target":{"provider":"run_pod", "profile":"development", "disk_gib":20,
                        "lifetime":"persistent", "image":format!("example/worker@sha256:{}", "a".repeat(64))},
                    "repository":{"repository":"example/project", "commit":"b".repeat(40)}
                }
            }))
            .expect("state");
            if timed {
                state.spec.target.lifetime = WorkerLifetime::TimeLimited { seconds: 900 };
            }
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
                    .expect("caller-supplied selection");
            }
            let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
            blob.extend([7; 32]);
            let key = format!("ssh-ed25519 {}", STANDARD.encode(blob));
            let reserved = store
                .reserve_remote_worker_request(&allocation, &key)
                .expect("public request");
            let request = reserved.worker_request().expect("request");
            let lifetime = if timed {
                InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
                    terminate_after: (time::OffsetDateTime::now_utc() + time::Duration::seconds(900))
                        .format(&time::format_description::well_known::Rfc3339)
                        .expect("lease"),
                })
            } else {
                InteractiveWorkerLifetime::Persistent
            };
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
                            lifetime,
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
                    }),
                )
                .expect("public retained metadata only");
            let profile = serde_json::from_value(serde_json::json!({
                "name":"development", "gpu_type_ids":["synthetic-gpu"], "gpu_count":1,
                "ports":["22/tcp"], "volume_gib":0, "data_center_id":"synthetic-dc"
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

        fn stop(&self, provider: &Provider) -> Result<RemoteEnvironmentSummary, Rejected> {
            stop_with(
                &self.store,
                &self.profile,
                &self.current().workspace().environment_summary(),
                key,
                |_, allocation| stop_allocation(&self.store, provider, allocation),
            )
        }

        fn drift(&self, selection: bool) {
            if selection {
                let mut connection = rusqlite::Connection::open(self.store.path()).expect("fixture writer");
                let transaction = connection.transaction().expect("transaction");
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
                transaction.commit().expect("atomic fixture mutation");
            } else {
                let current = self.current();
                let mut workflow = current.workflow().workflow().clone();
                workflow.updated_at_millis += 1;
                self.store
                    .replace(current.workflow(), &workflow)
                    .expect("workflow-only drift");
            }
        }
    }

    fn key() -> Result<RunPodApiKey, Rejected> {
        RunPodApiKey::new("synthetic-provider-credential").map_err(|_| Rejected::CredentialUnavailable)
    }

    fn provider(result: Result<InteractiveWorkerStop, &'static str>) -> Provider {
        let mut provider = Provider::new(result);
        provider.kind = CloudProvider::RunPod;
        provider
    }

    #[test]
    fn admission_requires_exact_persistent_worker_pin_profile_and_hps_before_credentials() {
        for fault in 0..10 {
            let mut fixture = Retained::new(!matches!(fault, 0 | 2), fault != 1, fault == 2);
            match fault {
                0 | 2 => fixture.profile.volume_gib = 10, // Ordinary disk cannot supply HPS or persistent admission.
                3 => fixture.profile.name = "foreign-profile".into(),
                4 => fixture.profile.data_center_id = Some("foreign-dc".into()),
                5 => fixture.profile.ports.clear(),
                6 => {
                    let current = fixture.current();
                    let mut next = current.workspace().state().clone();
                    next.runtime.as_mut().expect("runtime").cleanup = Some(RemoteCleanupIntent {
                        reason: RemoteCleanupReason::Cancelled,
                        requested_at_millis: 1,
                    });
                    fixture
                        .store
                        .replace_remote_workspace(current.workspace(), &next)
                        .expect("management fixture");
                }
                _ => {}
            }
            let before = fixture.current();
            let mut expected = before.workspace().environment_summary();
            match fault {
                7 => expected.panel_count += 1,
                8 => expected.owning_session_id = "00000000-0000-4000-8000-000000000002".into(),
                9 => expected.worker_identity.as_mut().expect("worker").resource_id = "foreign-worker".into(),
                _ => {}
            }
            assert!(
                stop_with(
                    &fixture.store,
                    &fixture.profile,
                    &expected,
                    || panic!("credentials must remain unread"),
                    |_, _| panic!("no provider work")
                )
                .is_err(),
                "fault {fault}"
            );
            assert_eq!(fixture.current(), before);
        }
    }

    #[test]
    fn missing_credentials_leave_no_stop_intent_or_private_identity() {
        let fixture = Retained::new(true, true, false);
        let before = fixture.current();
        let error = stop_with(
            &fixture.store,
            &fixture.profile,
            &before.workspace().environment_summary(),
            || Err(Rejected::CredentialUnavailable),
            |_, _| panic!("no dispatch"),
        )
        .expect_err("missing key");
        assert_eq!(error, Rejected::CredentialUnavailable);
        assert!(!format!("{error:?} {error}").contains("synthetic-provider-credential"));
        assert_eq!(fixture.current(), before);
        assert_eq!(
            fixture.directory.path().read_dir().expect("root").count(),
            1,
            "only control store, no identity"
        );
    }

    #[test]
    fn one_stop_records_intent_before_dispatch_and_preserves_every_other_field() {
        let fixture = Retained::new(true, true, false);
        let before = fixture.current();
        let store = fixture.store.clone();
        let mut provider = provider(Ok(InteractiveWorkerStop::Stopped));
        provider.on_stop = Some(Box::new(move |worker| {
            let current = store
                .load_remote_allocation(OWNER, "workspace")
                .expect("read")
                .expect("allocation");
            let runtime = current.workspace().state().runtime.as_ref().expect("runtime");
            assert!(matches!(runtime.phase, RemoteRuntimePhase::Stopping { .. }));
            assert_eq!(runtime.worker.as_ref(), Some(worker));
        }));
        let result = fixture.stop(&provider).expect("verified fake Stop");
        let after = fixture.current();
        assert_eq!(result, after.workspace().environment_summary());
        let mut expected = before.workspace().state().clone();
        expected.runtime.as_mut().expect("runtime").phase =
            after.workspace().state().runtime.as_ref().expect("runtime").phase;
        assert!(
            matches!(result.saved_phase, Some(RemoteRuntimePhase::Stopped { requested_at_millis, observed_at_millis })
            if requested_at_millis > 0 && observed_at_millis >= requested_at_millis)
        );
        assert_eq!(after.workspace().state(), &expected);
        assert_eq!(after.workflow(), before.workflow());
        assert_eq!(result.revision, before.workspace().revision() + 2);
        assert_eq!(provider.counts(), [0, 0, 0, 0, 1]);
        assert_eq!(fixture.stop(&provider), Err(Rejected::ExistingStopIntent));
        assert_eq!(fixture.current(), after);
        assert_eq!(provider.counts(), [0, 0, 0, 0, 1]);
    }

    #[test]
    fn uncertainty_or_absence_keeps_original_intent_and_never_authorizes_another_stop() {
        for result in [Err("private-provider-marker"), Ok(InteractiveWorkerStop::AlreadyAbsent)] {
            let fixture = Retained::new(true, true, false);
            let before = fixture.current();
            let provider = provider(result);
            let error = fixture.stop(&provider).expect_err("unverified");
            assert_eq!(
                error,
                Rejected::Stop(if result.is_err() {
                    Error::ProviderUnavailable
                } else {
                    Error::ResourceAbsent
                })
            );
            assert!(!format!("{error:?} {error}").contains("private-provider-marker"));
            let retained = fixture.current();
            assert!(matches!(
                retained.workspace().state().runtime.as_ref().expect("runtime").phase,
                RemoteRuntimePhase::Stopping { .. }
            ));
            let mut expected = before.workspace().state().clone();
            expected.runtime.as_mut().expect("runtime").phase =
                retained.workspace().state().runtime.as_ref().expect("runtime").phase;
            assert_eq!(retained.workspace().state(), &expected);
            assert_eq!(retained.workflow(), before.workflow());
            let reopened =
                CloudWorkflowStore::open_existing_without_migration_path(fixture.store.path()).expect("fresh client");
            assert_eq!(
                stop_with(
                    &reopened,
                    &fixture.profile,
                    &retained.workspace().environment_summary(),
                    || panic!("existing intent precedes credential lookup"),
                    |_, _| panic!("never replay")
                ),
                Err(Rejected::ExistingStopIntent)
            );
            assert_eq!(fixture.current(), retained);
            assert_eq!(provider.counts(), [0, 0, 0, 0, 1]);
        }
    }

    #[test]
    fn workflow_and_selection_drift_during_credential_lookup_refuse_dispatch_even_on_error() {
        for selection in [false, true] {
            for error in [false, true] {
                let fixture = Retained::new(true, true, false);
                let before = fixture.current();
                let result = stop_with(
                    &fixture.store,
                    &fixture.profile,
                    &before.workspace().environment_summary(),
                    || {
                        fixture.drift(selection);
                        if error {
                            Err(Rejected::CredentialUnavailable)
                        } else {
                            key()
                        }
                    },
                    |_, _| panic!("no dispatch after snapshot drift"),
                );
                assert_eq!(result, Err(Rejected::Stop(Error::StateChanged)));
                assert_eq!(fixture.current().workspace(), before.workspace());
            }
        }
    }

    #[test]
    fn exact_allocation_cas_does_not_reload_a_new_workflow_before_stop() {
        let fixture = Retained::new(true, true, false);
        let before = fixture.current();
        let provider = provider(Ok(InteractiveWorkerStop::Stopped));
        let result = stop_with(
            &fixture.store,
            &fixture.profile,
            &before.workspace().environment_summary(),
            key,
            |_, exact| {
                fixture.drift(false);
                stop_allocation(&fixture.store, &provider, exact)
            },
        );
        assert_eq!(result, Err(Rejected::Stop(Error::StateChanged)));
        assert_eq!(fixture.current().workspace(), before.workspace());
        assert_eq!(provider.counts(), [0; 5]);
    }

    #[test]
    fn post_dispatch_workflow_and_selection_drift_cannot_be_reported_as_success() {
        for selection in [false, true] {
            let fixture = Retained::new(true, true, false);
            let provider = provider(Ok(InteractiveWorkerStop::Stopped));
            let result = stop_with(
                &fixture.store,
                &fixture.profile,
                &fixture.current().workspace().environment_summary(),
                key,
                |_, exact| {
                    let stopped = stop_allocation(&fixture.store, &provider, exact)?;
                    fixture.drift(selection);
                    Ok(stopped)
                },
            );
            assert_eq!(result, Err(Rejected::Stop(Error::StateChanged)));
            assert_eq!(provider.counts(), [0, 0, 0, 0, 1]);
            assert!(
                fixture
                    .current()
                    .workspace()
                    .state()
                    .runtime
                    .as_ref()
                    .expect("runtime")
                    .phase
                    .stop_requested_at_millis()
                    .is_some()
            );
        }
    }
}
