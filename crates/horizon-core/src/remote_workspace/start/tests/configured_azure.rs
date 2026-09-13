//! Configured Azure Start on real stores with a fake provider: admission before the
//! client, durable intent before the one start, only the saved identity accepted, and
//! binding drift fenced around the provider call.
use crate::{
    cloud_run::{
        CloudProvider, CloudWorkflowStore, StoredRemoteAllocation, WorkerLifetime,
        azure::{AzureDiskSku, AzureProfile, resource_group_name},
        interactive_worker::{
            InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerIdentity,
            InteractiveWorkerLifecycle, InteractiveWorkerLifetime, InteractiveWorkerProvider, InteractiveWorkerRequest,
            InteractiveWorkerSshEndpoint, InteractiveWorkerStatus,
        },
        interactive_worker_start::{InteractiveWorkerStart, InteractiveWorkerStartProvider},
    },
    remote_provider_config::RemoteProviderConfig,
    remote_workspace::{
        RemoteEnvironmentSummary, RemoteRuntimePhase, RemoteWorkspaceState,
        start::{
            ConfiguredAzureStartError as Rejected, RemoteWorkspaceStartError as Error, configured_azure::start_with,
            start_configured_azure_environment, start_remote_workspace,
        },
        stop::{ConfiguredStopConfirmationError, configured_azure::RetainedAzure},
    },
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::sync::Mutex;

const OWNER: &str = "00000000-0000-4000-8000-000000000001";
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

fn key(byte: u8) -> String {
    let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
    blob.extend([byte; 32]);
    format!("ssh-ed25519 {}", STANDARD.encode(blob))
}

fn pin() -> InteractiveWorkerSshEndpoint {
    InteractiveWorkerSshEndpoint {
        host: "203.0.113.9".into(),
        port: 2222,
        username: "root".into(),
        host_key: key(9),
    }
}

/// The saved phase the fixture ends in.
#[derive(Clone, Copy, PartialEq)]
enum Saved {
    Running,
    Stopping,
    Stopped,
    Starting,
}

struct Fixture {
    directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    profile: AzureProfile,
}

impl Fixture {
    fn new(saved: Saved, binding: bool, pinned: bool) -> Self {
        let directory = tempfile::tempdir().expect("fixture");
        let store = CloudWorkflowStore::open_path(directory.path().join("control/store.sqlite3")).expect("store");
        let profile = azure_profile();
        let state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version":1, "spec":{
                "workspace_local_id":"workspace", "working_directory":".", "generation":0, "panels":[],
                "target":{"provider":"azure", "profile":"cpu", "disk_gib":20, "lifetime":"persistent",
                    "image":format!("synthetic.azurecr.io/worker@sha256:{}", "a".repeat(64)),
                    "max_hourly_cost_micros":200_000},
                "repository":{"repository":"example/project", "commit":"b".repeat(40)}
            }
        }))
        .expect("state");
        let workspace = store.create_remote_workspace(OWNER, &state).expect("workspace");
        let allocation = store.allocate_remote_runtime(&workspace, i64::MAX).expect("allocation");
        if binding {
            store
                .record_remote_cpu_profile_binding(&allocation, &profile)
                .expect("binding");
        }
        let reserved = store
            .reserve_remote_worker_request(&allocation, &key(7))
            .expect("public request");
        let request = reserved.worker_request().expect("request");
        let status = InteractiveWorkerStatus {
            worker: worker_for(&request),
            lifecycle: if pinned {
                InteractiveWorkerLifecycle::Ready
            } else {
                InteractiveWorkerLifecycle::Provisioning
            },
            ssh: pinned.then(pin),
        };
        store
            .record_remote_worker_recovery(&reserved, Some(&status))
            .expect("retained public observation");
        let fixture = Self {
            directory,
            store,
            profile,
        };
        if saved != Saved::Running {
            fixture
                .store
                .record_remote_stop_phase(
                    &fixture.current(),
                    RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
                )
                .expect("intent");
        }
        if matches!(saved, Saved::Stopped | Saved::Starting) {
            fixture
                .store
                .record_remote_stop_phase(
                    &fixture.current(),
                    RemoteRuntimePhase::Stopped {
                        requested_at_millis: 1,
                        observed_at_millis: 2,
                    },
                )
                .expect("completion");
        }
        if saved == Saved::Starting {
            fixture
                .store
                .record_remote_start_phase(
                    &fixture.current(),
                    RemoteRuntimePhase::Starting { requested_at_millis: 3 },
                )
                .expect("start intent");
        }
        fixture
    }

    fn current(&self) -> StoredRemoteAllocation {
        self.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation")
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

    fn start(&self, starter: &Starter) -> Result<super::super::ConfiguredAzureStart, Rejected> {
        start_with(
            &self.store,
            &self.profile,
            &self.current().workspace().environment_summary(),
            |_| Ok(starter),
            |provider, allocation| start_remote_workspace(&self.store, provider, allocation),
        )
    }
}

