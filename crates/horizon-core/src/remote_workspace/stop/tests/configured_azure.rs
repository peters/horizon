//! Azure saved-Stop admission on real stores with a fake observer: the real ordering,
//! coordinator and drift checks run without the Azure CLI or ARM.
use super::*;
use crate::cloud_run::{
    azure::{AzureDiskSku, AzureProfile, resource_group_name},
    interactive_worker::{InteractiveWorkerLifecycle, InteractiveWorkerSshEndpoint},
    interactive_worker_stop::{
        InteractiveWorkerStopExpectation, InteractiveWorkerStopObservation as Observation,
        InteractiveWorkerStopObserver,
    },
};
use crate::remote_workspace::{
    RemoteEnvironmentSummary,
    stop::configured_azure::{RetainedAzure, azure_with},
};

const SUBSCRIPTION: &str = "11111111-1111-4111-8111-111111111111";

fn azure_profile() -> AzureProfile {
    AzureProfile {
        name: "cpu".into(),
        subscription_id: SUBSCRIPTION.into(),
        location: "northeurope".into(),
        vm_size: "Standard_D4s_v3".into(),
        image_pull_identity_id: format!(
            "/subscriptions/{SUBSCRIPTION}/resourceGroups/synthetic/providers/Microsoft.ManagedIdentity/userAssignedIdentities/pull"
        ),
        declared_hourly_cost_micros: 100_000,
        registry_login_server: "synthetic.azurecr.io".into(),
        disk_sku: AzureDiskSku::StandardSsdLrs,
    }
}

struct AzureFixture {
    directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    profile: AzureProfile,
}

struct Shape {
    lifetime: WorkerLifetime,
    binding: bool,
    intent: bool,
    pin: bool,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            lifetime: WorkerLifetime::Persistent,
            binding: true,
            intent: true,
            pin: true,
        }
    }
}

impl AzureFixture {
    fn new(shape: &Shape) -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        let store = CloudWorkflowStore::open_path(directory.path().join("control/store.sqlite3")).expect("store");
        let profile = azure_profile();
        let mut state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version":1, "spec":{
                "workspace_local_id":"workspace", "working_directory":".", "generation":0, "panels":[],
                "target":{"provider":"azure", "profile":"cpu", "disk_gib":20, "lifetime":"persistent",
                    "image":format!("synthetic.azurecr.io/worker@sha256:{}", "a".repeat(64)),
                    "max_hourly_cost_micros":200_000},
                "repository":{"repository":"example/project", "commit":"b".repeat(40)}
            }
        }))
        .expect("state");
        state.spec.target.lifetime = shape.lifetime;
        let saved = store.create_remote_workspace(OWNER, &state).expect("workspace");
        let allocation = store.allocate_remote_runtime(&saved, i64::MAX).expect("allocation");
        if shape.binding {
            store
                .record_remote_cpu_profile_binding(&allocation, &profile)
                .expect("immutable binding before any key or provider work");
        }
        let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
        blob.extend([7; 32]);
        let key = format!("ssh-ed25519 {}", STANDARD.encode(blob));
        let reserved = store
            .reserve_remote_worker_request(&allocation, &key)
            .expect("public request only");
        let request = reserved.worker_request().expect("request");
        let lifetime = if shape.lifetime == WorkerLifetime::Persistent {
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
                    provider: CloudProvider::Azure,
                    workflow_id: request.workflow_id,
                    job_id: request.job_id,
                    resource_id: format!(
                        "/subscriptions/{SUBSCRIPTION}/resourceGroups/{}",
                        resource_group_name(request.workflow_id, request.job_id)
                    ),
                },
                target: request.target,
                ssh_public_key: request.ssh_public_key,
                lifetime,
            },
            lifecycle: if shape.pin {
                InteractiveWorkerLifecycle::Ready
            } else {
                InteractiveWorkerLifecycle::Provisioning
            },
            ssh: shape.pin.then_some(InteractiveWorkerSshEndpoint {
                host: "203.0.113.9".into(),
                port: 2222,
                username: "root".into(),
                host_key: key,
            }),
        };
        let retained = store
            .record_remote_worker_recovery(&reserved, Some(&status))
            .expect("synthetic retained public observation");
        if shape.intent {
            store
                .record_remote_stop_phase(&retained, RemoteRuntimePhase::Stopping { requested_at_millis: 1 })
                .expect("existing intent, no provider Stop");
        }
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
        azure_with(
            &self.store,
            &self.profile,
            &self.current().workspace().environment_summary(),
            |_| Ok(observer),
            |observer, allocation| confirm_remote_workspace_stop(&self.store, observer, allocation),
        )
    }
}

struct Observer {
    result: Result<Observation, &'static str>,
    calls: Mutex<usize>,
}

impl InteractiveWorkerProvider for &Observer {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        CloudProvider::Azure
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("no create")
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("no generic inspection")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("no setup recovery")
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("no delete")
    }
}

