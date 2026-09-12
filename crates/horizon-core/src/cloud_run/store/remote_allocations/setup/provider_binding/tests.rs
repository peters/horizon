use super::*;
use crate::cloud_run::interactive_worker::{
    InteractiveWorker, InteractiveWorkerIdentity, InteractiveWorkerLifecycle, InteractiveWorkerLifetime,
    InteractiveWorkerStatus,
};
use crate::cloud_run::store::encode_workflow;
use crate::remote_workspace::{RemoteRuntimePhase, RemoteWorkspaceState};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::sync::{Arc, Barrier};

mod corruption;

const OWNER: &str = "11111111-1111-4111-8111-111111111111";
const SUBSCRIPTION: &str = "00000000-0000-4000-8000-000000000001";

fn profile() -> AzureProfile {
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
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    saved: StoredRemoteAllocation,
}

impl Fixture {
    fn new() -> Self {
        Self::with_target(|_| {})
    }
    fn with_target(change: impl FnOnce(&mut crate::cloud_run::WorkerTarget)) -> Self {
        let directory = tempfile::tempdir().expect("directory");
        let store = CloudWorkflowStore::open_path(directory.path().join("control/workflows.sqlite3")).expect("store");
        let mut state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({
            "version": 1, "spec": {
                "workspace_local_id": "workspace", "working_directory": ".", "generation": 0, "panels": [],
                "target": { "provider": "azure", "profile": "development",
                    "image": format!("synthetic.azurecr.io/worker@sha256:{}", "a".repeat(64)),
                    "disk_gib": 20, "lifetime": "persistent" },
                "repository": { "repository": "example/project", "commit": "b".repeat(40) }
            }
        }))
        .expect("state");
        change(&mut state.spec.target);
        let dormant = store.create_remote_workspace(OWNER, &state).expect("workspace");
        let saved = store.allocate_remote_runtime(&dormant, i64::MAX).expect("allocation");
        Self {
            _directory: directory,
            store,
            saved,
        }
    }
    fn raw(&self) -> Connection {
        Connection::open(self.store.path()).expect("raw fixture")
    }
    fn reload(&self) -> StoredRemoteAllocation {
        self.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation")
    }
    fn count(&self) -> i64 {
        self.raw()
            .query_row("SELECT COUNT(*) FROM remote_provider_bindings", [], |row| row.get(0))
            .expect("count")
    }
    fn snapshots(&self) -> Vec<(i64, Vec<u8>)> {
        self.raw().prepare("SELECT revision, snapshot FROM remote_workspaces UNION ALL SELECT revision, snapshot FROM cloud_workflows")
            .expect("query").query_map([], |row| Ok((row.get(0)?, row.get(1)?))).expect("rows")
            .collect::<Result<_, _>>().expect("snapshots")
    }
    fn record(&self) {
        self.store
            .record_remote_cpu_profile_binding(&self.saved, &profile())
            .expect("record");
    }
    fn claim(&self) -> bool {
        let runtime = self.saved.workspace().state().runtime.as_ref().expect("runtime");
        self.store
            .claim_worker_creation(
                runtime.workflow_id,
                runtime.job_id,
                &self.saved.workspace().state().spec.target,
                "synthetic-worker",
            )
            .expect("claim")
    }
    fn reserve(&self) -> StoredRemoteAllocation {
        let mut key = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
        key.extend([7; 32]);
        self.store
            .reserve_remote_worker_request(&self.reload(), &format!("ssh-ed25519 {}", STANDARD.encode(key)))
            .expect("key")
    }
    fn expire(&self) -> StoredRemoteAllocation {
        let mut workflow = self.reload().workflow().workflow().clone();
        workflow.created_at_millis = 1000;
        workflow.updated_at_millis = 1000;
        workflow.retain_until_millis = 2000;
        self.raw()
            .execute(
                "UPDATE cloud_workflows SET created_at_millis=1000, updated_at_millis=1000,
            retain_until_millis=2000, snapshot=?1",
                [encode_workflow(&workflow).expect("snapshot")],
            )
            .expect("expire");
        self.reload()
    }
    fn observe(&self) -> StoredRemoteAllocation {
        let saved = self.reserve();
        let request = saved.worker_request().expect("request");
        let status = InteractiveWorkerStatus {
            worker: InteractiveWorker {
                identity: InteractiveWorkerIdentity {
                    provider: CloudProvider::Azure,
                    workflow_id: request.workflow_id,
                    job_id: request.job_id,
                    resource_id: format!("/subscriptions/{SUBSCRIPTION}/resourceGroups/synthetic"),
                },
                target: request.target,
                ssh_public_key: request.ssh_public_key,
                lifetime: InteractiveWorkerLifetime::Persistent,
            },
            lifecycle: InteractiveWorkerLifecycle::Provisioning,
            ssh: None,
        };
        self.store
            .record_remote_worker_recovery(&saved, Some(&status))
            .expect("observation")
    }
}

