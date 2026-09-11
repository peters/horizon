use super::*;
use crate::{
    HorizonHome,
    cloud_run::{CloudProvider, interactive_worker::*},
    remote_repository_pack::tests::Fixture,
};
use std::sync::atomic::{AtomicUsize, Ordering};

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
        panic!("credential delivery cannot create")
    }
    fn reconcile_worker(&self, _: &InteractiveWorkerRequest) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        panic!("credential delivery requires a saved worker")
    }
    fn inspect_worker(&self, _: &InteractiveWorker) -> Result<Option<InteractiveWorkerStatus>, Self::Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(action) = &self.during {
            action();
        }
        Ok(self.status.clone())
    }
    fn delete_worker(&self, _: &InteractiveWorker) -> Result<InteractiveWorkerCleanup, Self::Error> {
        panic!("credential delivery cannot delete")
    }
}

fn identities(fixture: &Fixture) -> RemoteSshIdentityStore {
    RemoteSshIdentityStore::new(&HorizonHome::from_root(fixture.directory.path().join("home")))
}

fn token() -> RepositoryPat<'static> {
    RepositoryPat::new("synthetic_PAT_123").expect("synthetic")
}

#[test]
fn tokens_are_bounded_ascii_and_debug_redacted() {
    for value in [
        "",
        "synthetic token",
        "synthetic\n",
        "synthetic\r",
        "synthetic\0",
        "é",
        "a=b",
        "a-b",
    ] {
        let error = RepositoryPat::new(value).expect_err("invalid syntax");
        assert_eq!(error, RemoteCredentialDeliveryError::InvalidToken);
    }
    assert!(RepositoryPat::new(&"a".repeat(16_384)).is_ok());
    assert!(RepositoryPat::new(&"a".repeat(16_385)).is_err());
    assert_eq!(format!("{:?}", token()), "RepositoryPat([redacted])");
}

#[test]
fn explicit_installation_inspects_once_and_preserves_zero_panel_allocation_and_identity() {
    for expected in [
        RemoteCredentialInstallation::Installed,
        RemoteCredentialInstallation::Present,
    ] {
        let fixture = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
        let before = fixture.current();
        let key = std::fs::read(fixture.recovered.identity().private_key_path()).expect("key");
        let provider = Provider::new(&fixture);
        assert!(before.workspace().state().spec.panels.is_empty());
        assert_eq!(
            install_with(
                &fixture.store,
                &identities(&fixture),
                &provider,
                &before,
                &token(),
                |_, recovered, supplied| {
                    assert_eq!(recovered.allocation(), &before);
                    assert_eq!(recovered.observation(), provider.status.as_ref());
                    assert_eq!(supplied.0, "synthetic_PAT_123");
                    Ok(expected)
                }
            ),
            Ok(expected)
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.current(), before);
        assert_eq!(
            std::fs::read(fixture.recovered.identity().private_key_path()).expect("key"),
            key
        );
    }
}