impl InteractiveWorkerStopObserver for &Observer {
    fn observe_worker_stop(&self, expected: InteractiveWorkerStopExpectation<'_>) -> Result<Observation, Self::Error> {
        assert!(expected.network_volume.is_none(), "no RunPod expectation reaches Azure");
        assert!(expected.ssh.is_complete());
        assert!(expected.worker.is_valid_for(CloudProvider::Azure));
        *self.calls.lock().expect("calls") += 1;
        self.result.map_err(std::io::Error::other)
    }
}

#[test]
fn typed_observations_use_actual_coordinator_without_private_key_or_replay() {
    let fixture = AzureFixture::new(&Shape::default());
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
                assert_eq!(fixture.current(), before, "pending and absence change nothing");
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

#[test]
fn admission_refusals_precede_client_construction_and_provider_calls() {
    let refusals: [(&str, Shape); 4] = [
        (
            "no intent",
            Shape {
                intent: false,
                ..Shape::default()
            },
        ),
        (
            // A timed target cannot carry a binding at all; its lifetime is refused first.
            "timed worker",
            Shape {
                lifetime: WorkerLifetime::TimeLimited { seconds: 900 },
                binding: false,
                ..Shape::default()
            },
        ),
        (
            "no pin",
            Shape {
                pin: false,
                ..Shape::default()
            },
        ),
        (
            "no binding",
            Shape {
                binding: false,
                ..Shape::default()
            },
        ),
    ];
    let expected_errors = [
        ConfiguredStopConfirmationError::Stop(Error::MissingStopIntent),
        ConfiguredStopConfirmationError::Stop(Error::UnsupportedLifetime),
        ConfiguredStopConfirmationError::Stop(Error::MissingTrust),
        ConfiguredStopConfirmationError::InvalidBinding,
    ];
    for ((label, shape), expected_error) in refusals.iter().zip(expected_errors) {
        let fixture = AzureFixture::new(shape);
        assert_eq!(refused(&fixture, &fixture.profile, None), expected_error, "{label}");
    }
}

/// Faults injected after a valid fixture: management intent, a future request, the
/// named profile drifting away from the immutable binding, a wrong profile name, a
/// stale summary and a foreign `RunPod` storage selection.
#[test]
fn injected_faults_are_refused_before_client_construction_and_provider_calls() {
    // Management intent cannot be added over Stop intent, so it is modelled before any
    // intent exists; admission refuses it before it looks for intent.
    let fixture = AzureFixture::new(&Shape {
        intent: false,
        ..Shape::default()
    });
    let current = fixture.current();
    let mut state = current.workspace().state().clone();
    state.runtime.as_mut().expect("runtime").cleanup = Some(RemoteCleanupIntent {
        reason: RemoteCleanupReason::Cancelled,
        requested_at_millis: 1,
    });
    fixture
        .store
        .replace_remote_workspace(current.workspace(), &state)
        .expect("valid refusal fixture");
    assert_eq!(
        refused(&fixture, &fixture.profile, None),
        ConfiguredStopConfirmationError::Stop(Error::ManagementConflict)
    );

    let fixture = AzureFixture::new(&Shape {
        intent: false,
        ..Shape::default()
    });
    fixture
        .store
        .record_remote_stop_phase(
            &fixture.current(),
            RemoteRuntimePhase::Stopping {
                requested_at_millis: i64::MAX,
            },
        )
        .expect("future intent");
    assert_eq!(
        refused(&fixture, &fixture.profile, None),
        ConfiguredStopConfirmationError::Stop(Error::InvalidTimestamp)
    );

    let fixture = AzureFixture::new(&Shape::default());
    let mut drifted = fixture.profile.clone();
    drifted.vm_size = "Standard_D2s_v3".into();
    assert_eq!(
        refused(&fixture, &drifted, None),
        ConfiguredStopConfirmationError::InvalidBinding,
        "the named profile no longer matches the binding the worker was created under"
    );
    let mut other_subscription = fixture.profile.clone();
    other_subscription.subscription_id = "22222222-2222-4222-8222-222222222222".into();
    other_subscription.image_pull_identity_id = other_subscription
        .image_pull_identity_id
        .replace(SUBSCRIPTION, "22222222-2222-4222-8222-222222222222");
    assert_eq!(
        refused(&fixture, &other_subscription, None),
        ConfiguredStopConfirmationError::InvalidBinding
    );
    let mut renamed = fixture.profile.clone();
    renamed.name = "other".into();
    assert_eq!(
        refused(&fixture, &renamed, None),
        ConfiguredStopConfirmationError::InvalidBinding
    );
    let mut summary = fixture.current().workspace().environment_summary();
    summary.panel_count += 1;
    assert_eq!(
        refused(&fixture, &fixture.profile, Some(summary)),
        ConfiguredStopConfirmationError::Stop(Error::StateChanged)
    );
    // The store itself reports a RunPod storage row under an Azure allocation as corrupt
    // storage, so a foreign selection never reaches admission's own check or the observer.
    foreign_selection(&fixture);
    assert_eq!(
        refused(&fixture, &fixture.profile, None),
        ConfiguredStopConfirmationError::Stop(Error::StorageUnavailable),
        "a RunPod storage expectation is never an Azure binding"
    );
}

fn refused(
    fixture: &AzureFixture,
    profile: &AzureProfile,
    summary: Option<RemoteEnvironmentSummary>,
) -> ConfiguredStopConfirmationError {
    let before = fixture.current();
    let expected = summary.unwrap_or_else(|| before.workspace().environment_summary());
    let error = azure_with(
        &fixture.store,
        profile,
        &expected,
        |_: &RetainedAzure| -> Result<&Observer, ConfiguredStopConfirmationError> {
            panic!("must not construct the client")
        },
        |_, _| panic!("must not query provider"),
    )
    .expect_err("refused");
    assert_eq!(fixture.current(), before);
    error
}

/// A `RunPod` storage selection row written against the Azure allocation, as a foreign
/// or corrupt store might carry; the public API refuses to record one for Azure.
fn foreign_selection(fixture: &AzureFixture) {
    let current = fixture.current();
    let runtime = current.workspace().state().runtime.as_ref().expect("runtime");
    let connection = rusqlite::Connection::open(fixture.store.path()).expect("fixture database");
    connection
        .execute(
            "INSERT INTO remote_network_volume_selections
             (workspace_local_id, session_id, generation, workflow_id, job_id, version,
              volume_id, data_center_id, minimum_size_gb, storage_type)
             VALUES ('workspace', ?1, ?2, ?3, ?4, 1, 'foreign-volume', 'foreign-dc', 10, 'HIGH_PERFORMANCE')",
            rusqlite::params![
                OWNER,
                i64::try_from(runtime.generation).expect("generation"),
                runtime.workflow_id.to_string(),
                runtime.job_id.to_string()
            ],
        )
        .expect("fixture only");
}

fn drift(fixture: &AzureFixture) {
    let current = fixture.current();
    let mut workflow = current.workflow().workflow().clone();
    workflow.updated_at_millis += 1;
    fixture
        .store
        .replace(current.workflow(), &workflow)
        .expect("workflow-only drift");
}

#[test]
fn full_allocation_drift_is_rejected_at_both_callbacks_even_on_error() {
    for at_client in [false, true] {
        for failure in [false, true] {
            let fixture = AzureFixture::new(&Shape::default());
            let before = fixture.current();
            let observer = Observer {
                result: Ok(Observation::Pending),
                calls: Mutex::new(0),
            };
            let result = azure_with(
                &fixture.store,
                &fixture.profile,
                &before.workspace().environment_summary(),
                |_| {
                    if at_client {
                        drift(&fixture);
                    }
                    if failure {
                        Err(ConfiguredStopConfirmationError::InvalidBinding)
                    } else {
                        Ok(&observer)
                    }
                },
                |observer, allocation| {
                    let result = confirm_remote_workspace_stop(&fixture.store, observer, allocation);
                    drift(&fixture);
                    result
                },
            );
            if failure && !at_client {
                assert_eq!(result, Err(ConfiguredStopConfirmationError::InvalidBinding));
                assert_eq!(fixture.current(), before);
            } else {
                assert_eq!(result, Err(ConfiguredStopConfirmationError::Stop(Error::StateChanged)));
            }
            assert_eq!(
                *observer.calls.lock().expect("calls"),
                usize::from(!at_client && !failure)
            );
        }
    }
}

#[test]
fn the_production_client_is_lazy_and_bound_to_the_named_profile() {
    let fixture = AzureFixture::new(&Shape::default());
    let admitted = RetainedAzure::load(
        &fixture.store,
        &fixture.profile,
        &fixture.current().workspace().environment_summary(),
    )
    .expect("admitted");
    // Constructing the credential and client performs no CLI run, request or write.
    let before = fixture.current();
    let client = RetainedAzure::client(&fixture.store, &fixture.profile).expect("client");
    drop(admitted);
    assert_eq!(client.provider(), CloudProvider::Azure);
    assert_eq!(fixture.current(), before);
    assert_eq!(
        fixture.directory.path().read_dir().expect("fixture root").count(),
        1,
        "no token cache, key or file beside the store"
    );
}

#[test]
fn public_check_reaches_azure_admission_by_named_profile_without_the_cli() {
    use crate::remote_provider_config::RemoteProviderConfig;
    // A configured profile with no saved intent is refused by admission, so the public
    // path proves its dispatch without ever reaching the Azure CLI.
    let fixture = AzureFixture::new(&Shape {
        intent: false,
        ..Shape::default()
    });
    let before = fixture.current();
    let config = RemoteProviderConfig {
        azure: vec![fixture.profile.clone()],
        ..Default::default()
    };
    assert_eq!(
        confirm_configured_remote_environment_stop(&fixture.store, &config, &before.workspace().environment_summary()),
        Err(ConfiguredStopConfirmationError::Stop(Error::MissingStopIntent))
    );
    assert_eq!(fixture.current(), before);
    let mut foreign = before.workspace().environment_summary();
    foreign.provider = CloudProvider::LocalDocker;
    assert_eq!(
        confirm_configured_remote_environment_stop(&fixture.store, &config, &foreign),
        Err(ConfiguredStopConfirmationError::UnsupportedProvider)
    );
}
