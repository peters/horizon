//! Azure reconnection admission on real stores with a fake bound provider: the real
//! ordering and drift fences run without the Azure CLI, ARM or SSH.
use super::*;
use crate::{
    cloud_run::{
        StoredRemoteAllocation,
        azure::{AzureDiskSku, AzureProfile, resource_group_name},
    },
    remote_provider_config::{RemoteProviderConfig, RemoteProviderConfigError},
    remote_workspace::{RemoteEnvironmentSummary, stop::configured_azure::Bound},
    remote_workspace_recovery::RemoteWorkspaceRecoveryError,
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

struct Shape {
    lifetime: WorkerLifetime,
    binding: bool,
    pin: bool,
    handoff: bool,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            lifetime: WorkerLifetime::Persistent,
            binding: true,
            pin: true,
            handoff: false,
        }
    }
}

struct AzureFixture {
    directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    identities: RemoteSshIdentityStore,
    profile: AzureProfile,
    allocation: StoredRemoteAllocation,
}

impl AzureFixture {
    fn new(shape: &Shape) -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).expect("private");
        let home = HorizonHome::from_root(directory.path().join("home"));
        let store = CloudWorkflowStore::open_path(directory.path().join("control/store.sqlite3")).expect("store");
        let identities = RemoteSshIdentityStore::new(&home);
        let profile = azure_profile();
        let mut state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version":1, "spec":{
                "workspace_local_id":"workspace", "working_directory":".", "generation":0,
                "target":{"provider":"azure", "profile":"cpu", "disk_gib":20, "lifetime":"persistent",
                    "image":format!("synthetic.azurecr.io/worker@sha256:{}", "a".repeat(64)),
                    "max_hourly_cost_micros":200_000},
                "repository":{"repository":"example/project", "commit":"b".repeat(40)},
                "panels":[{"panel_local_id":"terminal", "kind":"command",
                    "command":{"program":"printf", "args":["synthetic task"]}}]
            }
        }))
        .expect("state");
        state.spec.target.lifetime = shape.lifetime;
        if shape.handoff {
            state.spec.panels[0].task_handoff = Some("synthetic handoff".into());
        }
        let saved = store.create_remote_workspace(OWNER, &state).expect("workspace");
        let allocation = store.allocate_remote_runtime(&saved, i64::MAX).expect("allocation");
        if shape.binding {
            store
                .record_remote_cpu_profile_binding(&allocation, &profile)
                .expect("immutable binding before any key or provider work");
        }
        let runtime = allocation.workspace().state().runtime.as_ref().expect("runtime");
        let identity = identities
            .prepare_new(runtime.workflow_id, runtime.job_id)
            .expect("identity");
        let reserved = store
            .reserve_remote_worker_request(&allocation, identity.public_key())
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
                host_key: identity.public_key().into(),
            }),
        };
        let allocation = store
            .record_remote_worker_recovery(&reserved, Some(&status))
            .expect("synthetic retained public observation");
        Self {
            directory,
            store,
            identities,
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

    fn request<'a>(expected: &'a RemoteEnvironmentSummary, panel: &'a str) -> ConfiguredRemotePanelAttachRequest<'a> {
        ConfiguredRemotePanelAttachRequest {
            expected,
            client_session_id: OWNER,
            panel_id: panel,
            terminal: terminal_size(),
        }
    }
}

/// One admission case: the record shape, an optional profile edit, the panel asked for
/// and the refusal expected before any client exists.
type Case = (
    Shape,
    Option<fn(&mut AzureFixture)>,
    &'static str,
    ConfiguredRemotePanelAttachError,
);

/// The only fake provider an admitted attachment may see: it answers with the retained
/// worker's own observation and records every inspection.
struct Inspector {
    status: InteractiveWorkerStatus,
    calls: Mutex<usize>,
    on_inspect: Option<Box<dyn Fn() + Send + Sync>>,
}

impl InteractiveWorkerProvider for Inspector {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        CloudProvider::Azure
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("no creation")
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("no deletion")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("retained fixture must use its exact worker")
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        *self.calls.lock().expect("calls") += 1;
        if let Some(action) = &self.on_inspect {
            action();
        }
        Ok(Some(self.status.clone()))
    }
}

