use super::persistence::{persistent_pod, persistent_request};
use super::*;
use crate::cloud_run::{CloudWorkflowStore, GitCommitSha, GitSource, StoredRemoteAllocation};
use crate::remote_workspace::{RemoteWorkspaceSpec, RemoteWorkspaceState};

const OWNER: &str = "11111111-1111-4111-8111-111111111111";

struct Fixture {
    _directory: tempfile::TempDir,
    store: CloudWorkflowStore,
    allocation: StoredRemoteAllocation,
    request: InteractiveWorkerRequest,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("private fixture");
        let store = CloudWorkflowStore::open_path(directory.path().join("control/workflows.sqlite3")).expect("store");
        let mut request = persistent_request();
        let state = RemoteWorkspaceState::new(RemoteWorkspaceSpec {
            workspace_local_id: "remote-workspace".into(),
            target: request.target.clone(),
            repository: GitSource {
                repository: "owner/project".into(),
                commit: GitCommitSha::parse("b".repeat(40)).expect("commit"),
                branch: None,
            },
            working_directory: ".".into(),
            generation: 0,
            panels: Vec::new(),
        })
        .expect("workspace");
        let original = store.create_remote_workspace(OWNER, &state).expect("dormant record");
        let allocation = store.allocate_remote_runtime(&original, i64::MAX).expect("allocation");
        let runtime = allocation.workspace().state().runtime.as_ref().expect("runtime");
        request.workflow_id = runtime.workflow_id;
        request.job_id = runtime.job_id;
        Self {
            _directory: directory,
            store,
            allocation,
            request,
        }
    }

    fn provider(&self, transport: FakeTransport) -> RunPodInteractiveWorkerProvider {
        RunPodInteractiveWorkerProvider::new(
            RunPodClient {
                transport: Box::new(transport),
                creation_fence: Box::new(CloudWorkflowStore::open_path(self.store.path()).expect("reopen fence")),
            },
            profile(),
            FakeHostKeySource::new(Some(ed25519_key(73))),
        )
    }
}

#[test]
fn uncertain_persistent_creation_reconciles_without_deletion_or_a_second_durable_grant() {
    // The acknowledged-ID cases run the same visibility loop used by HTTP creation.
    for outcome in 0..4 {
        let fixture = Fixture::new();
        let request = &fixture.request;
        let transport = FakeTransport::with_create_response(persistent_pod(request));
        {
            let mut state = transport.0.lock().expect("state");
            state.reconcile_creation = outcome != 2;
            state.fail_create_after_insert = outcome == 2;
            if outcome < 2 {
                state.scripted_gets = http::PROPAGATION_BACKOFF_MS
                    .iter()
                    .map(|_| {
                        if outcome == 0 {
                            Ok(None)
                        } else {
                            Err(RunPodError::RequestFailed {
                                operation: "pod inspection",
                            })
                        }
                    })
                    .collect();
            } else if outcome == 3 {
                let mut incomplete = persistent_pod(request);
                incomplete.env.remove(LIFETIME_ENV);
                state.scripted_gets.push(Ok(Some(incomplete)));
            }
        }
        let first = fixture.provider(transport.clone());
        let error = first.ensure_worker(request).expect_err("uncertain creation");
        let name = resource_name(request.workflow_id, request.job_id);
        if outcome == 2 {
            assert!(
                matches!(error, RunPodError::PersistentCreationUnresolved { name: actual, cause }
                if actual == name && *cause == RunPodError::RequestFailed { operation: "pod creation" })
            );
        } else {
            assert_eq!(
                error,
                RunPodError::PersistentCreationReconciliationRequired {
                    name,
                    pod_id: "pod_123456".into()
                }
            );
        }
        {
            let state = transport.0.lock().expect("state");
            assert!(state.deleted.is_empty());
            assert_eq!(state.pods.len(), 1);
            assert_eq!(state.create_requests.len(), 1);
            if outcome < 2 {
                assert_eq!(state.inspected.len(), http::PROPAGATION_BACKOFF_MS.len());
            }
        }
        drop(first);
        let reopened = fixture.provider(transport.clone());
        let InteractiveWorkerEnsure::Reused(status) = reopened.ensure_worker(request).expect("reconcile existing")
        else {
            panic!("must not create again");
        };
        assert_eq!(status.worker.identity.resource_id, "pod_123456");
        assert_eq!(status.worker.lifetime, InteractiveWorkerLifetime::Persistent);
        assert_eq!(reopened.inspect_worker(&status.worker), Ok(Some(status.clone())));
        transport.0.lock().expect("state").pods.clear();
        assert_eq!(reopened.inspect_worker(&status.worker), Ok(None));
        drop(reopened);
        let reopened = fixture.provider(transport.clone());
        assert!(matches!(
            reopened.ensure_worker(request),
            Err(RunPodError::CreationUnresolved { .. })
        ));
        assert_eq!(
            fixture
                .store
                .load_remote_allocation(OWNER, "remote-workspace")
                .expect("allocation retained"),
            Some(fixture.allocation.clone())
        );
        let state = transport.0.lock().expect("state");
        assert_eq!(state.create_requests.len(), 1);
        assert!(state.deleted.is_empty());
    }
}

#[test]
fn acknowledged_timed_creation_still_cleans_up_after_visibility_exhaustion() {
    let request = interactive_request(CloudWorkflowId::new(), CloudJobId::new());
    let transport =
        FakeTransport::with_create_response(running_pod(request.workflow_id, request.job_id, &request.target));
    {
        let mut state = transport.0.lock().expect("state");
        state.reconcile_creation = true;
        state.scripted_gets = http::PROPAGATION_BACKOFF_MS.iter().map(|_| Ok(None)).collect();
    }
    assert_eq!(
        persistence::provider(transport.clone()).ensure_worker(&request),
        Err(RunPodError::CreationVerificationFailed {
            pod_id: "pod_123456".into()
        })
    );
    let state = transport.0.lock().expect("state");
    assert_eq!(state.create_requests.len(), 1);
    assert_eq!(state.deleted, ["pod_123456"]);
    assert!(state.pods.is_empty());
}

#[test]
fn multiple_persistent_matches_are_rejected_without_mutation() {
    let request = persistent_request();
    let pod = persistent_pod(&request);
    let mut duplicate = pod.clone();
    duplicate.id = "pod_654321".into();
    let transport = FakeTransport::with_pods(vec![pod.clone(), duplicate]);
    transport.0.lock().expect("state").scripted_lists = vec![vec![pod]];
    assert!(matches!(
        persistence::provider(transport.clone()).ensure_worker(&request),
        Err(RunPodError::AmbiguousResource { count: 2, .. })
    ));
    let state = transport.0.lock().expect("state");
    assert!(state.create_requests.is_empty());
    assert!(state.deleted.is_empty());
}
