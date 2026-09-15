use super::admission::with_provider;
use super::*;
use crate::cloud_run::azure::AzureContainerRuntime;
use crate::{
    cloud_run::{
        StoredRemoteAllocation, WorkerLifetime,
        azure::{AzureDiskSku, AzureProfile, resource_group_name},
        interactive_worker::{
            InteractiveWorker, InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerIdentity,
            InteractiveWorkerLease, InteractiveWorkerLifecycle, InteractiveWorkerLifetime, InteractiveWorkerProvider,
            InteractiveWorkerRequest, InteractiveWorkerSshEndpoint, InteractiveWorkerStatus,
        },
        interactive_worker_delete::{
            InteractiveWorkerDeleteObserver, InteractiveWorkerDeletionObservation as Observation,
        },
        runpod::{RunPodNetworkVolumeExpectation, RunPodProfile},
    },
    remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason, RemoteRuntimePhase, RemoteWorkspaceState},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

mod admission;
mod operations;

const OWNER: &str = "00000000-0000-4000-8000-000000000001";
const SUBSCRIPTION: &str = "11111111-1111-4111-8111-111111111111";
const KINDS: [CloudProvider; 2] = [CloudProvider::RunPod, CloudProvider::Azure];

struct Fixture {
    _root: tempfile::TempDir,
    store: CloudWorkflowStore,
    config: RemoteProviderConfig,
}

impl Fixture {
    fn new(kind: CloudProvider, binding: bool) -> Self {
        Self::with_resource(kind, binding, None)
    }

    fn with_resource(kind: CloudProvider, binding: bool, resource: Option<&str>) -> Self {
        Self::build(kind, binding, resource, WorkerLifetime::Persistent)
    }

    fn build(kind: CloudProvider, binding: bool, resource: Option<&str>, lifetime: WorkerLifetime) -> Self {
        let root = tempfile::tempdir().expect("private fixture");
        let store = CloudWorkflowStore::open_path(root.path().join("control/workflows.sqlite3")).expect("store");
        let runpod: RunPodProfile = serde_json::from_value(serde_json::json!({
            "name":"development", "gpu_type_ids":["synthetic-gpu"], "gpu_count":1,
            "ports":["22/tcp"], "volume_gib":0, "data_center_id":"synthetic-dc"
        }))
        .expect("profile");
        let azure = azure_profile();
        let mut state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version":1, "spec":{
                "workspace_local_id":"workspace", "working_directory":".", "generation":0, "panels":[],
                "target":{"provider":kind, "profile":"development", "disk_gib":20,
                    "lifetime":"persistent", "image":format!("synthetic.azurecr.io/worker@sha256:{}", "a".repeat(64)),
                    "max_hourly_cost_micros":200_000},
                "repository":{"repository":"example/project", "commit":"b".repeat(40)}
            }
        }))
        .expect("state");
        state.spec.target.lifetime = lifetime;
        let saved = store.create_remote_workspace(OWNER, &state).expect("workspace");
        let allocation = store.allocate_remote_runtime(&saved, i64::MAX).expect("allocation");
        if binding {
            match kind {
                CloudProvider::RunPod => store
                    .record_remote_network_volume_selection(
                        &allocation,
                        &RunPodNetworkVolumeExpectation {
                            volume_id: "synthetic-volume".into(),
                            data_center_id: "synthetic-dc".into(),
                            minimum_size_gb: 10,
                        },
                    )
                    .expect("network binding"),
                CloudProvider::Azure => store
                    .record_remote_cpu_profile_binding(&allocation, &azure)
                    .expect("CPU binding"),
                CloudProvider::LocalDocker => unreachable!("fixture provider"),
            }
        }
        let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
        blob.extend([7; 32]);
        let key = format!("ssh-ed25519 {}", STANDARD.encode(blob));
        let reserved = store
            .reserve_remote_worker_request(&allocation, &key)
            .expect("public request");
        let request = reserved.worker_request().expect("request");
        let resource_id = if let Some(resource) = resource {
            resource.into()
        } else if kind == CloudProvider::Azure {
            format!(
                "/subscriptions/{SUBSCRIPTION}/resourceGroups/{}",
                resource_group_name(request.workflow_id, request.job_id)
            )
        } else {
            "synthetic-worker".into()
        };
        store
            .record_remote_worker_recovery(
                &reserved,
                Some(&InteractiveWorkerStatus {
                    worker: InteractiveWorker {
                        identity: InteractiveWorkerIdentity {
                            provider: kind,
                            workflow_id: request.workflow_id,
                            job_id: request.job_id,
                            resource_id,
                        },
                        target: request.target,
                        ssh_public_key: request.ssh_public_key,
                        lifetime: match lifetime {
                            WorkerLifetime::Persistent => InteractiveWorkerLifetime::Persistent,
                            WorkerLifetime::TimeLimited { seconds } => {
                                InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
                                    terminate_after: (time::OffsetDateTime::now_utc()
                                        + time::Duration::seconds(i64::from(seconds)))
                                    .format(&time::format_description::well_known::Rfc3339)
                                    .expect("fixture lease"),
                                })
                            }
                        },
                    },
                    lifecycle: InteractiveWorkerLifecycle::Provisioning,
                    ssh: None,
                }),
            )
            .expect("retained pin-free worker");
        Self {
            _root: root,
            store,
            config: RemoteProviderConfig {
                runpod: vec![runpod],
                azure: vec![azure],
                ..RemoteProviderConfig::default()
            },
        }
    }

    fn current(&self) -> StoredRemoteAllocation {
        current(&self.store)
    }
    fn summary(&self) -> RemoteEnvironmentSummary {
        self.current().workspace().environment_summary()
    }
    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.store.path()).expect("fixture bytes")
    }

    fn invoke(&self, operation: Operation, provider: &Provider) -> Result<ConfiguredEnvironmentDeletion, Error> {
        with_provider(&self.store, &self.config, &self.summary(), operation, |_| {
            Ok(provider.clone())
        })
    }

    fn pending(&self) {
        self.store
            .record_remote_delete_phase(
                &self.current(),
                RemoteRuntimePhase::DeleteRequested { requested_at_millis: 1 },
            )
            .expect("saved intent");
    }

    fn pinned(&self) {
        let before = self.current();
        let worker = super::super::retained_worker(&before).expect("worker").clone();
        self.store
            .record_remote_worker_recovery(
                &before,
                Some(&InteractiveWorkerStatus {
                    ssh: Some(InteractiveWorkerSshEndpoint {
                        host: "127.0.0.1".into(),
                        port: 2222,
                        username: "root".into(),
                        host_key: worker.ssh_public_key.clone(),
                    }),
                    worker,
                    lifecycle: InteractiveWorkerLifecycle::Ready,
                }),
            )
            .expect("public pin");
    }

    fn refuse_before_factory(&self, operation: Operation) -> Error {
        let before = self.bytes();
        let result = with_provider::<Provider>(&self.store, &self.config, &self.summary(), operation, |_| {
            panic!("no factory")
        });
        assert_eq!(self.bytes(), before, "admission must not write");
        result.expect_err("refused")
    }
}