#[test]
fn frozen_digest_covers_every_profile_field_and_rejects_invalid_profiles() {
    let original = profile();
    let binding = RemoteCpuProfileBinding::from_profile(&original).expect("binding");
    assert_eq!(binding.subscription_id(), SUBSCRIPTION);
    assert_eq!(
        binding.profile_digest().as_str(),
        "c35a8253e80b9965b2e38b95f928cbabeffbeda306a12a2c2d3b128ca305f615"
    );
    assert!(binding.matches_profile(&original).expect("match"));
    for mode in 0..8 {
        let mut next = original.clone();
        match mode {
            0 => next.name = "another".into(),
            1 => {
                next.subscription_id = "00000000-0000-4000-8000-000000000002".into();
                next.image_pull_identity_id = next.image_pull_identity_id.replace(SUBSCRIPTION, &next.subscription_id);
            }
            2 => next.location = "westeurope".into(),
            3 => next.vm_size = "Standard_D8s_v3".into(),
            4 => next.image_pull_identity_id.push('2'),
            5 => next.declared_hourly_cost_micros += 1,
            6 => next.registry_login_server = "other.azurecr.io".into(),
            _ => next.disk_sku = AzureDiskSku::PremiumLrs,
        }
        assert!(!binding.matches_profile(&next).expect("valid different profile"));
    }
    let mut invalid = original;
    invalid.subscription_id = "invalid-sensitive-value".into();
    assert!(binding.matches_profile(&invalid).is_err());
    assert!(!format!("{binding:?}").contains(SUBSCRIPTION));
}

#[test]
fn record_reopen_and_late_exact_repeats_preserve_snapshots_and_creation_grant() {
    let fixture = Fixture::new();
    let before = fixture.snapshots();
    assert_eq!(
        fixture
            .store
            .load_remote_cpu_profile_binding(&fixture.saved)
            .expect("absent"),
        None
    );
    fixture.record();
    fixture.record();
    assert_eq!(fixture.count(), 1);
    assert_eq!(fixture.snapshots(), before);
    assert_eq!(fixture.reload(), fixture.saved);
    let reader = CloudWorkflowStore::open_read_only_path(fixture.store.path()).expect("reader");
    assert_eq!(
        reader.load_remote_cpu_profile_binding(&fixture.saved).expect("read"),
        Some(RemoteCpuProfileBinding::from_profile(&profile()).expect("profile"))
    );
    assert!(fixture.claim());
    assert!(!fixture.claim());
    fixture.record();
    let expired = fixture.expire();
    let before = fixture.snapshots();
    fixture
        .store
        .record_remote_cpu_profile_binding(&expired, &profile())
        .expect("expired same binding");
    assert!(
        reader
            .load_remote_cpu_profile_binding(&expired)
            .expect("expired read")
            .is_some()
    );
    assert_eq!(fixture.snapshots(), before);
}

#[test]
fn different_stale_and_foreign_bindings_cannot_replace_the_original() {
    let fixture = Fixture::new();
    fixture.record();
    let mut next = profile();
    next.location = "westeurope".into();
    assert!(matches!(
        fixture.store.record_remote_cpu_profile_binding(&fixture.saved, &next),
        Err(Error::ReplacementIdentityMismatch)
    ));
    let other = Fixture::new();
    assert!(
        fixture
            .store
            .record_remote_cpu_profile_binding(&other.saved, &profile())
            .is_err()
    );
    assert!(fixture.store.load_remote_cpu_profile_binding(&other.saved).is_err());
    let current = fixture.reserve();
    assert!(matches!(
        fixture.store.load_remote_cpu_profile_binding(&fixture.saved),
        Err(Error::SnapshotConflict)
    ));
    assert!(matches!(
        fixture
            .store
            .record_remote_cpu_profile_binding(&fixture.saved, &profile()),
        Err(Error::SnapshotConflict)
    ));
    assert!(
        fixture
            .store
            .load_remote_cpu_profile_binding(&current)
            .expect("current")
            .is_some()
    );
    assert_eq!(fixture.count(), 1);
}

