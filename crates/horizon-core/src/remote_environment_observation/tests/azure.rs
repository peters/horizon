//! Azure overview checks on real stores with a fake bound provider: the real ordering
//! and drift fences run without the Azure CLI or ARM.
use super::*;
use crate::cloud_run::azure::AzureContainerRuntime;
use crate::{
    cloud_run::azure::{AzureDiskSku, AzureProfile, resource_group_name},
    remote_environment_observation::configured::azure_with,
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_workspace::{RemoteRuntimePhase, stop::configured_azure::Bound},
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
        container_runtime: AzureContainerRuntime::Default,
    }
}

struct Shape {
    lifetime: WorkerLifetime,
    binding: bool,
    pin: bool,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            lifetime: WorkerLifetime::Persistent,
            binding: true,
            pin: true,
        }
    }
}

struct AzureFixture {
    directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    profile: AzureProfile,
    allocation: StoredRemoteAllocation,
}

impl AzureFixture {
    fn new(shape: &Shape) -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        let store = CloudWorkflowStore::open_path(directory.path().join("control/store.sqlite3")).expect("store");
        let profile = azure_profile();
        let mut state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version":1, "spec":{
                "workspace_local_id":"workspace", "working_directory":".", "generation":0,
                "target":{"provider":"azure", "profile":"cpu", "disk_gib":20, "lifetime":"persistent",
                    "image":format!("synthetic.azurecr.io/worker@sha256:{}", "a".repeat(64)),
                    "max_hourly_cost_micros":200_000},
                "repository":{"repository":"example/project", "commit":"b".repeat(40)},
                "panels":[{"panel_local_id":"terminal", "kind":"command",
                    "command":{"program":"printf", "args":["private-task-marker"]}}]
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
        // Synthetic public identity only: an overview check must not open a private key.
        let reserved = store
            .reserve_remote_worker_request(&allocation, &public_key(1))
            .expect("public request only");
        let request = reserved.worker_request().expect("request");
        let lifetime = match shape.lifetime {
            WorkerLifetime::Persistent => InteractiveWorkerLifetime::Persistent,
            WorkerLifetime::TimeLimited { seconds } => InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
                terminate_after: (time::OffsetDateTime::now_utc() + time::Duration::seconds(i64::from(seconds)))
                    .format(&time::format_description::well_known::Rfc3339)
                    .expect("deadline"),
            }),
        };
        let group = resource_group_name(request.workflow_id, request.job_id);
        let status = InteractiveWorkerStatus {
            worker: InteractiveWorker {
                identity: InteractiveWorkerIdentity {
                    provider: CloudProvider::Azure,
                    workflow_id: request.workflow_id,
                    job_id: request.job_id,
                    resource_id: format!("/subscriptions/{SUBSCRIPTION}/resourceGroups/{group}"),
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
                host_key: public_key(7),
            }),
        };
        let allocation = store
            .record_remote_worker_recovery(&reserved, Some(&status))
            .expect("synthetic retained public observation");
        Self {
            directory,
            store,
            profile,
            allocation,
        }
    }

    fn current(&self) -> StoredRemoteAllocation {
        self.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation")
    }

    fn retained_status(&self, lifecycle: InteractiveWorkerLifecycle) -> InteractiveWorkerStatus {
        let runtime = self.allocation.workspace().state().runtime.as_ref().expect("runtime");
        InteractiveWorkerStatus {
            worker: runtime.worker.clone().expect("worker"),
            lifecycle,
            ssh: (lifecycle == InteractiveWorkerLifecycle::Ready)
                .then(|| runtime.ssh.clone())
                .flatten(),
        }
    }

    fn counts(&self) -> [i64; 3] {
        rusqlite::Connection::open(self.store.path())
            .expect("database")
            .query_row(
                "SELECT (SELECT COUNT(*) FROM cloud_workflows), (SELECT COUNT(*) FROM remote_runtime_allocations),
             (SELECT COUNT(*) FROM cloud_worker_creation_claims)",
                [],
                |row| Ok([row.get(0)?, row.get(1)?, row.get(2)?]),
            )
            .expect("counts")
    }
}

/// One admission case: the record shape, an optional profile edit and the refusal
/// expected before any client exists.
type Case = (Shape, Option<fn(&mut AzureProfile)>, ConfiguredObservationError);

fn azure_provider(status: Option<InteractiveWorkerStatus>) -> Provider {
    let mut provider = Provider::new(status);
    provider.kind = CloudProvider::Azure;
    provider
}

fn refused(fixture: &AzureFixture, expected: &RemoteEnvironmentSummary) -> ConfiguredObservationError {
    azure_with::<Provider, ()>(
        &fixture.store,
        &fixture.profile,
        expected,
        |_| panic!("the client must follow local admission"),
        |_, _| panic!("no provider read"),
    )
    .expect_err("invalid local admission")
}

