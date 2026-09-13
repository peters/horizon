use super::*;

#[test]
fn pin_free_delete_saves_intent_before_dispatch_and_retains_exact_tombstone() {
    for kind in KINDS {
        let fixture = Fixture::new(kind, true);
        let before = fixture.current();
        let runtime = before.workspace().state().runtime.as_ref().expect("runtime");
        assert!(runtime.ssh.is_none());
        let mut provider = Provider::witnessed_delete(kind, [Ok(Observation::Absent)]);
        let store = fixture.store.clone();
        let initial = before.clone();
        provider.hook = Some(Arc::new(move |action| {
            let current = current(&store);
            if current == initial {
                assert_eq!(kind, CloudProvider::RunPod);
                assert_eq!(action, "observe", "preflight precedes intent");
                return;
            }
            let runtime = current.workspace().state().runtime.as_ref().expect("runtime");
            assert!(matches!(runtime.phase, RemoteRuntimePhase::DeleteRequested { .. }));
            assert_eq!(
                runtime.cleanup.as_ref().expect("intent").reason,
                RemoteCleanupReason::WorkspaceRemoved
            );
        }));
        let result = fixture.invoke(Operation::Delete, &provider).expect("delete");
        assert!(result.absence_verified);
        assert_eq!(provider.deletion_calls(), ["delete", "observe"]);
        let after = fixture.current();
        assert_eq!(after.workflow(), before.workflow());
        assert_eq!(after.workspace().state().spec, before.workspace().state().spec);
        assert_eq!(
            after.workspace().state().runtime.as_ref().expect("runtime").worker,
            runtime.worker
        );
        assert_eq!(after.workspace().revision(), before.workspace().revision() + 2);
        assert_eq!(result.saved, after.workspace().environment_summary());
        let bytes = fixture.bytes();
        let repeated =
            with_provider::<Provider>(&fixture.store, &fixture.config, &result.saved, Operation::Check, |_| {
                panic!("no tombstone factory")
            })
            .expect("historical tombstone");
        assert_eq!(repeated, result);
        assert_eq!(
            confirm_configured_remote_environment_deletion(&fixture.store, &fixture.config, &result.saved),
            Ok(result)
        );
        assert_eq!(fixture.bytes(), bytes);
        assert_eq!(
            fixture.refuse_before_factory(Operation::Retry),
            Error::Delete(RemoteEnvironmentDeleteError::MissingDeleteIntent)
        );
    }
}

#[test]
fn present_or_lost_delete_reply_retains_intent_and_check_never_replays() {
    for kind in KINDS {
        let fixture = Fixture::new(kind, true);
        let mut provider = Provider::witnessed_delete(kind, [Ok(Observation::Present), Ok(Observation::Present)]);
        provider.fail_delete = true;
        assert!(
            !fixture
                .invoke(Operation::Delete, &provider)
                .expect("pending")
                .absence_verified
        );
        let pending = fixture.current();
        assert!(
            !fixture
                .invoke(Operation::Check, &provider)
                .expect("pending check")
                .absence_verified
        );
        assert_eq!(fixture.current(), pending);
        assert_eq!(provider.deletion_calls(), ["delete", "observe", "observe"]);
        assert_eq!(
            fixture.refuse_before_factory(Operation::Delete),
            Error::Delete(RemoteEnvironmentDeleteError::ManagementConflict)
        );
    }
}

#[test]
fn observation_errors_preserve_intent_and_do_not_leak_provider_payload() {
    for kind in KINDS {
        let fixture = Fixture::new(kind, true);
        let provider = Provider::witnessed_delete(kind, [Err(()), Err(())]);
        let error = fixture.invoke(Operation::Delete, &provider).expect_err("unverified");
        assert_eq!(error, Error::Delete(RemoteEnvironmentDeleteError::ProviderUnavailable));
        assert!(!format!("{error:?} {error}").contains("synthetic-secret"));
        let pending = fixture.current();
        assert_eq!(fixture.invoke(Operation::Retry, &provider), Err(error));
        assert_eq!(fixture.current(), pending);
        assert_eq!(provider.deletion_calls(), ["delete", "observe", "observe"]);
    }
}

#[test]
fn explicit_retry_observes_first_and_only_present_allows_one_cas_guarded_delete() {
    for kind in KINDS {
        for present in [true, false] {
            let fixture = Fixture::new(kind, true);
            fixture.pending();
            let before = fixture.current();
            let observations = if present {
                vec![Ok(Observation::Present), Ok(Observation::Absent)]
            } else {
                vec![Ok(Observation::Absent)]
            };
            let provider = Provider::new(kind, observations);
            let result = fixture.invoke(Operation::Retry, &provider);
            if !present && kind == CloudProvider::RunPod {
                assert_eq!(result, Err(Error::UnverifiedRunPodContext));
                assert_eq!(fixture.current(), before);
                assert_eq!(provider.calls(), ["observe"]);
                continue;
            }
            let result = result.expect("retry");
            assert!(result.absence_verified);
            assert_eq!(
                provider.calls(),
                if present {
                    vec!["observe", "delete", "observe"]
                } else {
                    vec!["observe"]
                }
            );
            assert!(matches!(
                result.saved.saved_phase,
                Some(RemoteRuntimePhase::Deleted {
                    requested_at_millis: 1,
                    ..
                })
            ));
            assert_eq!(
                fixture.current().workspace().revision(),
                before.workspace().revision() + if present { 2 } else { 1 }
            );
        }
    }
}