fn worker_for(request: &InteractiveWorkerRequest) -> InteractiveWorker {
    InteractiveWorker {
        identity: InteractiveWorkerIdentity {
            provider: CloudProvider::Azure,
            workflow_id: request.workflow_id,
            job_id: request.job_id,
            resource_id: format!(
                "/subscriptions/{SUBSCRIPTION}/resourceGroups/{}",
                resource_group_name(request.workflow_id, request.job_id)
            ),
        },
        target: request.target.clone(),
        ssh_public_key: request.ssh_public_key.clone(),
        lifetime: InteractiveWorkerLifetime::Persistent,
    }
}

type StartScript = Box<dyn Fn(&InteractiveWorker) -> Result<InteractiveWorkerStart, &'static str> + Send + Sync>;

struct Starter {
    script: StartScript,
    calls: Mutex<usize>,
}

impl Starter {
    fn answering(
        script: impl Fn(&InteractiveWorker) -> Result<InteractiveWorkerStart, &'static str> + Send + Sync + 'static,
    ) -> Self {
        Self {
            script: Box::new(script),
            calls: Mutex::new(0),
        }
    }

    fn same_worker(lifecycle: InteractiveWorkerLifecycle, already_running: bool) -> Self {
        Self::answering(move |worker| {
            let status = InteractiveWorkerStatus {
                worker: worker.clone(),
                lifecycle,
                ssh: (lifecycle == InteractiveWorkerLifecycle::Ready).then(pin),
            };
            Ok(if already_running {
                InteractiveWorkerStart::AlreadyRunning(status)
            } else {
                InteractiveWorkerStart::Started(status)
            })
        })
    }

    fn calls(&self) -> usize {
        *self.calls.lock().expect("calls")
    }
}

impl InteractiveWorkerProvider for &Starter {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        CloudProvider::Azure
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("no create")
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("no inspection")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("no recovery")
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("no delete")
    }
}

impl InteractiveWorkerStartProvider for &Starter {
    fn start_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerStart, Self::Error> {
        assert!(worker.is_valid_for(CloudProvider::Azure));
        *self.calls.lock().expect("calls") += 1;
        (self.script)(worker).map_err(std::io::Error::other)
    }
}

fn refused(fixture: &Fixture, profile: &AzureProfile, summary: Option<RemoteEnvironmentSummary>) -> Rejected {
    let before = fixture.current();
    let expected = summary.unwrap_or_else(|| before.workspace().environment_summary());
    let error = start_with(
        &fixture.store,
        profile,
        &expected,
        |_: &RetainedAzure| -> Result<&Starter, ConfiguredStopConfirmationError> {
            panic!("must not construct the client")
        },
        |_, _| panic!("no provider work"),
    )
    .expect_err("refused");
    assert_eq!(fixture.current(), before, "nothing recorded by a refusal");
    error
}

#[test]
fn public_azure_start_refuses_unsupported_or_missing_named_profiles_and_records_without_a_saved_stop() {
    let fixture = Fixture::new(Saved::Stopped, true, true);
    let before = fixture.current();
    for provider in [CloudProvider::LocalDocker, CloudProvider::RunPod, CloudProvider::Azure] {
        let mut expected = before.workspace().environment_summary();
        expected.provider = provider;
        let result = start_configured_azure_environment(&fixture.store, &RemoteProviderConfig::default(), &expected);
        if provider == CloudProvider::Azure {
            assert!(matches!(result, Err(Rejected::Configuration(_))), "{result:?}");
        } else {
            assert_eq!(result, Err(Rejected::UnsupportedProvider));
        }
        assert_eq!(fixture.current(), before);
    }
    // A configured profile with a running record is refused by admission, so the public
    // path proves its dispatch without ever reaching the Azure CLI.
    let fixture = Fixture::new(Saved::Running, true, true);
    let before = fixture.current();
    let config = RemoteProviderConfig {
        azure: vec![fixture.profile.clone()],
        ..Default::default()
    };
    assert_eq!(
        start_configured_azure_environment(&fixture.store, &config, &before.workspace().environment_summary()),
        Err(Rejected::Start(Error::NotStopped))
    );
    assert_eq!(fixture.current(), before);
}

