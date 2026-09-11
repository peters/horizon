use super::*;
use crate::{
    HorizonHome,
    cloud_run::{CloudProvider, WorkerLifetime, interactive_worker::*},
    remote_repository_pack::tests::Fixture,
};
use std::sync::atomic::{AtomicUsize, Ordering};

#[path = "tests/transport.rs"]
mod io;
#[path = "tests/protocol.rs"]
mod wire;

const BRANCH: &str = "work/explicit";

fn fixture() -> Fixture {
    Fixture::with_repository(
        Some(InteractiveWorkerLifecycle::Ready),
        WorkerLifetime::Persistent,
        true,
        Some(BRANCH),
    )
}

struct Provider {
    status: Option<InteractiveWorkerStatus>,
    calls: AtomicUsize,
    during: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Provider {
    fn new(fixture: &Fixture) -> Self {
        Self {
            status: fixture.recovered.observation().cloned(),
            calls: AtomicUsize::new(0),
            during: None,
        }
    }
}

impl InteractiveWorkerProvider for Provider {
    type Error = std::io::Error;
    fn provider(&self) -> CloudProvider {
        CloudProvider::LocalDocker
    }
    fn ensure_worker(&self, _: &InteractiveWorkerRequest) -> Result<InteractiveWorkerEnsure, Self::Error> {
        panic!("Git setup cannot create workers")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("Git setup cannot reconcile missing workers")
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(action) = &self.during {
            action();
        }
        Ok(self.status.clone())
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("Git setup cannot delete workers")
    }
}

fn identities(fixture: &Fixture) -> RemoteSshIdentityStore {
    RemoteSshIdentityStore::new(&HorizonHome::from_root(fixture.directory.path().join("home")))
}

fn operate(
    fixture: &Fixture,
    provider: &Provider,
    allocation: &StoredRemoteAllocation,
    branch: &str,
    execute: impl FnOnce(
        &CloudWorkflowStore,
        &crate::remote_workspace_recovery::RecoveredRemoteWorkspace,
        &[u8],
    ) -> Result<RemoteGitSubmission, RemoteGitSetupError>,
) -> Result<RemoteGitSubmission, RemoteGitSetupError> {
    operate_with(
        &fixture.store,
        &identities(fixture),
        provider,
        allocation,
        branch,
        execute,
    )
}

#[test]
fn exact_saved_binding_is_sent_once_without_state_or_identity_writes() {
    let fixture = fixture();
    let provider = Provider::new(&fixture);
    let before = fixture.current();
    let key = std::fs::read(fixture.recovered.identity().private_key_path()).unwrap();
    let mut calls = 0;
    assert_eq!(
        operate(&fixture, &provider, &before, BRANCH, |_, recovered, bytes| {
            calls += 1;
            let request = crate::repository_git::GitPreparation::decode(bytes).unwrap();
            let state = before.workspace().state();
            assert_eq!(request.source, state.spec.repository);
            assert_eq!(request.work_branch, BRANCH);
            assert_eq!(request.workspace_local_id, state.spec.workspace_local_id);
            assert_eq!(
                request.runtime_id.to_string(),
                state.runtime.as_ref().unwrap().job_id.to_string()
            );
            assert_eq!(recovered.allocation(), &before);
            Ok(RemoteGitSubmission::Submitted)
        }),
        Ok(RemoteGitSubmission::Submitted)
    );
    assert_eq!(calls, 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.current(), before);
    assert_eq!(
        std::fs::read(fixture.recovered.identity().private_key_path()).unwrap(),
        key
    );
    assert!(before.workspace().state().spec.panels.is_empty());
}

#[test]
fn missing_mismatched_and_invalid_saved_branches_reject_before_provider() {
    for saved in [None, Some(BRANCH), Some("HEAD")] {
        let fixture = Fixture::with_repository(
            Some(InteractiveWorkerLifecycle::Ready),
            WorkerLifetime::Persistent,
            true,
            saved,
        );
        let provider = Provider::new(&fixture);
        for branch in ["", "different", "HEAD"] {
            assert_eq!(
                operate(&fixture, &provider, &fixture.current(), branch, |_, _, _| panic!(
                    "no SSH"
                )),
                Err(RemoteGitSetupError::InvalidBinding)
            );
        }
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn stale_foreign_and_pending_stop_requests_never_reach_provider() {
    for stop in [false, true] {
        let fixture = fixture();
        let before = fixture.current();
        let provider = Provider::new(&fixture);
        fixture.edit(stop);
        assert!(operate(&fixture, &provider, &before, BRANCH, |_, _, _| panic!("no SSH")).is_err());
        if stop {
            assert!(
                operate(&fixture, &provider, &fixture.current(), BRANCH, |_, _, _| panic!(
                    "pending Stop"
                ))
                .is_err()
            );
        }
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }
    let first = fixture();
    let other = fixture();
    let provider = Provider::new(&first);
    assert!(operate(&other, &provider, &first.current(), BRANCH, |_, _, _| panic!("foreign")).is_err());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn missing_worker_or_pin_never_creates_or_establishes_first_trust() {
    for (state, pin) in [(None, true), (Some(InteractiveWorkerLifecycle::Provisioning), false)] {
        let fixture = Fixture::with_repository(state, WorkerLifetime::Persistent, pin, Some(BRANCH));
        let provider = Provider::new(&fixture);
        assert_eq!(
            operate(&fixture, &provider, &fixture.current(), BRANCH, |_, _, _| panic!(
                "no SSH"
            )),
            Err(RemoteGitSetupError::MissingRetainedWorker)
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn provider_drift_and_changed_valid_pin_never_cross_ssh() {
    let other = fixture();
    for change in 0..5 {
        let fixture = fixture();
        let mut provider = Provider::new(&fixture);
        let status = provider.status.as_mut().unwrap();
        match change {
            0 => status.lifecycle = InteractiveWorkerLifecycle::Stopped,
            1 => status.ssh.as_mut().unwrap().host_key = other.recovered.identity().public_key().into(),
            2 => status.worker.identity.resource_id = "different".into(),
            3 => status.worker.ssh_public_key = other.recovered.identity().public_key().into(),
            _ => provider.status = None,
        }
        assert!(
            operate(&fixture, &provider, &fixture.current(), BRANCH, |_, _, _| panic!(
                "no SSH"
            ))
            .is_err()
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn state_loss_during_inspection_rejects_without_recreation() {
    let fixture = fixture();
    let mut provider = Provider::new(&fixture);
    let path = fixture.store.path().to_path_buf();
    let retained = path.with_extension("retained");
    provider.during = Some(Box::new(move || std::fs::rename(&path, &retained).unwrap()));
    assert!(
        operate(&fixture, &provider, &fixture.current(), BRANCH, |_, _, _| panic!(
            "no SSH"
        ))
        .is_err()
    );
    assert!(!fixture.store.path().exists());
}

#[test]
fn post_exchange_drift_is_unknown_and_never_replayed() {
    for loss in [false, true] {
        let fixture = fixture();
        let provider = Provider::new(&fixture);
        let mut calls = 0;
        assert_eq!(
            operate(&fixture, &provider, &fixture.current(), BRANCH, |_, _, _| {
                calls += 1;
                if loss {
                    std::fs::rename(fixture.store.path(), fixture.store.path().with_extension("retained")).unwrap();
                } else {
                    fixture.edit(true);
                }
                Ok(RemoteGitSubmission::Submitted)
            }),
            Err(RemoteGitSetupError::OutcomeUnknown)
        );
        assert_eq!(calls, 1);
    }
}

#[test]
fn expired_before_exchange_rejects_and_expiry_during_exchange_is_unknown() {
    for during in [false, true] {
        let fixture = Fixture::with_repository(
            Some(InteractiveWorkerLifecycle::Ready),
            WorkerLifetime::TimeLimited { seconds: 2 },
            true,
            Some(BRANCH),
        );
        let provider = Provider::new(&fixture);
        let deadline = lease_deadline(&fixture.recovered).unwrap().unwrap();
        let expire = || {
            let delay = deadline - time::OffsetDateTime::now_utc();
            if delay.is_positive() {
                std::thread::sleep(delay.unsigned_abs());
            }
        };
        if !during {
            expire();
        }
        let mut calls = 0;
        let result = operate(&fixture, &provider, &fixture.current(), BRANCH, |_, _, _| {
            calls += 1;
            assert!(during);
            expire();
            Ok(RemoteGitSubmission::Submitted)
        });
        if during {
            assert_eq!(result, Err(RemoteGitSetupError::OutcomeUnknown));
        } else {
            assert!(result.is_err());
        }
        assert_eq!(calls, usize::from(during));
    }
}