#[test]
fn stale_foreign_or_pending_management_rejects_before_provider_or_delivery() {
    for stop in [false, true] {
        let fixture = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
        let before = fixture.current();
        let provider = Provider::new(&fixture);
        fixture.edit(stop);
        assert!(
            install_with(
                &fixture.store,
                &identities(&fixture),
                &provider,
                &before,
                &token(),
                |_, _, _| panic!("no delivery")
            )
            .is_err()
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        if stop {
            assert!(
                install_with(
                    &fixture.store,
                    &identities(&fixture),
                    &provider,
                    &fixture.current(),
                    &token(),
                    |_, _, _| panic!("no delivery")
                )
                .is_err()
            );
            assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        }
    }
    let fixture = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
    let foreign = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
    let provider = Provider::new(&fixture);
    assert!(
        install_with(
            &foreign.store,
            &identities(&fixture),
            &provider,
            &fixture.current(),
            &token(),
            |_, _, _| panic!("no foreign delivery")
        )
        .is_err()
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn changed_or_unavailable_observations_never_receive_token() {
    let foreign = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
    for change in 0..6 {
        let fixture = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
        let mut provider = Provider::new(&fixture);
        let status = provider.status.as_mut().expect("ready");
        match change {
            0 => status.lifecycle = InteractiveWorkerLifecycle::Stopped,
            1 => status.ssh.as_mut().expect("ssh").host_key = "invalid pin".into(),
            2 => status.worker.identity.resource_id = "other-worker".into(),
            3 => status.worker.ssh_public_key = "other-client-key".into(),
            4 => status.ssh.as_mut().expect("ssh").host_key = foreign.recovered.identity().public_key().into(),
            _ => provider.status = None,
        }
        assert!(
            install_with(
                &fixture.store,
                &identities(&fixture),
                &provider,
                &fixture.current(),
                &token(),
                |_, _, _| panic!("no delivery")
            )
            .is_err()
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn missing_retained_worker_rejects_before_reconciliation() {
    let fixture = Fixture::new(None);
    let provider = Provider::new(&fixture);
    assert_eq!(
        install_with(
            &fixture.store,
            &identities(&fixture),
            &provider,
            &fixture.current(),
            &token(),
            |_, _, _| panic!("no delivery")
        ),
        Err(RemoteCredentialDeliveryError::MissingRetainedWorker)
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn saved_worker_without_a_pin_cannot_receive_credentials_or_establish_first_trust() {
    let fixture = Fixture::with_worker(
        Some(InteractiveWorkerLifecycle::Provisioning),
        crate::cloud_run::WorkerLifetime::Persistent,
        false,
    );
    let provider = Provider::new(&fixture);
    assert!(
        fixture
            .current()
            .workspace()
            .state()
            .runtime
            .as_ref()
            .expect("runtime")
            .worker
            .is_some()
    );
    assert_eq!(
        install_with(
            &fixture.store,
            &identities(&fixture),
            &provider,
            &fixture.current(),
            &token(),
            |_, _, _| panic!("no first trust")
        ),
        Err(RemoteCredentialDeliveryError::MissingRetainedWorker)
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn expired_workers_reject_delivery_and_expiry_during_delivery_is_unknown() {
    for during in [false, true] {
        let fixture = Fixture::with_worker(
            Some(InteractiveWorkerLifecycle::Ready),
            crate::cloud_run::WorkerLifetime::TimeLimited { seconds: 2 },
            true,
        );
        let provider = Provider::new(&fixture);
        let lifetime = &provider.status.as_ref().expect("status").worker.lifetime;
        let InteractiveWorkerLifetime::TimeLimited(lease) = lifetime else {
            panic!("timed fixture")
        };
        let deadline =
            time::OffsetDateTime::parse(&lease.terminate_after, &time::format_description::well_known::Rfc3339)
                .expect("deadline");
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
        let result = install_with(
            &fixture.store,
            &identities(&fixture),
            &provider,
            &fixture.current(),
            &token(),
            |_, _, _| {
                calls += 1;
                assert!(during, "expired worker cannot receive token");
                expire();
                Ok(RemoteCredentialInstallation::Installed)
            },
        );
        if during {
            assert_eq!(result, Err(RemoteCredentialDeliveryError::DeliveryUnknown));
        } else {
            assert!(result.is_err());
        }
        assert_eq!(calls, usize::from(during));
    }
}

#[test]
fn snapshot_drift_during_provider_inspection_rejects_before_delivery() {
    let fixture = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
    let mut provider = Provider::new(&fixture);
    let path = fixture.store.path().to_path_buf();
    let retained = path.with_extension("retained");
    provider.during = Some(Box::new(move || {
        std::fs::rename(&path, &retained).expect("retain database");
    }));
    assert!(
        install_with(
            &fixture.store,
            &identities(&fixture),
            &provider,
            &fixture.current(),
            &token(),
            |_, _, _| panic!("no delivery")
        )
        .is_err()
    );
    assert!(!fixture.store.path().exists());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn drift_or_store_loss_after_delivery_is_unknown_and_never_replayed() {
    for missing in [false, true] {
        let fixture = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
        let provider = Provider::new(&fixture);
        let mut calls = 0;
        assert_eq!(
            install_with(
                &fixture.store,
                &identities(&fixture),
                &provider,
                &fixture.current(),
                &token(),
                |_, _, _| {
                    calls += 1;
                    if missing {
                        std::fs::rename(fixture.store.path(), fixture.store.path().with_extension("retained"))
                            .expect("retain database");
                    } else {
                        fixture.edit(true);
                    }
                    Ok(RemoteCredentialInstallation::Installed)
                }
            ),
            Err(RemoteCredentialDeliveryError::DeliveryUnknown)
        );
        assert_eq!(calls, 1);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        if missing {
            assert!(!fixture.store.path().exists());
        }
    }
}

#[test]
fn lost_reply_or_refusal_is_unknown_and_never_replayed() {
    let fixture = Fixture::new(Some(InteractiveWorkerLifecycle::Ready));
    let provider = Provider::new(&fixture);
    let before = fixture.current();
    let mut calls = 0;
    assert_eq!(
        install_with(
            &fixture.store,
            &identities(&fixture),
            &provider,
            &before,
            &token(),
            |_, _, _| {
                calls += 1;
                Err(RemoteCredentialDeliveryError::DeliveryUnknown)
            }
        ),
        Err(RemoteCredentialDeliveryError::DeliveryUnknown)
    );
    assert_eq!(calls, 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.current(), before);
}