#[test]
fn one_start_records_intent_before_dispatch_and_resolves_the_same_identity_to_reconciling() {
    for (already_running, lifecycle) in [
        (false, InteractiveWorkerLifecycle::Ready),
        (false, InteractiveWorkerLifecycle::Provisioning),
        (true, InteractiveWorkerLifecycle::Ready),
    ] {
        let fixture = Fixture::new(Saved::Stopped, true, true);
        let before = fixture.current();
        let store = fixture.store.clone();
        let fake = Starter::answering(move |worker| {
            let current = store
                .load_remote_allocation(OWNER, "workspace")
                .expect("read")
                .expect("allocation");
            let runtime = current.workspace().state().runtime.as_ref().expect("runtime");
            assert!(matches!(runtime.phase, RemoteRuntimePhase::Starting { .. }));
            assert_eq!(runtime.worker.as_ref(), Some(worker));
            let status = InteractiveWorkerStatus {
                worker: worker.clone(),
                lifecycle,
                ssh: (lifecycle == InteractiveWorkerLifecycle::Ready).then(pin),
            };
            Ok(if already_running {
                InteractiveWorkerStart::AlreadyRunning(status)
            } else {
                InteractiveWorkerStart::Started(status)
            })
        });
        let started = fixture.start(&fake).expect("verified start");
        let after = fixture.current();
        assert_eq!(started.saved, after.workspace().environment_summary());
        assert_eq!(
            (started.lifecycle, started.already_running),
            (lifecycle, already_running)
        );
        assert_eq!(fixture.phase(), RemoteRuntimePhase::Reconciling);
        let mut permitted = before.workspace().state().clone();
        permitted.runtime.as_mut().expect("runtime").phase = RemoteRuntimePhase::Reconciling;
        assert_eq!(after.workspace().state(), &permitted);
        assert_eq!(after.workspace().revision(), before.workspace().revision() + 2);
        assert_eq!(fake.calls(), 1);
        assert_eq!(fixture.start(&fake), Err(Rejected::Start(Error::NotStopped)));
        assert_eq!(fake.calls(), 1);
        assert_eq!(
            fixture.directory.path().read_dir().expect("root").count(),
            1,
            "only the control store, no identity"
        );
    }
}

#[test]
fn admission_refuses_before_the_client_for_every_invalid_record_or_profile() {
    let cases: [(&str, Saved, bool, bool, Rejected); 5] = [
        (
            "running",
            Saved::Running,
            true,
            true,
            Rejected::Start(Error::NotStopped),
        ),
        (
            "stop in flight",
            Saved::Stopping,
            true,
            true,
            Rejected::Start(Error::NotStopped),
        ),
        (
            "no pin",
            Saved::Stopped,
            true,
            false,
            Rejected::Start(Error::MissingTrust),
        ),
        ("no binding", Saved::Stopped, false, true, Rejected::InvalidBinding),
        (
            "no pin, no binding",
            Saved::Stopped,
            false,
            false,
            Rejected::InvalidBinding,
        ),
    ];
    for (label, saved, binding, pinned, expected) in cases {
        let fixture = Fixture::new(saved, binding, pinned);
        assert_eq!(refused(&fixture, &fixture.profile, None), expected, "{label}");
    }
    let fixture = Fixture::new(Saved::Stopped, true, true);
    let mut drifted = fixture.profile.clone();
    drifted.vm_size = "Standard_D2s_v3".into();
    assert_eq!(refused(&fixture, &drifted, None), Rejected::InvalidBinding);
    let mut renamed = fixture.profile.clone();
    renamed.name = "other".into();
    assert_eq!(refused(&fixture, &renamed, None), Rejected::InvalidBinding);
    let mut stale = fixture.current().workspace().environment_summary();
    stale.panel_count += 1;
    assert_eq!(
        refused(&fixture, &fixture.profile, Some(stale)),
        Rejected::Start(Error::StateChanged)
    );
}