#[test]
fn public_azure_check_is_admitted_before_any_profile_or_client_access() {
    let fixture = AzureFixture::new(&Shape::default());
    let expected = fixture.allocation.workspace().environment_summary();
    let configured = RemoteProviderConfig {
        azure: vec![fixture.profile.clone()],
        ..Default::default()
    };
    assert_eq!(
        observe_configured_remote_environment(&fixture.store, &RemoteProviderConfig::default(), &expected),
        Err(ConfiguredObservationError::Configuration(
            RemoteProviderConfigError::UnconfiguredAzureProfile
        ))
    );
    let mut stale = expected.clone();
    stale.revision += 1;
    assert_eq!(
        observe_configured_remote_environment(&fixture.store, &configured, &stale),
        Err(ConfiguredObservationError::Observation(Error::StateChanged))
    );
    let mut foreign = expected;
    foreign.owning_session_id = "00000000-0000-4000-8000-000000000002".into();
    assert!(observe_configured_remote_environment(&fixture.store, &configured, &foreign).is_err());
    assert_eq!(fixture.current(), fixture.allocation);
    assert_eq!(fixture.counts(), [1, 1, 0]);
}

#[test]
fn admission_refuses_before_the_client_for_every_invalid_record_or_profile() {
    let cases: Vec<Case> = vec![
        (
            Shape {
                binding: false,
                ..Shape::default()
            },
            None,
            ConfiguredObservationError::InvalidAzureBinding,
        ),
        (
            Shape::default(),
            Some(|profile| profile.vm_size = "Standard_D2s_v3".into()),
            ConfiguredObservationError::InvalidAzureBinding,
        ),
        (
            Shape {
                pin: false,
                ..Shape::default()
            },
            None,
            ConfiguredObservationError::InvalidAzureBinding,
        ),
        (
            // The store refuses a profile binding for a timed target; the lifetime is
            // refused before the binding is even looked up.
            Shape {
                lifetime: WorkerLifetime::TimeLimited { seconds: 900 },
                binding: false,
                ..Shape::default()
            },
            None,
            ConfiguredObservationError::InvalidAzureBinding,
        ),
    ];
    for (shape, edit, expected_error) in cases {
        let mut fixture = AzureFixture::new(&shape);
        if let Some(edit) = edit {
            edit(&mut fixture.profile);
        }
        let expected = fixture.allocation.workspace().environment_summary();
        let error = refused(&fixture, &expected);
        assert_eq!(error, expected_error);
        assert!(!format!("{error:?} {error}").contains(SUBSCRIPTION));
        assert_eq!(fixture.current(), fixture.allocation, "nothing is written by a refusal");
        assert_eq!(fixture.counts(), [1, 1, 0]);
    }
}

#[test]
fn retained_lifecycle_absence_and_pending_management_are_observable_without_mutation() {
    let fixture = AzureFixture::new(&Shape::default());
    let stopping = fixture
        .store
        .record_remote_stop_phase(
            &fixture.allocation,
            RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
        )
        .expect("pending Stop intent stays observable");
    let expected = stopping.workspace().environment_summary();
    for lifecycle in [
        Some(InteractiveWorkerLifecycle::Ready),
        Some(InteractiveWorkerLifecycle::Stopped),
        Some(InteractiveWorkerLifecycle::Failed),
        None,
    ] {
        let provider = azure_provider(lifecycle.map(|lifecycle| fixture.retained_status(lifecycle)));
        let observed = azure_with(
            &fixture.store,
            &fixture.profile,
            &expected,
            |_| Ok(provider),
            |provider: &Bound<'_, Provider>, workspace| {
                assert_eq!(workspace, stopping.workspace());
                observe_remote_environment(&fixture.store, provider, workspace)
            },
        )
        .expect("observation");
        assert_eq!(observed.saved, expected);
        assert_eq!(observed.worker.map(|worker| worker.lifecycle), lifecycle);
        assert_eq!(fixture.current(), stopping, "a read never moves the saved phase");
        assert_eq!(fixture.counts(), [1, 1, 0]);
    }
}

#[test]
fn cleanup_intent_is_observable_here_and_a_conflict_for_the_managing_admissions() {
    use crate::remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason, stop::configured_azure::RetainedAzure};
    let fixture = AzureFixture::new(&Shape::default());
    let mut state = fixture.allocation.workspace().state().clone();
    let runtime = state.runtime.as_mut().expect("runtime");
    runtime.phase = RemoteRuntimePhase::Deleting;
    runtime.cleanup = Some(RemoteCleanupIntent {
        reason: RemoteCleanupReason::Cancelled,
        requested_at_millis: 1,
    });
    fixture
        .store
        .replace_remote_workspace(fixture.allocation.workspace(), &state)
        .expect("management intent");
    let managed = fixture.current();
    let expected = managed.workspace().environment_summary();
    // The Stop and Start admission refuses the record; the observable admission does not.
    assert!(RetainedAzure::load(&fixture.store, &fixture.profile, &expected).is_err());
    let provider = azure_provider(Some(fixture.retained_status(InteractiveWorkerLifecycle::Ready)));
    let observed = azure_with(
        &fixture.store,
        &fixture.profile,
        &expected,
        |_| Ok(provider),
        |provider: &Bound<'_, Provider>, workspace| observe_remote_environment(&fixture.store, provider, workspace),
    )
    .expect("pending cleanup stays observable");
    assert_eq!(observed.saved.saved_phase, Some(RemoteRuntimePhase::Deleting));
    assert_eq!(
        observed.worker.map(|worker| worker.lifecycle),
        Some(InteractiveWorkerLifecycle::Ready)
    );
    assert_eq!(
        fixture.current(),
        managed,
        "the intent is neither changed nor cancelled"
    );
    assert_eq!(fixture.counts(), [1, 1, 0]);
}