fn retained_status(allocation: &StoredRemoteAllocation) -> InteractiveWorkerStatus {
    let runtime = allocation.workspace().state().runtime.as_ref().expect("runtime");
    InteractiveWorkerStatus {
        worker: runtime.worker.clone().expect("worker"),
        lifecycle: InteractiveWorkerLifecycle::Ready,
        ssh: runtime.ssh.clone(),
    }
}

fn admission_error(
    fixture: &AzureFixture,
    expected: &RemoteEnvironmentSummary,
    panel: &str,
) -> ConfiguredRemotePanelAttachError {
    super::super::configured::azure_with::<Inspector, ()>(
        &fixture.store,
        &fixture.identities,
        &fixture.profile,
        AzureFixture::request(expected, panel),
        |_| panic!("the client must follow local admission"),
        |_, _| panic!("no attachment"),
    )
    .expect_err("invalid local admission")
}

#[test]
fn public_azure_attachment_is_admitted_before_any_profile_or_client_access() {
    let fixture = AzureFixture::new(&Shape::default());
    let expected = fixture.allocation.workspace().environment_summary();
    let configured = RemoteProviderConfig {
        azure: vec![fixture.profile.clone()],
        ..Default::default()
    };
    let connect = |config: &RemoteProviderConfig, expected: &RemoteEnvironmentSummary, owner: &str| {
        attach_configured_remote_panel(
            &fixture.store,
            &fixture.identities,
            config,
            ConfiguredRemotePanelAttachRequest {
                expected,
                client_session_id: owner,
                panel_id: "terminal",
                terminal: terminal_size(),
            },
        )
        .expect_err("synthetic fixture cannot admit an interactive connection")
    };
    assert_eq!(
        connect(&configured, &expected, "copied-owner"),
        ConfiguredRemotePanelAttachError::ClientSessionMismatch
    );
    assert_eq!(
        connect(&RemoteProviderConfig::default(), &expected, OWNER),
        ConfiguredRemotePanelAttachError::Configuration(RemoteProviderConfigError::UnconfiguredAzureProfile)
    );
    let mut stale = expected.clone();
    stale.revision += 1;
    assert_eq!(
        connect(&configured, &stale, OWNER),
        RemotePanelAttachError::StateChanged.into()
    );
    let mut foreign = expected;
    foreign.provider = CloudProvider::RunPod;
    assert_eq!(
        connect(&configured, &foreign, OWNER),
        ConfiguredRemotePanelAttachError::Configuration(RemoteProviderConfigError::UnconfiguredRunPodProfile)
    );
    assert_eq!(fixture.current(), fixture.allocation);
}

#[test]
fn admission_refuses_before_the_client_for_every_invalid_record_profile_or_panel() {
    let wrong_size = |fixture: &mut AzureFixture| fixture.profile.vm_size = "Standard_D2s_v3".into();
    let cases: Vec<Case> = vec![
        (
            Shape {
                binding: false,
                ..Shape::default()
            },
            None,
            "terminal",
            ConfiguredRemotePanelAttachError::InvalidAzureBinding,
        ),
        (
            Shape::default(),
            Some(wrong_size),
            "terminal",
            ConfiguredRemotePanelAttachError::InvalidAzureBinding,
        ),
        (
            Shape {
                pin: false,
                ..Shape::default()
            },
            None,
            "terminal",
            RemotePanelAttachError::from(RemotePanelStatusError::WorkerUnavailable).into(),
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
            "terminal",
            RemotePanelAttachError::UnsupportedLifetime.into(),
        ),
        (
            Shape::default(),
            None,
            "missing",
            RemotePanelAttachError::from(RemotePanelStatusError::UnknownPanel).into(),
        ),
        (
            Shape {
                handoff: true,
                ..Shape::default()
            },
            None,
            "terminal",
            RemotePanelAttachError::from(RemotePanelStatusError::UnsupportedIntent).into(),
        ),
    ];
    for (shape, edit, panel, expected_error) in cases {
        let mut fixture = AzureFixture::new(&shape);
        if let Some(edit) = edit {
            edit(&mut fixture);
        }
        let expected = fixture.allocation.workspace().environment_summary();
        let error = admission_error(&fixture, &expected, panel);
        assert_eq!(error, expected_error, "{panel}");
        assert!(!format!("{error:?}").contains(SUBSCRIPTION));
        assert_eq!(fixture.current(), fixture.allocation, "nothing is written by a refusal");
    }
}

