use super::*;
use crate::{
    HorizonHome,
    cloud_run::{
        ArtifactDigest,
        interactive_worker::{
            InteractiveWorkerCleanup, InteractiveWorkerEnsure, InteractiveWorkerIdentity, InteractiveWorkerLifecycle,
            InteractiveWorkerLifetime, InteractiveWorkerProvider, InteractiveWorkerRequest, InteractiveWorkerStatus,
        },
    },
    remote_workspace::{RemoteCleanupIntent, RemoteCleanupReason},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

const OWNER: &str = "00000000-0000-4000-8000-000000000001";

fn key(byte: u8) -> String {
    let mut blob = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
    blob.extend([byte; 32]);
    format!("ssh-ed25519 {}", STANDARD.encode(blob))
}
fn pin() -> InteractiveWorkerSshEndpoint {
    InteractiveWorkerSshEndpoint {
        host: "old.example".into(),
        port: 2200,
        username: "root".into(),
        host_key: key(9),
    }
}
struct Fixture {
    root: tempfile::TempDir,
    store: CloudWorkflowStore,
}
impl Fixture {
    fn new(phase: RemoteRuntimePhase, network: bool) -> Self {
        let root = tempfile::tempdir().expect("fixture");
        let store = CloudWorkflowStore::open_path(root.path().join("control/store.sqlite3")).expect("store");
        let state: RemoteWorkspaceState = serde_json::from_value(serde_json::json!({"version":1,"spec":{
            "workspace_local_id":"workspace","working_directory":".","generation":0,"panels":[],
            "target":{"provider":"run_pod","profile":"development","disk_gib":20,"lifetime":"persistent",
                "image":format!("example/worker@sha256:{}","a".repeat(64))},
            "repository":{"repository":"example/project","commit":"b".repeat(40)}}}))
        .expect("state");
        let saved = store.create_remote_workspace(OWNER, &state).expect("workspace");
        let allocated = store.allocate_remote_runtime(&saved, i64::MAX).expect("allocation");
        if network {
            store
                .record_remote_network_volume_selection(&allocated, &volume())
                .expect("selected volume");
        }
        let reserved = store
            .reserve_remote_worker_request(&allocated, &key(7))
            .expect("reserved");
        let request = reserved.worker_request().expect("request");
        let worker = InteractiveWorker {
            identity: InteractiveWorkerIdentity {
                provider: CloudProvider::RunPod,
                workflow_id: request.workflow_id,
                job_id: request.job_id,
                resource_id: "pod_exact".into(),
            },
            target: request.target,
            ssh_public_key: request.ssh_public_key,
            lifetime: InteractiveWorkerLifetime::Persistent,
        };
        let observed = store
            .record_remote_worker_recovery(
                &reserved,
                Some(&InteractiveWorkerStatus {
                    worker,
                    lifecycle: InteractiveWorkerLifecycle::Ready,
                    ssh: Some(pin()),
                }),
            )
            .expect("observed");
        match phase {
            RemoteRuntimePhase::Starting { .. } | RemoteRuntimePhase::Stopped { .. } => {
                let stopped = store
                    .record_remote_stop_phase(
                        &observed,
                        RemoteRuntimePhase::Stopped {
                            requested_at_millis: 1,
                            observed_at_millis: 2,
                        },
                    )
                    .expect("stopped");
                if matches!(phase, RemoteRuntimePhase::Starting { .. }) {
                    store.record_remote_start_phase(&stopped, phase).expect("starting");
                }
            }
            _ => {
                let mut next = observed.workspace().state().clone();
                let runtime = next.runtime.as_mut().expect("runtime");
                runtime.phase = phase;
                if matches!(phase, RemoteRuntimePhase::Cancelling | RemoteRuntimePhase::Deleting) {
                    runtime.cleanup = Some(RemoteCleanupIntent {
                        reason: RemoteCleanupReason::Cancelled,
                        requested_at_millis: 1,
                    });
                }
                store
                    .replace_remote_workspace(observed.workspace(), &next)
                    .expect("phase");
            }
        }
        Self { root, store }
    }
    fn current(&self) -> StoredRemoteAllocation {
        self.store
            .load_remote_allocation(OWNER, "workspace")
            .expect("load")
            .expect("allocation")
    }
    fn candidate(&self) -> InteractiveWorkerEndpointCandidate {
        InteractiveWorkerEndpointCandidate {
            worker: self
                .current()
                .workspace()
                .state()
                .runtime
                .as_ref()
                .expect("runtime")
                .worker
                .clone()
                .expect("worker"),
            host: "new.example".into(),
            port: 2300,
            username: "root".into(),
            storage_fingerprint: ArtifactDigest::sha256(b"mount"),
            network_volume: selection(&self.store, &self.current()).expect("selection"),
        }
    }
}
fn volume() -> RunPodNetworkVolumeExpectation {
    RunPodNetworkVolumeExpectation {
        volume_id: "volume_exact".into(),
        data_center_id: "EUR-NO-1".into(),
        minimum_size_gb: 20,
    }
}
struct Provider {
    script: Mutex<VecDeque<Result<InteractiveWorkerEndpointCandidate, &'static str>>>,
    calls: AtomicUsize,
}
impl Provider {
    fn new(candidate: InteractiveWorkerEndpointCandidate) -> Self {
        Self {
            script: Mutex::new(VecDeque::from([Ok(candidate.clone()), Ok(candidate)])),
            calls: AtomicUsize::new(0),
        }
    }
}
impl InteractiveWorkerProvider for Provider {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        CloudProvider::RunPod
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("no create")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("no reconcile")
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("no original endpoint read")
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("no delete")
    }
}
impl InteractiveWorkerEndpointObserver for Provider {
    fn observe_endpoint_candidate(
        &self,
        _: &InteractiveWorker,
        saved: &InteractiveWorkerSshEndpoint,
    ) -> Result<InteractiveWorkerEndpointCandidate, Self::Error> {
        assert_eq!(saved, &pin());
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.script
            .lock()
            .expect("script")
            .pop_front()
            .expect("bounded reads")
            .map_err(std::io::Error::other)
    }
}