fn current(store: &CloudWorkflowStore) -> StoredRemoteAllocation {
    store
        .load_remote_allocation(OWNER, "workspace")
        .expect("load")
        .expect("allocation")
}

fn azure_profile() -> AzureProfile {
    AzureProfile {
        name: "development".into(),
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

fn drift_workflow(store: &CloudWorkflowStore) {
    let before = current(store);
    let mut workflow = before.workflow().workflow().clone();
    workflow.updated_at_millis += 1;
    store
        .replace(before.workflow(), &workflow)
        .expect("synthetic workflow drift");
}

fn drift_binding(store: &CloudWorkflowStore, kind: CloudProvider) {
    let (trigger, update) = if kind == CloudProvider::RunPod {
        (
            "remote_network_volume_selections_no_update",
            "UPDATE remote_network_volume_selections SET volume_id='changed-volume'",
        )
    } else {
        (
            "remote_provider_bindings_no_update",
            "UPDATE remote_provider_bindings SET profile_digest=(CASE substr(profile_digest,1,1) WHEN '0' THEN '1' ELSE '0' END)||substr(profile_digest,2)",
        )
    };
    let mut connection = rusqlite::Connection::open(store.path()).expect("fixture writer");
    let transaction = connection.transaction().expect("transaction");
    let definition: String = transaction
        .query_row("SELECT sql FROM sqlite_schema WHERE name=?1", [trigger], |row| {
            row.get(0)
        })
        .expect("trigger");
    transaction
        .execute_batch(&format!("DROP TRIGGER {trigger}; {update}"))
        .expect("fixture-only drift");
    transaction.execute_batch(&definition).expect("restore exact schema");
    transaction.commit().expect("commit fixture");
}

type Hook = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
struct Provider {
    kind: CloudProvider,
    calls: Arc<Mutex<Vec<&'static str>>>,
    observations: Arc<Mutex<VecDeque<Result<Observation, ()>>>>,
    fail_delete: bool,
    already_absent: bool,
    hook: Option<Hook>,
}

impl Provider {
    fn witnessed_delete(kind: CloudProvider, observations: impl IntoIterator<Item = Result<Observation, ()>>) -> Self {
        let preflight = (kind == CloudProvider::RunPod).then_some(Ok(Observation::Present));
        Self::new(kind, preflight.into_iter().chain(observations))
    }

    fn deletion_calls(&self) -> Vec<&'static str> {
        let calls = self.calls();
        if self.kind == CloudProvider::RunPod {
            assert_eq!(calls.first(), Some(&"observe"), "owned-Present preflight");
            calls[1..].to_vec()
        } else {
            calls
        }
    }

    fn new(kind: CloudProvider, observations: impl IntoIterator<Item = Result<Observation, ()>>) -> Self {
        Self {
            kind,
            calls: Arc::default(),
            observations: Arc::new(Mutex::new(observations.into_iter().collect())),
            fail_delete: false,
            already_absent: false,
            hook: None,
        }
    }
    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().expect("calls").clone()
    }
    fn record(&self, name: &'static str) {
        self.calls.lock().expect("calls").push(name);
        if let Some(hook) = &self.hook {
            hook(name);
        }
    }
}

impl InteractiveWorkerProvider for Provider {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        self.kind
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("no provisioning")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("no recovery")
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("no guest inspection")
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        self.record("delete");
        if self.fail_delete {
            Err(std::io::Error::other("synthetic-secret-delete-payload"))
        } else if self.already_absent {
            Ok(InteractiveWorkerCleanup::AlreadyAbsent)
        } else {
            Ok(InteractiveWorkerCleanup::Deleted)
        }
    }
}

impl InteractiveWorkerDeleteObserver for Provider {
    fn observe_worker_deletion(&self, _: &InteractiveWorker) -> Result<Observation, Self::Error> {
        self.record("observe");
        self.observations
            .lock()
            .expect("observations")
            .pop_front()
            .expect("bounded observation")
            .map_err(|()| std::io::Error::other("synthetic-secret-observer-payload"))
    }
}