#[test]
fn first_record_rejects_claim_key_expiry_first_pin_worker_and_management() {
    for mode in 0..6 {
        let fixture = Fixture::new();
        let mut saved = fixture.saved.clone();
        match mode {
            0 => {
                assert!(fixture.claim());
            }
            1 => saved = fixture.reserve(),
            2 => saved = fixture.expire(),
            3 => {
                let runtime = saved.workspace().state().runtime.as_ref().expect("runtime");
                fixture
                    .raw()
                    .execute(
                        "INSERT INTO remote_first_pin_intents VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                        params![
                            "workspace",
                            OWNER,
                            i64::try_from(runtime.generation).expect("generation"),
                            runtime.workflow_id.to_string(),
                            runtime.job_id.to_string(),
                            "a".repeat(64)
                        ],
                    )
                    .expect("first-pin fixture");
            }
            4 => saved = fixture.observe(),
            _ => {
                saved = fixture
                    .store
                    .record_remote_stop_phase(
                        &fixture.observe(),
                        RemoteRuntimePhase::Stopping {
                            requested_at_millis: 1000,
                        },
                    )
                    .expect("Stop intent");
            }
        }
        let before = fixture.snapshots();
        assert!(matches!(
            fixture.store.record_remote_cpu_profile_binding(&saved, &profile()),
            Err(Error::RuntimeSetupUnavailable)
        ));
        assert_eq!(fixture.count(), 0);
        assert_eq!(fixture.snapshots(), before);
    }
}

#[test]
fn existing_binding_remains_readable_in_management_without_state_changes() {
    let fixture = Fixture::new();
    fixture.record();
    let stopped = fixture
        .store
        .record_remote_stop_phase(
            &fixture.observe(),
            RemoteRuntimePhase::Stopping {
                requested_at_millis: 1000,
            },
        )
        .expect("Stop intent");
    let before = fixture.snapshots();
    fixture
        .store
        .record_remote_cpu_profile_binding(&stopped, &profile())
        .expect("same binding");
    assert!(
        fixture
            .store
            .load_remote_cpu_profile_binding(&stopped)
            .expect("read")
            .is_some()
    );
    assert_eq!(fixture.reload(), stopped);
    assert_eq!(fixture.snapshots(), before);
}

#[test]
fn competing_identical_or_different_profiles_have_no_replacement_or_snapshot_writes() {
    for identical in [true, false] {
        let fixture = Fixture::new();
        let before = fixture.snapshots();
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|index| {
                let store = fixture.store.clone();
                let saved = fixture.saved.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let mut candidate = profile();
                    if index == 1 && !identical {
                        candidate.location = "westeurope".into();
                    }
                    barrier.wait();
                    store.record_remote_cpu_profile_binding(&saved, &candidate)
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().expect("thread")).collect();
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            if identical { 2 } else { 1 }
        );
        assert!(
            results
                .iter()
                .all(|r| r.is_ok() || matches!(r, Err(Error::ReplacementIdentityMismatch)))
        );
        assert_eq!(fixture.count(), 1);
        assert_eq!(fixture.snapshots(), before);
    }
}

#[test]
fn complete_deployment_target_is_required_before_any_binding_write() {
    for mode in 0..6 {
        let fixture = Fixture::with_target(|target| match mode {
            0 => target.provider = CloudProvider::LocalDocker,
            1 => target.profile = "other".into(),
            2 => target.disk_gib = 4096,
            3 => target.max_hourly_cost_micros = Some(1),
            4 => target.image = format!("foreign.azurecr.io/worker@sha256:{}", "a".repeat(64)),
            _ => target.lifetime = WorkerLifetime::TimeLimited { seconds: 60 },
        });
        let before = fixture.snapshots();
        assert!(matches!(
            fixture
                .store
                .record_remote_cpu_profile_binding(&fixture.saved, &profile()),
            Err(Error::RuntimeSetupUnavailable)
        ));
        assert_eq!(fixture.count(), 0);
        assert_eq!(fixture.snapshots(), before);
    }
}