#[test]
fn saved_stop_and_start_phases_are_refused_before_the_client_and_never_driven_forward() {
    let fixture = AzureFixture::new(&Shape::default());
    let stopping = fixture
        .store
        .record_remote_stop_phase(
            &fixture.allocation,
            RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
        )
        .expect("existing Stop intent");
    let expected = stopping.workspace().environment_summary();
    assert_eq!(
        admission_error(&fixture, &expected, "terminal"),
        RemotePanelAttachError::from(RemotePanelStatusError::ManagementPending).into()
    );
    let stopped = fixture
        .store
        .record_remote_stop_phase(
            &stopping,
            RemoteRuntimePhase::Stopped {
                requested_at_millis: 1,
                observed_at_millis: 2,
            },
        )
        .expect("verified Stop");
    let expected = stopped.workspace().environment_summary();
    assert_eq!(
        admission_error(&fixture, &expected, "terminal"),
        RemotePanelAttachError::from(RemotePanelStatusError::WorkerUnavailable).into()
    );
    let starting = fixture
        .store
        .record_remote_start_phase(&stopped, RemoteRuntimePhase::Starting { requested_at_millis: 3 })
        .expect("Start intent");
    let expected = starting.workspace().environment_summary();
    assert_eq!(
        admission_error(&fixture, &expected, "terminal"),
        RemotePanelAttachError::from(RemotePanelStatusError::ManagementPending).into()
    );
    assert_eq!(fixture.current(), starting, "no phase moved");
}

#[test]
fn missing_retained_key_is_refused_before_the_client_without_creating_a_home() {
    let fixture = AzureFixture::new(&Shape::default());
    let absent_home = HorizonHome::from_root(fixture.directory.path().join("absent-identity-home"));
    let absent = RemoteSshIdentityStore::new(&absent_home);
    let expected = fixture.allocation.workspace().environment_summary();
    let error = super::super::configured::azure_with::<Inspector, ()>(
        &fixture.store,
        &absent,
        &fixture.profile,
        AzureFixture::request(&expected, "terminal"),
        |_| panic!("no client without the retained key"),
        |_, _| panic!("no attachment"),
    )
    .expect_err("missing key");
    assert!(matches!(
        error,
        ConfiguredRemotePanelAttachError::Attachment(RemotePanelAttachError::Recovery(
            RemoteWorkspaceRecoveryError::Identity(_)
        ))
    ));
    assert!(!fixture.directory.path().join("absent-identity-home").exists());
}

#[test]
fn drift_at_the_client_discards_before_attachment_and_a_failed_client_is_a_binding_error() {
    let fixture = AzureFixture::new(&Shape::default());
    let expected = fixture.allocation.workspace().environment_summary();
    let error = super::super::configured::azure_with::<Inspector, ()>(
        &fixture.store,
        &fixture.identities,
        &fixture.profile,
        AzureFixture::request(&expected, "terminal"),
        |_| {
            let mut next = fixture.allocation.workspace().state().clone();
            next.spec.working_directory = "changed-during-client-construction".into();
            fixture
                .store
                .replace_remote_workspace(fixture.allocation.workspace(), &next)
                .expect("concurrent edit");
            Err(crate::remote_workspace::stop::ConfiguredStopConfirmationError::InvalidBinding)
        },
        |_, _| panic!("stale snapshot cannot reach attachment"),
    )
    .expect_err("state drift outranks the client failure");
    assert_eq!(error, RemotePanelAttachError::StateChanged.into());

    let fixture = AzureFixture::new(&Shape::default());
    let expected = fixture.allocation.workspace().environment_summary();
    let error = super::super::configured::azure_with::<Inspector, ()>(
        &fixture.store,
        &fixture.identities,
        &fixture.profile,
        AzureFixture::request(&expected, "terminal"),
        |_| Err(crate::remote_workspace::stop::ConfiguredStopConfirmationError::InvalidBinding),
        |_, _| panic!("no attachment without a client"),
    )
    .expect_err("client failure");
    assert_eq!(error, ConfiguredRemotePanelAttachError::InvalidAzureBinding);
    assert_eq!(
        error.to_string(),
        "the configured Azure profile or retained worker, public pin, profile binding and storage binding is invalid"
    );
}