#[test]
fn uncertainty_absence_and_foreign_observations_retain_intent_and_an_existing_intent_is_retried() {
    let outcomes: [(&str, Starter, Rejected); 4] = [
        (
            "provider failure",
            Starter::answering(|_| Err("private-provider-marker")),
            Rejected::Start(Error::ProviderUnavailable),
        ),
        (
            "absence",
            Starter::answering(|_| Ok(InteractiveWorkerStart::AlreadyAbsent)),
            Rejected::Start(Error::ResourceAbsent),
        ),
        (
            "another worker",
            Starter::answering(|worker| {
                let mut other = worker.clone();
                other.identity.resource_id = format!("/subscriptions/{SUBSCRIPTION}/resourceGroups/horizon-ws-other");
                Ok(InteractiveWorkerStart::Started(InteractiveWorkerStatus {
                    worker: other,
                    lifecycle: InteractiveWorkerLifecycle::Ready,
                    ssh: Some(pin()),
                }))
            }),
            Rejected::Start(Error::IdentityMismatch),
        ),
        (
            "replacement pin",
            Starter::answering(|worker| {
                Ok(InteractiveWorkerStart::Started(InteractiveWorkerStatus {
                    worker: worker.clone(),
                    lifecycle: InteractiveWorkerLifecycle::Ready,
                    ssh: Some(InteractiveWorkerSshEndpoint {
                        host_key: key(11),
                        ..pin()
                    }),
                }))
            }),
            Rejected::Start(Error::IdentityMismatch),
        ),
    ];
    for (label, starter, expected) in outcomes {
        let fixture = Fixture::new(Saved::Stopped, true, true);
        let before = fixture.current();
        let error = fixture.start(&starter).expect_err(label);
        assert_eq!(error, expected, "{label}");
        assert!(!format!("{error:?} {error}").contains("private-provider-marker"));
        assert!(
            matches!(fixture.phase(), RemoteRuntimePhase::Starting { .. }),
            "{label}"
        );
        let retained = fixture.current();
        let mut permitted = before.workspace().state().clone();
        permitted.runtime.as_mut().expect("runtime").phase = fixture.phase();
        assert_eq!(
            retained.workspace().state(),
            &permitted,
            "{label}: identity and pin unchanged"
        );
        assert_eq!(starter.calls(), 1);
    }
    // A saved Start intent is admitted again and resolved without re-posting.
    let fixture = Fixture::new(Saved::Starting, true, true);
    let before = fixture.current();
    let retry = Starter::same_worker(InteractiveWorkerLifecycle::Ready, true);
    let started = fixture.start(&retry).expect("retry");
    assert!(started.already_running);
    assert_eq!(fixture.phase(), RemoteRuntimePhase::Reconciling);
    assert_eq!(started.saved.revision, before.workspace().revision() + 1);
}

#[test]
fn drift_at_the_client_and_binding_drift_during_the_start_never_resolve_intent() {
    // Drift while the client is built refuses dispatch, with and without a client error.
    for failure in [false, true] {
        let fixture = Fixture::new(Saved::Stopped, true, true);
        let before = fixture.current();
        let starter = Starter::same_worker(InteractiveWorkerLifecycle::Ready, false);
        let result = start_with(
            &fixture.store,
            &fixture.profile,
            &before.workspace().environment_summary(),
            |_| {
                let current = fixture.current();
                let mut workflow = current.workflow().workflow().clone();
                workflow.updated_at_millis += 1;
                fixture
                    .store
                    .replace(current.workflow(), &workflow)
                    .expect("workflow drift");
                if failure {
                    Err(ConfiguredStopConfirmationError::InvalidBinding)
                } else {
                    Ok(&starter)
                }
            },
            |_, _| panic!("no dispatch after snapshot drift"),
        );
        assert_eq!(result, Err(Rejected::Start(Error::StateChanged)));
        assert_eq!(fixture.current().workspace(), before.workspace());
        assert_eq!(starter.calls(), 0);
    }
    // The binding changing while the provider starts leaves the intent for a retry.
    for result in [Ok(()), Err("in-flight failure")] {
        let fixture = Fixture::new(Saved::Stopped, true, true);
        let path = fixture.store.path().to_path_buf();
        let starter = Starter::answering(move |worker| {
            drift_binding(&path);
            result.map(|()| {
                InteractiveWorkerStart::Started(InteractiveWorkerStatus {
                    worker: worker.clone(),
                    lifecycle: InteractiveWorkerLifecycle::Ready,
                    ssh: Some(pin()),
                })
            })
        });
        assert_eq!(
            fixture.start(&starter),
            Err(Rejected::Start(Error::StateChanged)),
            "{result:?}"
        );
        assert_eq!(starter.calls(), 1);
        assert!(matches!(fixture.phase(), RemoteRuntimePhase::Starting { .. }));
    }
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

#[test]
fn a_timed_target_is_never_started() {
    let fixture = Fixture::new(Saved::Stopped, true, true);
    let before = fixture.current();
    let mut summary = before.workspace().environment_summary();
    summary.lifetime = WorkerLifetime::TimeLimited { seconds: 900 };
    assert_eq!(
        refused(&fixture, &fixture.profile, Some(summary)),
        Rejected::Start(Error::StateChanged),
        "a summary that does not match the saved persistent record is stale, not admitted"
    );
}
