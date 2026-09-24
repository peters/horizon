use super::*;
use crate::runpod::registry::{Binding, PullBinding, State};

fn binding() -> Binding {
    Binding {
        id: "pull1".into(),
        name: "horizon-pull-generation1".into(),
    }
}

#[test]
fn prepared_generation_cannot_adopt_an_existing_provider_binding() {
    let (provider, requests, task) = server(vec![(200, serde_json::to_string(&vec![binding()]).unwrap())]);
    let credential = Credential::new("synthetic-pull-secret".into()).unwrap();
    let input = PullBinding {
        operation_id: "generation1",
        username: "pull-user",
        credential: &credential,
    };
    let mut state = State::Prepared;
    assert!(matches!(
        provider.ensure_registry_binding(&input, &mut state, &Cancellation::default(), |_| {
            panic!("An unrelated provider binding must not change the journal")
        }),
        Err(CloudError::IdentityMismatch)
    ));
    assert_eq!(state, State::Prepared);
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /containerregistryauth "));
}

#[test]
fn prepared_revocation_requires_absence_and_never_deletes_an_unowned_binding() {
    for response in [
        (200, "[]".into()),
        (200, serde_json::to_string(&vec![binding()]).unwrap()),
        (503, "{}".into()),
    ] {
        let absent = response.1 == "[]";
        let (provider, requests, task) = server(vec![response]);
        let mut state = State::Prepared;
        let mut transitions = Vec::new();
        let result = provider.revoke_registry_binding("generation1", &mut state, &Cancellation::default(), |next| {
            transitions.push(next.clone());
            Ok(())
        });
        if absent {
            assert!(result.is_ok());
            assert_eq!(state, State::Revoked);
            assert_eq!(transitions, [State::Revoked]);
        } else {
            assert!(result.is_err());
            assert_eq!(state, State::Prepared);
            assert!(transitions.is_empty());
        }
        task.join().unwrap();
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("GET /containerregistryauth "));
    }
}

#[test]
fn missing_delete_response_remains_fenced_until_absence_is_observed() {
    let result = binding();
    let listed = serde_json::to_string(&vec![&result]).unwrap();
    let (provider, requests, task) = server(vec![
        (200, listed),
        (404, r#"{"error":"synthetic-pull-secret"}"#.into()),
        (200, "[]".into()),
    ]);
    let mut state = State::Bound(result.clone());
    let error = provider
        .revoke_registry_binding("generation1", &mut state, &Cancellation::default(), |_| Ok(()))
        .unwrap_err();
    assert!(!format!("{error:?} {error}").contains("synthetic-pull-secret"));
    assert_eq!(state, State::Revoking(result));
    provider
        .reconcile_registry_binding("generation1", &mut state, &Cancellation::default(), |_| Ok(()))
        .unwrap();
    assert_eq!(state, State::Revoked);
    task.join().unwrap();
    assert_eq!(requests.lock().unwrap().len(), 3);
}

#[test]
fn creation_persists_intent_before_sending_only_the_explicit_pull_credential() {
    let result = binding();
    let (provider, requests, task) = server(vec![(200, "[]".into()), (200, serde_json::to_string(&result).unwrap())]);
    let credential = Credential::new("synthetic-pull-secret".into()).unwrap();
    let input = PullBinding {
        operation_id: "generation1",
        username: "pull-user",
        credential: &credential,
    };
    let mut state = State::Prepared;
    let mut transitions = Vec::new();
    assert_eq!(
        provider
            .ensure_registry_binding(&input, &mut state, &Cancellation::default(), |next| {
                if matches!(next, State::Requested { .. }) {
                    assert_eq!(requests.lock().unwrap().len(), 1);
                }
                transitions.push(next.clone());
                Ok(())
            })
            .unwrap(),
        result
    );
    task.join().unwrap();
    assert_eq!(
        transitions,
        [
            State::Requested {
                name: "horizon-pull-generation1".into()
            },
            State::Bound(result)
        ]
    );
    let requests = requests.lock().unwrap();
    assert!(requests[1].starts_with("POST /containerregistryauth "));
    assert!(requests[1].contains("synthetic-pull-secret"));
    assert!(requests[1].contains("pull-user"));
}

#[test]
fn uncertain_creation_reconciles_without_reposting_even_after_empty_observation() {
    let result = binding();
    let (provider, requests, task) = server(vec![
        (200, "[]".into()),
        (500, r#"{"error":"synthetic-pull-secret"}"#.into()),
        (200, "[]".into()),
        (200, serde_json::to_string(&vec![&result]).unwrap()),
    ]);
    let credential = Credential::new("synthetic-pull-secret".into()).unwrap();
    let input = PullBinding {
        operation_id: "generation1",
        username: "pull-user",
        credential: &credential,
    };
    let mut state = State::Prepared;
    let error = provider
        .ensure_registry_binding(&input, &mut state, &Cancellation::default(), |_| Ok(()))
        .unwrap_err();
    assert!(!format!("{error:?} {error}").contains("synthetic-pull-secret"));
    assert_eq!(
        state,
        State::Requested {
            name: "horizon-pull-generation1".into()
        }
    );
    assert!(
        provider
            .ensure_registry_binding(&input, &mut state, &Cancellation::default(), |_| Ok(()))
            .is_err()
    );
    assert_eq!(
        state,
        State::Requested {
            name: "horizon-pull-generation1".into()
        }
    );
    provider
        .reconcile_registry_binding("generation1", &mut state, &Cancellation::default(), |_| Ok(()))
        .unwrap();
    assert_eq!(state, State::Bound(result));
    task.join().unwrap();
    assert_eq!(
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.starts_with("POST"))
            .count(),
        1
    );
}

#[test]
fn failed_intent_persistence_prevents_create() {
    let (provider, requests, task) = server(vec![(200, "[]".into())]);
    let credential = Credential::new("synthetic-pull-secret".into()).unwrap();
    let input = PullBinding {
        operation_id: "generation1",
        username: "pull-user",
        credential: &credential,
    };
    let mut state = State::Prepared;
    assert!(matches!(
        provider.ensure_registry_binding(&input, &mut state, &Cancellation::default(), |_| Err(
            CloudError::Persistence
        )),
        Err(CloudError::Persistence)
    ));
    assert_eq!(state, State::Prepared);
    task.join().unwrap();
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[test]
fn revocation_fences_uncertain_delete_and_confirms_absence_on_reconcile() {
    let result = binding();
    let listed = serde_json::to_string(&vec![&result]).unwrap();
    let (provider, requests, task) = server(vec![(200, listed), (500, "{}".into()), (200, "[]".into())]);
    let mut state = State::Bound(result.clone());
    assert!(
        provider
            .revoke_registry_binding("generation1", &mut state, &Cancellation::default(), |_| Ok(()))
            .is_err()
    );
    assert_eq!(state, State::Revoking(result));
    provider
        .reconcile_registry_binding("generation1", &mut state, &Cancellation::default(), |_| Ok(()))
        .unwrap();
    assert_eq!(state, State::Revoked);
    task.join().unwrap();
    assert_eq!(
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.starts_with("DELETE"))
            .count(),
        1
    );
}

#[test]
fn duplicate_or_replaced_bindings_cannot_be_selected_or_deleted() {
    let result = binding();
    for listed in [
        serde_json::to_string(&vec![&result, &result]).unwrap(),
        serde_json::to_string(&vec![Binding {
            id: "unrelated".into(),
            ..result.clone()
        }])
        .unwrap(),
    ] {
        let (provider, requests, task) = server(vec![(200, listed)]);
        let mut state = State::Bound(result.clone());
        assert!(matches!(
            provider.revoke_registry_binding("generation1", &mut state, &Cancellation::default(), |_| Ok(())),
            Err(CloudError::IdentityMismatch)
        ));
        task.join().unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1);
        assert_eq!(state, State::Bound(result.clone()));
    }
}