#[test]
fn authenticated_coordinates_preserve_phase_identity_fence_and_both_keys() {
    for phase in [
        RemoteRuntimePhase::Reconciling,
        RemoteRuntimePhase::Ready,
        RemoteRuntimePhase::Stopped {
            requested_at_millis: 1,
            observed_at_millis: 2,
        },
        RemoteRuntimePhase::Starting { requested_at_millis: 3 },
    ] {
        for network in [false, true] {
            let f = Fixture::new(phase, network);
            let original = f.current();
            let provider = Provider::new(f.candidate());
            let saved = refresh_with(
                &f.store,
                &provider,
                &original,
                |worker| {
                    assert_eq!(worker.ssh_public_key, key(7));
                    Ok(())
                },
                |(), endpoint| {
                    assert_eq!(endpoint.host_key, key(9));
                    assert_eq!(endpoint.username, "root");
                    assert_eq!((endpoint.host.as_str(), endpoint.port), ("new.example", 2300));
                    Ok(())
                },
            )
            .expect("refresh");
            let mut permitted = original.workspace().state().clone();
            let ssh = permitted.runtime.as_mut().expect("runtime").ssh.as_mut().expect("ssh");
            ssh.host = "new.example".into();
            ssh.port = 2300;
            assert_eq!(saved.workspace().state(), &permitted);
            assert_eq!(saved.workspace().revision(), original.workspace().revision() + 1);
            assert_eq!(saved.workflow(), original.workflow());
            assert_eq!(selection(&f.store, &saved).expect("selection"), network.then(volume));
            assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
            assert!(
                f.store
                    .replace_remote_workspace(saved.workspace(), original.workspace().state())
                    .is_err()
            );
        }
    }
}

