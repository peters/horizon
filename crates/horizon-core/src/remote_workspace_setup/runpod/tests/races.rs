use super::*;

#[test]
fn competing_first_pin_after_trust_selection_fails_before_recovery_dispatch() {
    let f = Fixture::new();
    let expected = f.reserve(true);
    f.claim(&expected);
    let observed = f.status();
    *f.remote.observation.lock().expect("observation") = Some(observed.clone());
    let mut winner = None;
    let result = f.run_expected(&expected, Operation::Recover, |trust| {
        assert!(matches!(&trust, TrustSelection::Initial(_)));
        let provider = f.factory(&trust);
        winner = Some(
            f.store
                .record_remote_worker_recovery(&expected, Some(&observed))
                .expect("competing first pin"),
        );
        provider
    });
    assert_eq!(result, Err(RemoteWorkspaceRecoveryError::StateChanged.into()));
    assert_eq!(f.allocation(), winner.expect("winner"));
    assert_eq!(*f.remote.calls.lock().expect("calls"), [0; 5]);
    assert_eq!(f.counts(), [1, 1, 1, 0]);
}

#[test]
fn updates_after_inspection_starts_cannot_be_overwritten_or_trigger_cleanup() {
    for kind in 0..3 {
        let f = Fixture::new();
        let expected = f.reserve(true);
        f.claim(&expected);
        *f.remote.observation.lock().expect("observation") = Some(f.status());
        let store = f.store.clone();
        let winner = Arc::new(Mutex::new(None));
        let recorded = winner.clone();
        let mut status = f.status();
        *f.remote.after_read.lock().expect("hook") = Some(Box::new(move || {
            match kind {
                0 => {
                    status.ssh.as_mut().expect("ssh").host = "127.0.0.2".into();
                    store
                        .record_remote_worker_recovery(&expected, Some(&status))
                        .expect("winning pin");
                }
                1 => {
                    let mut workflow = expected.workflow().workflow().clone();
                    workflow.title = "winning edit".into();
                    workflow.updated_at_millis += 1;
                    store.replace(expected.workflow(), &workflow).expect("winning workflow");
                }
                _ => record_management(&store, &expected),
            }
            *recorded.lock().expect("recorded") = store.load_remote_allocation(OWNER, "workspace").expect("winner");
        }));
        let keys = f.keys();
        assert_eq!(
            f.run(Operation::Recover),
            Err(RemoteWorkspaceRecoveryError::StateChanged.into())
        );
        assert_eq!(f.allocation(), winner.lock().expect("winner").clone().expect("stored"));
        assert_eq!(f.keys(), keys);
        assert_eq!(*f.remote.calls.lock().expect("calls"), [0, 0, 1, 0, 0]);
    }
}

#[test]
fn concurrent_explicit_starts_and_retries_never_allocate_or_create_twice() {
    use std::sync::Barrier;
    for retry in [false, true] {
        let f = Fixture::new();
        if retry {
            f.reserve(true);
        }
        let barrier = Barrier::new(2);
        std::thread::scope(|scope| {
            let tasks = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        if retry { f.run(Operation::Retry) } else { f.start() }
                    })
                })
                .collect::<Vec<_>>();
            let results = tasks
                .into_iter()
                .map(|task| task.join().expect("thread"))
                .collect::<Vec<_>>();
            assert!(results.iter().any(Result::is_ok));
        });
        assert_eq!(f.counts()[..3], [1, 1, 1]);
        let keys = f.keys();
        assert_eq!(keys.len(), 1);
        let calls = *f.remote.calls.lock().expect("calls");
        assert_eq!(calls[1], 1);
        f.run(Operation::Recover).expect("noncreating convergence");
        assert_eq!(f.counts(), [1, 1, 1, 0]);
        assert_eq!(f.remote.calls.lock().expect("calls")[..2], calls[..2]);
        assert_eq!(f.remote.calls.lock().expect("calls")[4], 0);
        assert_eq!(f.keys(), keys);
    }
}