#[test]
fn the_bound_provider_carries_the_admitted_allocation_and_fences_binding_drift_during_inspection() {
    let fixture = AzureFixture::new(&Shape::default());
    let expected = fixture.allocation.workspace().environment_summary();
    let observed = super::super::configured::azure_with(
        &fixture.store,
        &fixture.identities,
        &fixture.profile,
        AzureFixture::request(&expected, "terminal"),
        |_| {
            Ok(Inspector {
                status: retained_status(&fixture.allocation),
                calls: Mutex::new(0),
                on_inspect: None,
            })
        },
        |provider: &Bound<'_, Inspector>, request| {
            assert_eq!(request.allocation, &fixture.allocation);
            assert_eq!(request.panel_id, "terminal");
            let worker = request
                .allocation
                .workspace()
                .state()
                .runtime
                .as_ref()
                .and_then(|r| r.worker.clone());
            let status = provider
                .inspect_worker(worker.as_ref().expect("worker"))
                .expect("intact binding passes the observation through")
                .expect("observation");
            Ok(status.lifecycle)
        },
    )
    .expect("admitted");
    assert_eq!(observed, InteractiveWorkerLifecycle::Ready);

    let fixture = AzureFixture::new(&Shape::default());
    let expected = fixture.allocation.workspace().environment_summary();
    let database = fixture.directory.path().join("control/store.sqlite3");
    let error = super::super::configured::azure_with(
        &fixture.store,
        &fixture.identities,
        &fixture.profile,
        AzureFixture::request(&expected, "terminal"),
        |_| {
            Ok(Inspector {
                status: retained_status(&fixture.allocation),
                calls: Mutex::new(0),
                on_inspect: Some(Box::new(move || drift_binding(&database))),
            })
        },
        |provider: &Bound<'_, Inspector>, request| {
            let worker = request
                .allocation
                .workspace()
                .state()
                .runtime
                .as_ref()
                .and_then(|r| r.worker.clone());
            let answer = provider.inspect_worker(worker.as_ref().expect("worker"));
            assert!(answer.is_err(), "a drifted binding never yields an observation");
            Err::<(), _>(RemotePanelAttachError::Recovery(
                RemoteWorkspaceRecoveryError::ProviderUnavailable,
            ))
        },
    )
    .expect_err("binding drift");
    assert_eq!(error, RemotePanelAttachError::StateChanged.into());
}

#[test]
fn binding_drift_after_the_provider_answered_drops_a_finished_attachment() {
    let fixture = AzureFixture::new(&Shape::default());
    let expected = fixture.allocation.workspace().environment_summary();
    let database = fixture.directory.path().join("control/store.sqlite3");
    let error = super::super::configured::azure_with(
        &fixture.store,
        &fixture.identities,
        &fixture.profile,
        AzureFixture::request(&expected, "terminal"),
        |_| {
            Ok(Inspector {
                status: retained_status(&fixture.allocation),
                calls: Mutex::new(0),
                on_inspect: None,
            })
        },
        |provider: &Bound<'_, Inspector>, request| {
            let worker = request
                .allocation
                .workspace()
                .state()
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.worker.clone());
            provider
                .inspect_worker(worker.as_ref().expect("worker"))
                .expect("intact binding at inspection");
            // The binding changes between the provider's answer and the terminal
            // handover, as a foreign writer could do during the SSH intent check.
            drift_binding(&database);
            Ok("a terminal that must not be handed over")
        },
    )
    .expect_err("late binding drift");
    assert_eq!(error, RemotePanelAttachError::StateChanged.into());
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
