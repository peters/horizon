//! A store that locks under a running Horizon: the presence cache must not stand,
//! and the refusal must reach a create before it allocates.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use horizon_browser::remote::{ControlEndpoint, CredentialReference, CredentialStoreKind};

use super::{basic_profile, wait_until};
use crate::remote_browser_credential::{
    CredentialState, CredentialWorkbench, FakeCredentialStore, KeychainState, RemoteCredentialError,
    RemoteCredentialStore,
};

#[test]
fn a_store_that_locks_under_a_running_horizon_reports_locked_without_a_restart() {
    let locked = Arc::new(AtomicBool::new(false));
    let handle = Arc::clone(&locked);
    let profile = basic_profile(CredentialStoreKind::OsKeychain, CredentialStoreKind::Session);
    let mut workbench = CredentialWorkbench::with_opener(Box::new(move || {
        Ok(Box::new(FakeCredentialStore::with_shared_lock(Arc::clone(&handle)))
            as Box<dyn RemoteCredentialStore + Send>)
    }));
    wait_until(&mut workbench, |w| w.keychain_state() == &KeychainState::Available);
    let user = CredentialReference::from("user");
    workbench
        .store_in_keychain("grid", &profile, &user, b"alice")
        .expect("store request accepted");
    wait_until(&mut workbench, |w| {
        w.readiness(&profile)[0].state == CredentialState::Present
    });
    let _ = workbench.take_notices();

    // The daemon behind the store locks. The cached presence must not stand.
    locked.store(true, Ordering::SeqCst);
    workbench.revalidate_on_next_read();
    assert_eq!(
        workbench.readiness(&profile)[0].state,
        CredentialState::Present,
        "the state the person saw stays until the store answers"
    );
    wait_until(&mut workbench, |w| {
        w.readiness(&profile)[0].state == CredentialState::Locked
    });
    // Every later use of the store reports the relock, so a create that resolves
    // this credential is refused with `credentials_not_ready` instead of
    // allocating: the value cannot be read while the store is locked.
    workbench
        .store_in_keychain("grid", &profile, &user, b"alice2")
        .expect("the request reaches the worker");
    wait_until(&mut workbench, |w| !w.is_busy());
    let notices = workbench.take_notices();
    assert!(
        notices
            .iter()
            .any(|notice| notice.error == Some(RemoteCredentialError::Locked)),
        "{notices:?}"
    );
}

#[test]
fn the_workbench_names_the_frame_that_the_next_pass_needs() {
    let locked = Arc::new(AtomicBool::new(false));
    let handle = Arc::clone(&locked);
    let mut workbench = CredentialWorkbench::with_opener(Box::new(move || {
        Ok(Box::new(FakeCredentialStore::with_shared_lock(Arc::clone(&handle)))
            as Box<dyn RemoteCredentialStore + Send>)
    }));
    assert_eq!(workbench.next_revalidation_in(), None, "nothing is cached yet");
    let profile = basic_profile(CredentialStoreKind::OsKeychain, CredentialStoreKind::Session);
    wait_until(&mut workbench, |w| w.keychain_state() == &KeychainState::Available);
    wait_until(&mut workbench, |w| {
        w.readiness(&profile)[0].state == CredentialState::Missing
    });
    assert!(
        workbench.next_revalidation_in().is_some_and(|due| due > Duration::ZERO),
        "a cached address names the frame of its next pass"
    );
    // A due address is probed by the frame that reads it, so no frame is asked
    // for: that would spin the tab while nothing changes.
    workbench.revalidate_on_next_read();
    assert_eq!(workbench.next_revalidation_in(), None);
}

#[test]
fn every_provider_is_asked_again_even_while_another_probe_is_in_flight() {
    // Settings reads one provider at a time, so a gate for the whole workbench
    // would starve every provider after the first.
    let locked = Arc::new(AtomicBool::new(false));
    let handle = Arc::clone(&locked);
    let mut workbench = CredentialWorkbench::with_opener(Box::new(move || {
        Ok(Box::new(FakeCredentialStore::with_shared_lock(Arc::clone(&handle)))
            as Box<dyn RemoteCredentialStore + Send>)
    }));
    wait_until(&mut workbench, |w| w.keychain_state() == &KeychainState::Available);
    let first = basic_profile(CredentialStoreKind::OsKeychain, CredentialStoreKind::Session);
    let mut second = first.clone();
    second.endpoint = ControlEndpoint::parse("https://other.example.net/wd/hub").expect("endpoint");
    let user = CredentialReference::from("user");
    for profile in [&first, &second] {
        workbench
            .store_in_keychain("grid", profile, &user, b"alice")
            .expect("store request accepted");
    }
    wait_until(&mut workbench, |w| {
        w.readiness(&first)[0].state == CredentialState::Present
            && w.readiness(&second)[0].state == CredentialState::Present
    });
    let _ = workbench.take_notices();

    locked.store(true, Ordering::SeqCst);
    workbench.revalidate_on_next_read();
    // Both providers are due in the same pass, though the first sets in_flight.
    let _ = workbench.readiness(&first);
    let _ = workbench.readiness(&second);
    wait_until(&mut workbench, |w| {
        w.readiness(&first)[0].state == CredentialState::Locked
            && w.readiness(&second)[0].state == CredentialState::Locked
    });
}
