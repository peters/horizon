use super::*;

#[test]
fn public_entrypoints_refuse_unconfigured_profiles_and_unsupported_provider_without_writes() {
    for kind in KINDS {
        let fixture = Fixture::new(kind, true);
        for operation in [Operation::Delete, Operation::Check, Operation::Retry] {
            if operation != Operation::Delete
                && fixture.summary().saved_phase != Some(RemoteRuntimePhase::DeleteRequested { requested_at_millis: 1 })
            {
                fixture.pending();
            }
            let before = fixture.bytes();
            let entrypoint = match operation {
                Operation::Delete => delete_configured_remote_environment,
                Operation::Check => confirm_configured_remote_environment_deletion,
                Operation::Retry => retry_configured_remote_environment_deletion,
            };
            assert!(matches!(
                entrypoint(&fixture.store, &RemoteProviderConfig::default(), &fixture.summary()),
                Err(Error::Configuration(_))
            ));
            let mut foreign = fixture.summary();
            foreign.provider = CloudProvider::LocalDocker;
            assert_eq!(
                execute(&fixture.store, &fixture.config, &foreign, operation),
                Err(Error::UnsupportedProvider)
            );
            assert_eq!(fixture.bytes(), before);
        }
    }
}

#[test]
fn time_limited_workers_cannot_enter_configured_delete() {
    for kind in KINDS {
        let fixture = Fixture::build(kind, false, None, WorkerLifetime::TimeLimited { seconds: 900 });
        assert_eq!(
            fixture.refuse_before_factory(Operation::Delete),
            Error::Delete(RemoteEnvironmentDeleteError::UnsupportedLifetime)
        );
    }
}

#[test]
fn missing_binding_and_profile_drift_precede_credentials_for_every_operation() {
    for kind in KINDS {
        for operation in [Operation::Delete, Operation::Check, Operation::Retry] {
            for missing in [true, false] {
                let mut fixture = Fixture::new(kind, !missing);
                if operation != Operation::Delete {
                    fixture.pending();
                }
                if !missing {
                    if kind == CloudProvider::RunPod {
                        fixture.config.runpod[0].data_center_id = Some("other-dc".into());
                    } else {
                        fixture.config.azure[0].vm_size = "Standard_D8s_v3".into();
                    }
                }
                assert_eq!(fixture.refuse_before_factory(operation), Error::InvalidBinding);
            }
        }
    }
}

#[test]
fn summary_drift_and_wrong_owner_never_construct_a_provider() {
    let fixture = Fixture::new(CloudProvider::RunPod, true);
    for change in 0..4 {
        let mut expected = fixture.summary();
        match change {
            0 => expected.revision += 1,
            1 => expected.repository = "other/repository".into(),
            2 => expected.worker_identity.as_mut().expect("identity").resource_id = "other-worker".into(),
            _ => expected.owning_session_id = "00000000-0000-4000-8000-000000000002".into(),
        }
        let before = fixture.bytes();
        assert!(
            with_provider::<Provider>(
                &fixture.store,
                &fixture.config,
                &expected,
                Operation::Delete,
                |_| panic!("no factory")
            )
            .is_err()
        );
        assert_eq!(fixture.bytes(), before);
    }
}

#[test]
fn competing_management_and_future_delete_intent_refuse_before_factory() {
    for kind in KINDS {
        let fixture = Fixture::new(kind, true);
        for operation in [Operation::Check, Operation::Retry] {
            assert_eq!(
                fixture.refuse_before_factory(operation),
                Error::Delete(RemoteEnvironmentDeleteError::MissingDeleteIntent)
            );
        }
        fixture.pinned();
        for phase in [
            RemoteRuntimePhase::Stopping { requested_at_millis: 1 },
            RemoteRuntimePhase::Starting { requested_at_millis: 1 },
        ] {
            if matches!(phase, RemoteRuntimePhase::Starting { .. }) {
                let stopped = fixture
                    .store
                    .record_remote_stop_phase(
                        &fixture.current(),
                        RemoteRuntimePhase::Stopped {
                            requested_at_millis: 1,
                            observed_at_millis: 1,
                        },
                    )
                    .expect("saved Stop");
                fixture
                    .store
                    .record_remote_start_phase(&stopped, phase)
                    .expect("Start intent");
            } else {
                fixture
                    .store
                    .record_remote_stop_phase(&fixture.current(), phase)
                    .expect("Stop intent");
            }
            assert_eq!(
                fixture.refuse_before_factory(Operation::Delete),
                Error::Delete(RemoteEnvironmentDeleteError::ManagementConflict)
            );
        }
        let future = Fixture::new(kind, true);
        future
            .store
            .record_remote_delete_phase(
                &future.current(),
                RemoteRuntimePhase::DeleteRequested {
                    requested_at_millis: i64::MAX,
                },
            )
            .expect("future fixture intent");
        for operation in [Operation::Check, Operation::Retry] {
            assert_eq!(
                future.refuse_before_factory(operation),
                Error::Delete(RemoteEnvironmentDeleteError::InvalidTimestamp)
            );
        }
    }
}

