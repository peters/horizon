use super::*;
use crate::cloud_runtime::mutation::State as Mutation;
use std::cell::{Cell, RefCell};

/// A provider call that is sent: it announces itself, then gives `outcome`.
fn sent(
    outcome: impl FnOnce() -> std::result::Result<(), horizon_cloud::CloudError>,
) -> impl FnOnce(
    &mut dyn FnMut() -> std::result::Result<(), horizon_cloud::CloudError>,
) -> std::result::Result<(), horizon_cloud::CloudError> {
    move |announce| {
        announce()?;
        outcome()
    }
}

fn state(root: &Path) -> Deployment {
    serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"power-test","repository":root,"revision":"a".repeat(40),
        "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
        "stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},
        "spec":null,"worker":null,"sessions":[]
    }))
    .unwrap()
}

#[test]
fn refused_boundary_never_calls_power_provider_and_keeps_explicit_stop_intent() {
    for resume in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut state = state(root.path());
        store.save(&state).unwrap();
        let before = std::fs::read(root.path().join("deployment.json")).unwrap();
        let calls = Cell::new(0);
        let request = || {
            calls.set(calls.get() + 1);
            Ok(())
        };
        let observe = |_| Err(Error::Invalid("Synthetic journal failure"));
        let result = if resume {
            request_resume(&store, &mut state, true, sent(request), &observe)
        } else {
            request_stop(&store, &mut state, true, sent(request), &observe)
        };
        assert!(result.is_err());
        assert_eq!(calls.get(), 0);
        if resume {
            assert_eq!(std::fs::read(root.path().join("deployment.json")).unwrap(), before);
        } else {
            assert_eq!(store.load().unwrap().unwrap().stage, Stage::Stopping);
        }
    }
}

#[test]
fn definite_power_rejections_settle_only_after_stop_rollback_is_durable() {
    for error in [
        horizon_cloud::CloudError::Unauthorized,
        horizon_cloud::CloudError::Rejected(horizon_cloud::Reason::default()),
        horizon_cloud::CloudError::Cancelled,
    ] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut state = state(root.path());
        store.save(&state).unwrap();
        let seen = RefCell::new(Vec::new());
        let observe = |phase| {
            if phase == Mutation::Settled {
                let saved = store.load()?.unwrap();
                assert_eq!(saved.stage, Stage::Ready);
                assert!(!saved.stop_requested);
            }
            let previous = seen.borrow().last().copied().unwrap_or(Mutation::Settled);
            seen.borrow_mut().push(phase);
            Ok(previous)
        };
        assert!(request_stop(&store, &mut state, true, sent(|| Err(error)), &observe).is_err());
        assert_eq!(*seen.borrow(), [Mutation::Pending, Mutation::Settled]);
    }
}

#[test]
fn lost_power_response_or_failed_rollback_keeps_pending_evidence() {
    for broken_rollback in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut state = state(root.path());
        store.save(&state).unwrap();
        let seen = RefCell::new(Vec::new());
        let observe = |phase| {
            let previous = seen.borrow().last().copied().unwrap_or(Mutation::Settled);
            seen.borrow_mut().push(phase);
            Ok(previous)
        };
        let result = request_stop(
            &store,
            &mut state,
            true,
            sent(|| {
                if broken_rollback {
                    std::fs::remove_file(root.path().join("deployment.json")).unwrap();
                    std::fs::create_dir(root.path().join("deployment.json")).unwrap();
                    Err(horizon_cloud::CloudError::Unauthorized)
                } else {
                    Err(horizon_cloud::CloudError::Transport)
                }
            }),
            &observe,
        );
        assert!(result.is_err());
        assert_eq!(*seen.borrow(), [Mutation::Pending]);
        if !broken_rollback {
            assert_eq!(store.load().unwrap().unwrap().stage, Stage::Stopping);
        }
    }
}

#[test]
fn resume_settles_after_persistence_and_before_later_local_preparation() {
    for fail_save in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut state = state(root.path());
        state.stage = Stage::Stopped;
        state.stop_requested = true;
        store.save(&state).unwrap();
        let seen = RefCell::new(Vec::new());
        let observe = |phase| {
            if phase == Mutation::Settled {
                let saved = store.load()?.unwrap();
                assert_eq!(saved.stage, Stage::Readiness);
                assert!(!saved.stop_requested);
            }
            let previous = seen.borrow().last().copied().unwrap_or(Mutation::Settled);
            seen.borrow_mut().push(phase);
            Ok(previous)
        };
        let resumed = request_resume(
            &store,
            &mut state,
            true,
            sent(|| {
                if fail_save {
                    std::fs::remove_file(root.path().join("deployment.json")).unwrap();
                    std::fs::create_dir(root.path().join("deployment.json")).unwrap();
                }
                Ok(())
            }),
            &observe,
        );
        assert_eq!(resumed.is_err(), fail_save);
        if fail_save {
            assert_eq!(*seen.borrow(), [Mutation::Pending]);
        } else {
            let settings: Settings = serde_json::from_value(serde_json::json!({
                "runpod_key_file":"/missing","ssh_identity_file":"/missing","docker_config":"/missing",
                "cpu_flavors":[],"gpu_types":[]
            }))
            .unwrap();
            let request = super::super::deployment::Request::new(
                state.cloud_id,
                root.path().into(),
                state.revision,
                state.profile,
                root.path().into(),
                settings,
            );
            assert!(
                super::super::deployment::deploy_locked(
                    &request,
                    &[],
                    &store,
                    |_, _| Ok(()),
                    &Cancellation::default(),
                    &|_| {},
                    &observe
                )
                .is_err()
            );
            assert_eq!(*seen.borrow(), [Mutation::Pending, Mutation::Settled]);
        }
    }
}