#[test]
fn cancellation_and_revoked_generations_never_create_again() {
    let (provider, requests, task) = server(vec![]);
    let credential = Credential::new("synthetic-pull-secret".into()).unwrap();
    let input = PullBinding {
        operation_id: "generation1",
        username: "pull-user",
        credential: &credential,
    };
    let cancel = Cancellation::default();
    cancel.cancel();
    let mut state = State::Prepared;
    assert!(matches!(
        provider.ensure_registry_binding(&input, &mut state, &cancel, |_| Ok(())),
        Err(CloudError::Cancelled)
    ));
    state = State::Revoked;
    assert!(
        provider
            .ensure_registry_binding(&input, &mut state, &Cancellation::default(), |_| Ok(()))
            .is_err()
    );
    task.join().unwrap();
    assert!(requests.lock().unwrap().is_empty());
}

#[test]
fn missing_uncertain_create_cannot_be_declared_revoked() {
    let (provider, requests, task) = server(vec![(200, "[]".into())]);
    let mut state = State::Requested {
        name: "horizon-pull-generation1".into(),
    };
    assert!(
        provider
            .revoke_registry_binding("generation1", &mut state, &Cancellation::default(), |_| Ok(()))
            .is_err()
    );
    assert_eq!(
        state,
        State::Requested {
            name: "horizon-pull-generation1".into()
        }
    );
    task.join().unwrap();
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[test]
fn another_operation_cannot_reconcile_or_revoke_a_recorded_generation() {
    let (provider, requests, task) = server(vec![]);
    for mut state in [
        State::Bound(binding()),
        State::Revoking(binding()),
        State::Requested {
            name: "horizon-pull-generation1".into(),
        },
    ] {
        let original = state.clone();
        assert!(matches!(
            provider.revoke_registry_binding("generation2", &mut state, &Cancellation::default(), |_| Ok(())),
            Err(CloudError::IdentityMismatch)
        ));
        assert_eq!(state, original);
        assert!(matches!(
            provider.reconcile_registry_binding("generation2", &mut state, &Cancellation::default(), |_| Ok(())),
            Err(CloudError::IdentityMismatch)
        ));
        assert_eq!(state, original);
    }
    task.join().unwrap();
    assert!(requests.lock().unwrap().is_empty());
}

#[test]
fn a_renamed_live_binding_cannot_be_reported_as_revoked() {
    let renamed = Binding {
        name: "renamed".into(),
        ..binding()
    };
    for mut state in [State::Bound(binding()), State::Revoking(binding())] {
        let (provider, requests, task) = server(vec![(200, serde_json::to_string(&vec![&renamed]).unwrap())]);
        let original = state.clone();
        assert!(matches!(
            provider.revoke_registry_binding("generation1", &mut state, &Cancellation::default(), |_| Ok(())),
            Err(CloudError::IdentityMismatch)
        ));
        assert_eq!(state, original);
        task.join().unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1);
    }
}