#[test]
fn complete_provider_handle_is_validated_before_credentials_even_without_a_pin() {
    for (kind, resource) in [
        (CloudProvider::RunPod, "invalid/pod"),
        (CloudProvider::Azure, "incomplete-handle"),
        (
            CloudProvider::Azure,
            "/subscriptions/11111111-1111-4111-8111-111111111112/resourceGroups/other-group",
        ),
        (
            CloudProvider::Azure,
            "/subscriptions/11111111-1111-4111-8111-111111111111/resourceGroups/other-group",
        ),
    ] {
        let fixture = Fixture::with_resource(kind, true, Some(resource));
        assert_eq!(fixture.refuse_before_factory(Operation::Delete), Error::InvalidBinding);
        fixture.pending();
        assert_eq!(fixture.refuse_before_factory(Operation::Check), Error::InvalidBinding);
        assert_eq!(fixture.refuse_before_factory(Operation::Retry), Error::InvalidBinding);
    }
}

#[test]
fn legacy_cleanup_is_not_a_delete_retry_and_existing_delete_is_not_a_first_delete() {
    let fixture = Fixture::new(CloudProvider::RunPod, true);
    let before = fixture.current();
    let mut state = before.workspace().state().clone();
    let runtime = state.runtime.as_mut().expect("runtime");
    runtime.cleanup = Some(RemoteCleanupIntent {
        reason: RemoteCleanupReason::ApplicationExit,
        requested_at_millis: 1,
    });
    runtime.phase = RemoteRuntimePhase::Deleting;
    fixture
        .store
        .replace_remote_workspace(before.workspace(), &state)
        .expect("legacy cleanup");
    assert_eq!(
        fixture.refuse_before_factory(Operation::Delete),
        Error::Delete(RemoteEnvironmentDeleteError::ManagementConflict)
    );
    assert_eq!(
        fixture.refuse_before_factory(Operation::Retry),
        Error::Delete(RemoteEnvironmentDeleteError::MissingDeleteIntent)
    );
    let pending = Fixture::new(CloudProvider::Azure, true);
    pending.pending();
    assert_eq!(
        pending.refuse_before_factory(Operation::Delete),
        Error::Delete(RemoteEnvironmentDeleteError::ManagementConflict)
    );
}

#[test]
fn credential_failure_is_redacted_nonwriting_and_factory_drift_wins_over_failure() {
    for kind in KINDS {
        for drift in [false, true] {
            let fixture = Fixture::new(kind, true);
            let before = fixture.current();
            let result = with_provider::<Provider>(
                &fixture.store,
                &fixture.config,
                &fixture.summary(),
                Operation::Delete,
                |_| {
                    if drift {
                        drift_workflow(&fixture.store);
                    }
                    Err(Error::CredentialUnavailable)
                },
            );
            assert_eq!(
                result,
                Err(if drift {
                    Error::Delete(RemoteEnvironmentDeleteError::StateChanged)
                } else {
                    Error::CredentialUnavailable
                })
            );
            assert_eq!(fixture.current().workspace(), before.workspace());
        }
    }
}

#[test]
fn lazy_factory_cannot_change_binding_or_supply_a_different_provider() {
    for kind in KINDS {
        for binding_drift in [true, false] {
            let fixture = Fixture::new(kind, true);
            let before = fixture.current();
            let result = with_provider(
                &fixture.store,
                &fixture.config,
                &fixture.summary(),
                Operation::Delete,
                |_| {
                    if binding_drift {
                        drift_binding(&fixture.store, kind);
                    }
                    Ok(Provider::new(CloudProvider::LocalDocker, []))
                },
            );
            assert_eq!(
                result,
                Err(if binding_drift {
                    Error::Delete(RemoteEnvironmentDeleteError::StateChanged)
                } else {
                    Error::InvalidBinding
                })
            );
            assert_eq!(fixture.current(), before);
        }
    }
}