#[test]
fn stop_save_failure_never_calls_provider_or_changes_mutation_evidence() {
    for retry in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut state = state(root.path());
        if retry {
            state.stage = Stage::Stopping;
            state.stop_requested = true;
        }
        std::fs::create_dir(root.path().join("deployment.json")).unwrap();
        let seen = RefCell::new(Vec::new());
        let calls = Cell::new(0);
        let observe = |phase| {
            let previous = seen.borrow().last().copied().unwrap_or(Mutation::Settled);
            seen.borrow_mut().push(phase);
            Ok(previous)
        };
        assert!(
            request_stop(
                &store,
                &mut state,
                true,
                sent(|| {
                    calls.set(calls.get() + 1);
                    Ok(())
                }),
                &observe
            )
            .is_err()
        );
        assert!(seen.borrow().is_empty());
        assert_eq!(calls.get(), 0);
    }
}

#[test]
fn a_rejected_stop_retry_preserves_the_previous_uncertain_stop() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::lock(root.path()).unwrap();
    let mut state = state(root.path());
    state.stage = Stage::Stopping;
    state.stop_requested = true;
    store.save(&state).unwrap();
    let seen = RefCell::new(Vec::new());
    let observe = |phase| {
        let previous = seen.borrow().last().copied().unwrap_or(Mutation::Settled);
        seen.borrow_mut().push(phase);
        Ok(previous)
    };
    assert!(
        request_stop(
            &store,
            &mut state,
            true,
            sent(|| Err(horizon_cloud::CloudError::Unauthorized)),
            &observe
        )
        .is_err()
    );
    assert_eq!(*seen.borrow(), [Mutation::Pending]);
    assert_eq!(store.load().unwrap().unwrap().stage, Stage::Stopping);
}

#[test]
fn rejected_power_retries_preserve_the_same_observers_earlier_uncertainty() {
    for resume in [false, true] {
        for uncertain in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let store = Store::lock(root.path()).unwrap();
            let mut state = state(root.path());
            let seen = RefCell::new(Vec::new());
            let observe = |phase| {
                let previous = seen.borrow().last().copied().unwrap_or(Mutation::Settled);
                seen.borrow_mut().push(phase);
                Ok(previous)
            };
            let errors = uncertain
                .then_some(horizon_cloud::CloudError::Transport)
                .into_iter()
                .chain([horizon_cloud::CloudError::Unauthorized]);
            for error in errors {
                // Even when another local path has changed the stage, the durable
                // observer retains the first request's unresolved outcome.
                state.stage = if resume { Stage::Stopped } else { Stage::Ready };
                store.save(&state).unwrap();
                let result = if resume {
                    request_resume(&store, &mut state, true, sent(|| Err(error)), &observe)
                } else {
                    request_stop(&store, &mut state, true, sent(|| Err(error)), &observe)
                };
                assert!(result.is_err());
            }
            assert_eq!(
                seen.borrow().last(),
                Some(&if uncertain {
                    Mutation::Pending
                } else {
                    Mutation::Settled
                })
            );
        }
    }
}

#[test]
fn a_provider_check_that_fails_before_sending_records_no_pending_mutation() {
    for resume in [false, true] {
        let errors: [fn() -> horizon_cloud::CloudError; 3] = [
            || horizon_cloud::CloudError::Transport,
            || horizon_cloud::CloudError::WorkerLost,
            || horizon_cloud::CloudError::IdentityMismatch,
        ];
        for error in errors {
            let root = tempfile::tempdir().unwrap();
            let store = Store::lock(root.path()).unwrap();
            let mut state = state(root.path());
            if resume {
                state.stage = Stage::Stopped;
                state.stop_requested = true;
            }
            store.save(&state).unwrap();
            let seen = RefCell::new(Vec::new());
            let observe = |phase| {
                seen.borrow_mut().push(phase);
                Ok(Mutation::Settled)
            };
            // The provider's second identity check fails; it never announces a request.
            let unsent = |_: Announce<'_>| Err(error());
            let result = if resume {
                request_resume(&store, &mut state, true, unsent, &observe)
            } else {
                request_stop(&store, &mut state, true, unsent, &observe)
            };
            assert!(result.is_err());
            assert!(seen.borrow().is_empty(), "nothing was sent, so nothing is pending");
            let saved = store.load().unwrap().unwrap();
            if resume {
                assert_eq!(saved.stage, Stage::Stopped);
            } else {
                assert_eq!(saved.stage, Stage::Ready, "the stop intent is rolled back");
                assert!(!saved.stop_requested);
            }
        }
    }
}