#[test]
fn binding_or_full_workflow_drift_during_io_cannot_save_completion() {
    for kind in KINDS {
        for action in ["delete", "observe"] {
            for binding in [false, true] {
                let fixture = Fixture::new(kind, true);
                let mut provider = Provider::witnessed_delete(kind, [Ok(Observation::Absent)]);
                let store = fixture.store.clone();
                provider.hook = Some(Arc::new(move |called| {
                    if called == action
                        && matches!(
                            current(&store)
                                .workspace()
                                .state()
                                .runtime
                                .as_ref()
                                .expect("runtime")
                                .phase,
                            RemoteRuntimePhase::DeleteRequested { .. }
                        )
                    {
                        if binding {
                            drift_binding(&store, kind);
                        } else {
                            drift_workflow(&store);
                        }
                    }
                }));
                assert_eq!(
                    fixture.invoke(Operation::Delete, &provider),
                    Err(Error::Delete(RemoteEnvironmentDeleteError::StateChanged))
                );
                assert!(matches!(
                    fixture.summary().saved_phase,
                    Some(RemoteRuntimePhase::DeleteRequested { .. })
                ));
                assert_eq!(
                    provider.deletion_calls(),
                    if action == "delete" {
                        vec!["delete"]
                    } else {
                        vec!["delete", "observe"]
                    }
                );
            }
        }
    }
}

#[test]
fn unbound_runpod_absence_or_error_preflight_cannot_send_delete_or_record_intent() {
    for observation in [Ok(Observation::Absent), Err(())] {
        let fixture = Fixture::new(CloudProvider::RunPod, true);
        let before = fixture.bytes();
        let mut wrong_context = Provider::new(CloudProvider::RunPod, [observation]);
        wrong_context.fail_delete = true;
        assert_eq!(
            fixture.invoke(Operation::Delete, &wrong_context),
            Err(Error::UnverifiedRunPodContext)
        );
        assert_eq!(wrong_context.calls(), ["observe"]);
        assert_eq!(fixture.bytes(), before);
    }
}

#[test]
fn owned_present_then_failed_or_already_absent_delete_and_404_never_complete() {
    for operation in [Operation::Delete, Operation::Retry] {
        for already_absent in [false, true] {
            let fixture = Fixture::new(CloudProvider::RunPod, true);
            if operation == Operation::Retry {
                fixture.pending();
            }
            let mut provider = Provider::new(
                CloudProvider::RunPod,
                [Ok(Observation::Present), Ok(Observation::Absent)],
            );
            provider.fail_delete = !already_absent;
            provider.already_absent = already_absent;
            let before = fixture.current();
            assert_eq!(
                fixture.invoke(operation, &provider),
                Err(Error::UnverifiedRunPodContext)
            );
            assert_eq!(provider.calls(), ["observe", "delete", "observe"]);
            assert!(matches!(
                fixture.summary().saved_phase,
                Some(RemoteRuntimePhase::DeleteRequested { .. })
            ));
            assert_eq!(
                fixture.current().workspace().revision(),
                before.workspace().revision() + 1
            );
        }
    }
}

#[test]
fn failed_delete_then_fresh_wrong_context_404_never_completes_or_reuses_a_witness() {
    let fixture = Fixture::new(CloudProvider::RunPod, true);
    let mut original = Provider::witnessed_delete(CloudProvider::RunPod, [Ok(Observation::Present)]);
    original.fail_delete = true;
    assert!(
        !fixture
            .invoke(Operation::Delete, &original)
            .expect("pending")
            .absence_verified
    );
    assert_eq!(original.deletion_calls(), ["delete", "observe"]);
    let before = fixture.bytes();
    for operation in [Operation::Check, Operation::Retry] {
        let wrong_context = Provider::new(CloudProvider::RunPod, [Ok(Observation::Absent)]);
        assert_eq!(
            fixture.invoke(operation, &wrong_context),
            Err(Error::UnverifiedRunPodContext)
        );
        assert_eq!(wrong_context.calls(), ["observe"]);
        assert_eq!(fixture.bytes(), before);
    }
}

#[test]
fn runpod_preflight_rechecks_allocation_and_binding_before_intent_even_on_error() {
    for binding in [false, true] {
        for observation in [Ok(Observation::Present), Err(())] {
            let fixture = Fixture::new(CloudProvider::RunPod, true);
            let before = fixture.current();
            let store = fixture.store.clone();
            let mut provider = Provider::new(CloudProvider::RunPod, [observation]);
            provider.hook = Some(Arc::new(move |_| {
                if binding {
                    drift_binding(&store, CloudProvider::RunPod);
                } else {
                    drift_workflow(&store);
                }
            }));
            assert_eq!(
                fixture.invoke(Operation::Delete, &provider),
                Err(Error::Delete(RemoteEnvironmentDeleteError::StateChanged))
            );
            assert_eq!(provider.calls(), ["observe"]);
            assert_eq!(fixture.current().workspace(), before.workspace());
        }
    }
}

#[test]
fn check_and_retry_refuse_drift_before_completion_or_second_mutation() {
    for operation in [Operation::Check, Operation::Retry] {
        let fixture = Fixture::new(CloudProvider::Azure, true);
        fixture.pending();
        let mut provider = Provider::new(CloudProvider::Azure, [Ok(Observation::Present)]);
        let store = fixture.store.clone();
        provider.hook = Some(Arc::new(move |_| {
            drift_workflow(&store);
        }));
        assert_eq!(
            fixture.invoke(operation, &provider),
            Err(Error::Delete(RemoteEnvironmentDeleteError::StateChanged))
        );
        assert_eq!(provider.calls(), ["observe"]);
        assert_eq!(
            fixture.summary().saved_phase,
            Some(RemoteRuntimePhase::DeleteRequested { requested_at_millis: 1 })
        );
    }
}