#[test]
fn drift_at_the_client_during_the_read_and_after_it_never_yields_an_observation() {
    // Drift while the client is being built: the client failure is outranked.
    let fixture = AzureFixture::new(&Shape::default());
    let expected = fixture.allocation.workspace().environment_summary();
    let error = azure_with::<Provider, ()>(
        &fixture.store,
        &fixture.profile,
        &expected,
        |_| {
            let mut next = fixture.allocation.workspace().state().clone();
            next.spec.working_directory = "changed-during-client-construction".into();
            fixture
                .store
                .replace_remote_workspace(fixture.allocation.workspace(), &next)
                .expect("concurrent edit");
            Err(crate::remote_workspace::stop::ConfiguredStopConfirmationError::InvalidBinding)
        },
        |_, _| panic!("stale snapshot cannot reach the provider"),
    )
    .expect_err("state drift");
    assert_eq!(error, ConfiguredObservationError::Observation(Error::StateChanged));

    // A failed client with an intact snapshot is a binding error, with no private detail.
    let fixture = AzureFixture::new(&Shape::default());
    let expected = fixture.allocation.workspace().environment_summary();
    let error = azure_with::<Provider, ()>(
        &fixture.store,
        &fixture.profile,
        &expected,
        |_| Err(crate::remote_workspace::stop::ConfiguredStopConfirmationError::InvalidBinding),
        |_, _| panic!("no read without a client"),
    )
    .expect_err("client failure");
    assert_eq!(error, ConfiguredObservationError::InvalidAzureBinding);
    assert_eq!(
        error.to_string(),
        "the configured Azure profile or retained public worker, pin, profile binding and storage binding is invalid"
    );

    // Binding drift during the provider read is fenced by the bound provider.
    let fixture = AzureFixture::new(&Shape::default());
    let expected = fixture.allocation.workspace().environment_summary();
    let database = fixture.directory.path().join("control/store.sqlite3");
    let mut provider = azure_provider(Some(fixture.retained_status(InteractiveWorkerLifecycle::Ready)));
    provider.during_read = Some(Box::new(move || drift_binding(&database)));
    let error = azure_with(
        &fixture.store,
        &fixture.profile,
        &expected,
        |_| Ok(provider),
        |provider: &Bound<'_, Provider>, workspace| observe_remote_environment(&fixture.store, provider, workspace),
    )
    .expect_err("binding drift during the read");
    assert_eq!(error, ConfiguredObservationError::Observation(Error::StateChanged));

    // Drift after the read discards the result without overwriting the new state.
    let fixture = AzureFixture::new(&Shape::default());
    let expected = fixture.allocation.workspace().environment_summary();
    let provider = azure_provider(Some(fixture.retained_status(InteractiveWorkerLifecycle::Ready)));
    let error = azure_with(
        &fixture.store,
        &fixture.profile,
        &expected,
        |_| Ok(provider),
        |_, _| {
            let mut next = fixture.allocation.workspace().state().clone();
            next.spec.working_directory = "changed-after-the-read".into();
            fixture
                .store
                .replace_remote_workspace(fixture.allocation.workspace(), &next)
                .expect("concurrent edit");
            Ok(())
        },
    )
    .expect_err("late drift");
    assert_eq!(error, ConfiguredObservationError::Observation(Error::StateChanged));
    assert_eq!(
        fixture.current().workspace().state().spec.working_directory,
        "changed-after-the-read"
    );
}

/// Change the immutable binding row underneath the allocation, as only corruption or a
/// foreign writer could; the store's own triggers are bypassed for the fixture only.
fn drift_binding(path: &std::path::Path) {
    let mut connection = rusqlite::Connection::open(path).expect("fixture database");
    let transaction = connection.transaction().expect("fixture transaction");
    let trigger: String = transaction
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='remote_provider_bindings_no_update'",
            [],
            |row| row.get(0),
        )
        .expect("trigger");
    let digest: String = transaction
        .query_row(
            "SELECT profile_digest FROM remote_provider_bindings WHERE workspace_local_id='workspace'",
            [],
            |row| row.get(0),
        )
        .expect("digest");
    let head = &digest[..digest.len() - 1];
    let flipped = if digest.ends_with('0') {
        format!("{head}1")
    } else {
        format!("{head}0")
    };
    transaction
        .execute_batch("DROP TRIGGER remote_provider_bindings_no_update")
        .expect("fixture only");
    transaction
        .execute(
            "UPDATE remote_provider_bindings SET profile_digest=?1 WHERE workspace_local_id='workspace'",
            [&flipped],
        )
        .expect("binding drift");
    transaction.execute_batch(&trigger).expect("restore exact schema");
    transaction.commit().expect("atomic fixture");
}
