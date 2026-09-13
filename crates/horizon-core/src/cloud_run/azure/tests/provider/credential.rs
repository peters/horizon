//! The production client over a credential that cannot produce a token: every entry
//! point fails with that exact error before any request, claim or command, and the
//! error is passed through unchanged so the caller sees why.
use super::*;
use crate::cloud_run::{
    interactive_worker::InteractiveWorkerSshEndpoint,
    interactive_worker_start::InteractiveWorkerStartProvider,
    interactive_worker_stop::{
        InteractiveWorkerStopExpectation, InteractiveWorkerStopObserver, InteractiveWorkerStopProvider,
    },
};
use std::sync::atomic::{AtomicUsize, Ordering};

const REASON: &str = "Azure CLI did not return a token";

fn unavailable() -> AzureError {
    AzureError::CredentialUnavailable { reason: REASON }
}

/// The production constructor with a credential that always fails and a fence that
/// must never be consulted: without a token there is no control-plane read, and
/// without a read nothing is ever claimed.
fn client(asked: &Arc<AtomicUsize>) -> AzureClient {
    let asked = Arc::clone(asked);
    let credential = move || {
        asked.fetch_add(1, Ordering::SeqCst);
        Err(unavailable())
    };
    let fence = |_: CloudWorkflowId, _: CloudJobId, _: &WorkerTarget, _: &str| -> Result<bool, AzureError> {
        panic!("a creation claim without a control-plane read")
    };
    AzureClient::new(profile(), credential, fence).expect("client")
}

#[test]
fn every_entry_point_reports_the_credential_failure_before_any_request_or_claim() {
    let s = Scenario::new();
    let worker = s.persisted();
    let asked = Arc::new(AtomicUsize::new(0));
    let client = client(&asked);
    let pin = InteractiveWorkerSshEndpoint {
        host: "203.0.113.9".into(),
        port: SSH_PORT,
        username: SSH_USERNAME.into(),
        host_key: host_key(),
    };
    let expectation = InteractiveWorkerStopExpectation {
        worker: &worker,
        ssh: &pin,
        network_volume: None,
    };
    let attempts: [(&str, Result<(), AzureError>); 7] = [
        ("ensure", client.ensure_worker(&s.request).map(drop)),
        ("reconcile", client.reconcile_worker(&s.request).map(drop)),
        ("inspect", client.inspect_worker(&worker).map(drop)),
        ("delete", client.delete_worker(&worker).map(drop)),
        ("stop", client.stop_worker(&worker).map(drop)),
        ("start", client.start_worker(&worker).map(drop)),
        ("observe stop", client.observe_worker_stop(expectation).map(drop)),
    ];
    for (entry, outcome) in &attempts {
        assert_eq!(
            outcome,
            &Err(unavailable()),
            "{entry} passes the credential error through"
        );
    }
    assert_eq!(
        asked.load(Ordering::SeqCst),
        attempts.len(),
        "one token request per entry point, no retry around a failing credential"
    );
    assert!(format!("{}", unavailable()).contains(REASON));
}
