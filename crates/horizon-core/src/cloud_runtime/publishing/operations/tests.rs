use super::super::management::Action;
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn fixture() -> (tempfile::TempDir, Owner, Request) {
    let temp = tempfile::tempdir().unwrap();
    let owner = Owner {
        actor: "horizon:synthetic-agent".into(),
        host: "synthetic-host".into(),
        root: temp.path().join("cloud"),
        repository: temp.path().join("source"),
    };
    let request = Request {
        repository: owner.repository.clone(),
        profile: "cpu".into(),
        action: Action::List {},
    };
    (temp, owner, request)
}

fn info() -> Info {
    Info {
        enabled: true,
        selection: None,
        selection_error: None,
        choices: Vec::new(),
        shown_override: None,
        pull_saved: false,
    }
}

fn finished(manager: &mut Manager, owner: &Owner, id: &str) -> Status {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = manager.status(owner, id).unwrap();
        if status.done {
            return status;
        }
        assert!(Instant::now() < deadline, "owned operation did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn repeated_ids_reuse_owned_work_and_cannot_substitute_another_scope_or_request() {
    let (_temp, owner, request) = fixture();
    let mut manager = Manager::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let (release, wait) = mpsc::channel();
    let status = manager
        .start_with(
            owner.clone(),
            request.clone(),
            "operation-one",
            Arc::new(|| {}),
            move |_, _| {
                counted.fetch_add(1, Ordering::SeqCst);
                wait.recv().unwrap();
                Ok(info())
            },
        )
        .unwrap();
    assert!(!status.done);
    let repeated = manager
        .start_with(
            owner.clone(),
            request.clone(),
            "operation-one",
            Arc::new(|| {}),
            |_, _| panic!("replayed work"),
        )
        .unwrap();
    assert_eq!(repeated.operation_id, status.operation_id);
    let mut changed = request.clone();
    changed.profile = "another".into();
    assert!(
        manager
            .start_with(owner.clone(), changed, "operation-one", Arc::new(|| {}), |_, _| panic!(
                "changed request ran"
            ))
            .is_err()
    );
    for stranger in [
        Owner {
            actor: "horizon:other-agent".into(),
            ..owner.clone()
        },
        Owner {
            host: "another-host".into(),
            ..owner.clone()
        },
        Owner {
            root: owner.root.join("changed"),
            ..owner.clone()
        },
        Owner {
            repository: owner.repository.join("changed"),
            ..owner.clone()
        },
    ] {
        assert!(manager.status(&stranger, "operation-one").is_err());
        assert!(manager.cancel(&stranger, "operation-one").is_err());
    }
    release.send(()).unwrap();
    assert!(finished(&mut manager, &owner, "operation-one").result.is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn cancellation_is_a_request_until_the_worker_acknowledges_and_clears_its_signin() {
    let (_temp, owner, request) = fixture();
    let mut manager = Manager::default();
    let (release, wait) = mpsc::channel();
    manager
        .start_with(
            owner.clone(),
            request,
            "operation-signin",
            Arc::new(|| {}),
            move |cancel, emit| {
                emit(Event::GitHub(Prompt::Publish {
                    user_code: "SYNTHETIC-CODE".into(),
                    verification_uri: "https://github.com/login/device".into(),
                    expires_at: std::time::SystemTime::now() + Duration::from_secs(60),
                }));
                wait.recv().unwrap();
                cancel.check()?;
                Ok(info())
            },
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = manager.status(&owner, "operation-signin").unwrap();
        if let Some(prompt) = status.sign_in {
            assert_eq!(prompt.user_code, "SYNTHETIC-CODE");
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let cancelling = manager.cancel(&owner, "operation-signin").unwrap();
    assert!(cancelling.cancel_requested);
    assert!(!cancelling.done);
    release.send(()).unwrap();
    let status = finished(&mut manager, &owner, "operation-signin");
    assert!(status.error.is_some());
    assert!(status.sign_in.is_none());
    assert!(status.result.is_none());
}

#[test]
fn losing_the_live_owner_and_dropping_the_manager_cancel_owned_work() {
    let (_temp, owner, request) = fixture();
    let mut manager = Manager::default();
    let (ack, wait) = mpsc::channel();
    manager
        .start_with(
            owner.clone(),
            request.clone(),
            "operation-lost-owner",
            Arc::new(|| {}),
            move |cancel, _| {
                while !cancel.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                ack.send(()).unwrap();
                cancel.check()?;
                Ok(info())
            },
        )
        .unwrap();
    manager.poll(|_| false);
    wait.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(finished(&mut manager, &owner, "operation-lost-owner").error.is_some());
    let (ack, wait) = mpsc::channel();
    manager
        .start_with(owner, request, "operation-drop", Arc::new(|| {}), move |cancel, _| {
            while !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            ack.send(()).unwrap();
            cancel.check()?;
            Ok(info())
        })
        .unwrap();
    drop(manager);
    wait.recv_timeout(Duration::from_secs(5)).unwrap();
}

#[test]
fn expired_receipts_are_unknown_without_an_external_poll() {
    let (_temp, owner, request) = fixture();
    let mut manager = Manager::default();
    manager
        .start_with(owner.clone(), request, "operation-expired", Arc::new(|| {}), |_, _| {
            Ok(info())
        })
        .unwrap();
    assert!(finished(&mut manager, &owner, "operation-expired").done);
    let at = manager.jobs["operation-expired"]
        .finished
        .unwrap()
        .checked_add(RETENTION + Duration::from_secs(1))
        .unwrap();
    assert!(manager.status_at(&owner, "operation-expired", at).is_err());
    assert!(manager.cancel_at(&owner, "operation-expired", at).is_err());
}

#[test]
fn active_work_is_bounded_and_a_timed_out_worker_must_acknowledge_cancellation() {
    let (_temp, owner, request) = fixture();
    let mut manager = Manager::default();
    let (release, wait) = mpsc::channel();
    manager
        .start_with(
            owner.clone(),
            request.clone(),
            "bounded-first",
            Arc::new(|| {}),
            move |cancel, _| {
                wait.recv().unwrap();
                cancel.check()?;
                Ok(info())
            },
        )
        .unwrap();
    assert!(
        manager
            .start_with(
                owner.clone(),
                request.clone(),
                "bounded-second",
                Arc::new(|| {}),
                |_, _| panic!("parallel actor job")
            )
            .is_err()
    );
    let at = manager.jobs["bounded-first"]
        .started
        .checked_add(LIFETIME + Duration::from_secs(1))
        .unwrap();
    let status = manager.status_at(&owner, "bounded-first", at).unwrap();
    assert!(status.cancel_requested && !status.done);
    release.send(()).unwrap();
    assert!(finished(&mut manager, &owner, "bounded-first").error.is_some());
    let mut releases = Vec::new();
    for index in 0..MAX_ACTIVE {
        let mut actor = owner.clone();
        actor.actor = format!("synthetic-{index}");
        let (release, wait) = mpsc::channel();
        releases.push(release);
        manager
            .start_with(
                actor,
                request.clone(),
                &format!("capacity-{index}"),
                Arc::new(|| {}),
                move |_, _| {
                    wait.recv().unwrap();
                    Ok(info())
                },
            )
            .unwrap();
    }
    let mut actor = owner;
    actor.actor = "over-capacity".into();
    assert!(
        manager
            .start_with(actor, request, "capacity-excess", Arc::new(|| {}), |_, _| panic!(
                "over capacity"
            ))
            .is_err()
    );
    for release in releases {
        release.send(()).unwrap();
    }
}

#[test]
fn public_dispatch_refuses_authority_fields_and_receipt_scope_substitution() {
    let (_temp, owner, request) = fixture();
    let mut manager = Manager::default();
    manager
        .start_with(
            owner.clone(),
            request,
            "receipt-one",
            Arc::new(|| {}),
            |_, _| Ok(info()),
        )
        .unwrap();
    finished(&mut manager, &owner, "receipt-one");
    for action in [
        serde_json::json!({"operation":"list","token":"private"}),
        serde_json::json!({"operation":"status","root":"/elsewhere"}),
    ] {
        assert!(
            manager
                .dispatch(
                    owner.clone(),
                    "receipt-one",
                    Some("cpu".into()),
                    action,
                    Arc::new(|| {})
                )
                .is_err()
        );
    }
    assert!(
        manager
            .dispatch(
                owner.clone(),
                "receipt-one",
                Some("cpu".into()),
                serde_json::json!({"operation":"status"}),
                Arc::new(|| {})
            )
            .is_err()
    );
    assert!(
        manager
            .dispatch(
                owner.clone(),
                "receipt-one",
                None,
                serde_json::json!({"operation":"status"}),
                Arc::new(|| {})
            )
            .unwrap()
            .done
    );
    let mut changed = owner;
    changed.repository.push("elsewhere");
    assert!(
        manager
            .dispatch(
                changed,
                "receipt-one",
                None,
                serde_json::json!({"operation":"cancel"}),
                Arc::new(|| {})
            )
            .is_err()
    );
}

#[test]
fn abandoned_authentication_is_cancelled_without_waiting_for_the_full_lifetime() {
    let (_temp, owner, mut request) = fixture();
    request.action = Action::Authenticate {
        shown: None,
        switch: false,
    };
    let mut manager = Manager::default();
    let (release, wait) = mpsc::channel();
    manager
        .start_with(
            owner.clone(),
            request,
            "observed-auth",
            Arc::new(|| {}),
            move |cancel, _| {
                wait.recv().unwrap();
                cancel.check()?;
                Ok(info())
            },
        )
        .unwrap();
    let at = manager.jobs["observed-auth"]
        .last_observed
        .checked_add(OBSERVATION + Duration::from_secs(1))
        .unwrap();
    manager.poll_at(at, |_| true);
    let status = manager.status(&owner, "observed-auth").unwrap();
    assert!(status.cancel_requested && !status.done);
    release.send(()).unwrap();
    assert!(finished(&mut manager, &owner, "observed-auth").error.is_some());
}

#[test]
fn completed_receipts_evict_the_oldest_before_history_exceeds_its_bound() {
    let (_temp, owner, request) = fixture();
    let mut manager = Manager::default();
    for index in 0..=MAX_RETAINED {
        let id = format!("history-{index}");
        manager
            .start_with(owner.clone(), request.clone(), &id, Arc::new(|| {}), |_, _| Ok(info()))
            .unwrap();
        assert!(finished(&mut manager, &owner, &id).done);
    }
    assert_eq!(manager.jobs.len(), MAX_RETAINED);
    assert!(manager.status(&owner, "history-0").is_err());
    assert!(manager.status(&owner, &format!("history-{MAX_RETAINED}")).unwrap().done);
}