#[test]
fn wrong_identity_or_failed_authentication_never_changes_saved_coordinates() {
    let f = Fixture::new(RemoteRuntimePhase::Reconciling, false);
    let original = f.current();
    let provider = Provider::new(f.candidate());
    assert_eq!(
        refresh_with(
            &f.store,
            &provider,
            &original,
            |_| Err::<(), _>(Error::IdentityUnavailable),
            |(), _| panic!("no probe")
        ),
        Err(Error::IdentityUnavailable)
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        refresh_with(
            &f.store,
            &provider,
            &original,
            |_| Ok(()),
            |(), _| Err(Error::AuthenticationFailed)
        ),
        Err(Error::AuthenticationFailed)
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.current(), original);
    let home = HorizonHome::from_root(f.root.path().join("missing-home"));
    assert!(
        refresh_remote_worker_endpoint(&f.store, &RemoteSshIdentityStore::new(&home), &provider, &original).is_err()
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert!(!home.root().exists());
}

#[test]
fn unchanged_coordinates_are_authenticated_without_advancing_revision() {
    let f = Fixture::new(RemoteRuntimePhase::Reconciling, false);
    let original = f.current();
    let mut candidate = f.candidate();
    candidate.host = pin().host;
    candidate.port = pin().port;
    let provider = Provider::new(candidate);
    let probes = AtomicUsize::new(0);
    let result = refresh_with(
        &f.store,
        &provider,
        &original,
        |_| Ok(()),
        |(), endpoint| {
            assert_eq!(endpoint, &pin());
            probes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .expect("authenticated unchanged endpoint");
    assert_eq!(result, original);
    assert_eq!(probes.load(Ordering::SeqCst), 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn selection_changes_during_identity_recovery_refuse_provider_io() {
    let f = Fixture::new(RemoteRuntimePhase::Reconciling, false);
    let original = f.current();
    let provider = Provider::new(f.candidate());
    let result = refresh_with(
        &f.store,
        &provider,
        &original,
        |_| {
            let mut edited = original.workspace().state().clone();
            edited.spec.working_directory = "changed".into();
            f.store
                .replace_remote_workspace(original.workspace(), &edited)
                .expect("edit");
            Ok(())
        },
        |(), _| panic!("no authentication after drift"),
    );
    assert_eq!(result, Err(Error::StateChanged));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn first_observation_mismatch_refuses_probe_and_second_read_drift_refuses_cas() {
    for later in [false, true] {
        for field in 0..7 {
            let f = Fixture::new(RemoteRuntimePhase::Reconciling, false);
            let original = f.current();
            let provider = Provider::new(f.candidate());
            let mut changed = f.candidate();
            match field {
                0 => changed.worker.identity.resource_id = "foreign".into(),
                1 => changed.username = "other".into(),
                2 => changed.network_volume = Some(volume()),
                3 => changed.port = 0,
                4 => changed.host = "bad host".into(),
                5 => changed.port = 2400,
                _ => changed.storage_fingerprint = ArtifactDigest::sha256(b"different"),
            }
            // Valid coordinate/storage changes are allowed in the first candidate,
            // but not across the authenticated re-observation.
            if !later && field >= 5 {
                continue;
            }
            provider.script.lock().expect("script")[usize::from(later)] = Ok(changed);
            let probes = AtomicUsize::new(0);
            assert!(
                refresh_with(
                    &f.store,
                    &provider,
                    &original,
                    |_| Ok(()),
                    |(), _| {
                        probes.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }
                )
                .is_err()
            );
            assert_eq!(probes.load(Ordering::SeqCst), usize::from(later));
            assert_eq!(f.current(), original);
        }
    }
}

#[test]
fn lost_observation_and_same_snapshot_competing_refresh_do_not_overwrite() {
    let f = Fixture::new(RemoteRuntimePhase::Reconciling, false);
    let original = f.current();
    for later in [false, true] {
        let provider = Provider::new(f.candidate());
        provider.script.lock().expect("script")[usize::from(later)] = Err("private provider output");
        assert_eq!(
            refresh_with(&f.store, &provider, &original, |_| Ok(()), |(), _| Ok(())),
            Err(Error::ObservationFailed)
        );
        assert_eq!(f.current(), original);
    }
    let outer = Provider::new(f.candidate());
    let inner = Provider::new(f.candidate());
    let result = refresh_with(
        &f.store,
        &outer,
        &original,
        |_| Ok(()),
        |(), _| {
            refresh_with(&f.store, &inner, &original, |_| Ok(()), |(), _| Ok(())).expect("winner");
            Ok(())
        },
    );
    assert_eq!(result, Err(Error::StateChanged));
    assert_eq!(outer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.current().workspace().revision(), original.workspace().revision() + 1);
}

#[test]
fn management_conflicts_and_late_stop_or_delete_win_before_persistence() {
    for phase in [
        RemoteRuntimePhase::Materializing,
        RemoteRuntimePhase::Checkpointing,
        RemoteRuntimePhase::Cancelling,
        RemoteRuntimePhase::Deleting,
        RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
    ] {
        let f = Fixture::new(phase, false);
        let provider = Provider::new(f.candidate());
        assert_eq!(
            refresh_with(
                &f.store,
                &provider,
                &f.current(),
                |_| panic!("no identity"),
                |(): &(), _| panic!("no probe")
            ),
            Err(Error::ManagementConflict)
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }
    for delete in [false, true] {
        let f = Fixture::new(RemoteRuntimePhase::Reconciling, false);
        let original = f.current();
        let provider = Provider::new(f.candidate());
        assert_eq!(
            refresh_with(
                &f.store,
                &provider,
                &original,
                |_| Ok(()),
                |(), _| {
                    if delete {
                        f.store
                            .record_remote_delete_phase(
                                &original,
                                RemoteRuntimePhase::DeleteRequested { requested_at_millis: 3 },
                            )
                            .expect("delete");
                    } else {
                        f.store
                            .record_remote_stop_phase(
                                &original,
                                RemoteRuntimePhase::Stopping { requested_at_millis: 3 },
                            )
                            .expect("stop");
                    }
                    Ok(())
                }
            ),
            Err(Error::StateChanged)
        );
        let pending = f.current();
        assert_eq!(
            pending
                .workspace()
                .state()
                .runtime
                .as_ref()
                .expect("runtime")
                .ssh
                .as_ref(),
            Some(&pin())
        );
        assert!(f.store.record_remote_endpoint_refresh(&pending, &pin()).is_err());
        assert_eq!(
            refresh_with(
                &f.store,
                &provider,
                &pending,
                |_| panic!("no identity"),
                |(): &(), _| panic!("no probe")
            ),
            Err(Error::ManagementConflict)
        );
    }
}

#[test]
fn dedicated_store_write_rejects_key_username_changes_and_stale_generic_writes() {
    let f = Fixture::new(RemoteRuntimePhase::Reconciling, false);
    let original = f.current();
    for field in 0..3 {
        let mut changed = pin();
        match field {
            0 => changed.host_key = key(8),
            1 => changed.username = "other".into(),
            _ => changed.port = 0,
        }
        assert!(f.store.record_remote_endpoint_refresh(&original, &changed).is_err());
        assert_eq!(f.current(), original);
    }
    let mut changed = original.workspace().state().clone();
    changed
        .runtime
        .as_mut()
        .expect("runtime")
        .ssh
        .as_mut()
        .expect("ssh")
        .port = 2300;
    assert!(
        f.store
            .replace_remote_workspace(original.workspace(), &changed)
            .is_err()
    );
    let mut edited = original.workspace().state().clone();
    edited.spec.working_directory = "src".into();
    f.store
        .replace_remote_workspace(original.workspace(), &edited)
        .expect("edit");
    let provider = Provider::new(f.candidate());
    assert_eq!(
        refresh_with(
            &f.store,
            &provider,
            &original,
            |_| panic!("no identity"),
            |(): &(), _| panic!("no probe")
        ),
        Err(Error::StateChanged)
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}
