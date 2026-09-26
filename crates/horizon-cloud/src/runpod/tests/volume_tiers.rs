use super::*;
use crate::runpod::volumes::{Spec, State, Tier, Volume};

fn volume_spec() -> Spec {
    Spec {
        operation_id: spec().operation_id,
        size: 20,
        data_center_id: "test-region".into(),
    }
}
fn volume(tier: Option<Tier>) -> Volume {
    let spec = volume_spec();
    Volume {
        id: "volume1".into(),
        name: spec.name(),
        size: spec.size,
        data_center_id: spec.data_center_id,
        tier,
    }
}
fn body(tier: Tier) -> String {
    serde_json::to_string(&volume(Some(tier))).unwrap()
}
fn cleanup_responses(tier: Tier) -> Vec<(u16, String)> {
    vec![
        (200, body(tier)),
        (200, endpoints(&json!([]))),
        (200, pods(&json!([]))),
        (204, String::new()),
        (404, String::new()),
    ]
}
fn cleanup(provider: &RunPod, state: &mut State) {
    provider
        .terminate_volume(&volume_spec(), state, &Cancellation::default(), |_| Ok(()))
        .unwrap();
    assert_eq!(*state, State::Deleted);
}

#[test]
fn wrong_tier_is_bound_without_creation_authority_and_retries_cannot_bypass_policy() {
    for direct in [false, true] {
        let mut responses = if direct {
            vec![
                (200, endpoints(&json!([]))),
                (200, volumes(&json!([]))),
                (201, body(Tier::HighPerformance)),
            ]
        } else {
            vec![(200, volumes(&json!([volume(Some(Tier::HighPerformance))])))]
        };
        responses.push((200, body(Tier::HighPerformance)));
        responses.extend(cleanup_responses(Tier::HighPerformance));
        let (provider, requests, task) = server(responses);
        let mut state = if direct { State::Prepared } else { State::Requested };
        let mut saved = Vec::new();
        for _ in 0..2 {
            assert!(matches!(
                provider.ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |next| {
                    saved = serde_json::to_vec(next).unwrap();
                    Ok(())
                }),
                Err(CloudError::Invalid(_))
            ));
            assert!(
                matches!(&state, State::Bound { volume, creation: None } if volume.tier == Some(Tier::HighPerformance))
            );
            assert!(state.creation_receipt(&volume_spec()).unwrap().is_none());
            state = serde_json::from_slice(&saved).unwrap();
        }
        let mut worker = spec();
        worker.profile.storage.volume_gb = 20;
        assert!(volume(Some(Tier::HighPerformance)).verify_worker_spec(&worker).is_err());
        cleanup(&provider, &mut state);
        task.join().unwrap();
        assert_eq!(
            requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.starts_with("POST "))
                .count(),
            usize::from(direct)
        );
    }
}

#[test]
fn cleanup_reconciles_a_requested_wrong_tier_without_admitting_it() {
    let mut responses = vec![(200, volumes(&json!([volume(Some(Tier::HighPerformance))])))];
    responses.extend(cleanup_responses(Tier::HighPerformance));
    let (provider, requests, task) = server(responses);
    cleanup(&provider, &mut State::Requested);
    task.join().unwrap();
    assert!(requests.lock().unwrap().iter().all(|r| !r.starts_with("POST ")));
}

#[test]
fn failed_tier_binding_persistence_keeps_allocation_requested_and_cleanup_can_recover() {
    for direct in [false, true] {
        let listing = volumes(&json!([volume(Some(Tier::HighPerformance))]));
        let mut responses = if direct {
            vec![
                (200, endpoints(&json!([]))),
                (200, volumes(&json!([]))),
                (201, body(Tier::HighPerformance)),
            ]
        } else {
            vec![(200, listing.clone())]
        };
        responses.push((200, listing));
        responses.extend(cleanup_responses(Tier::HighPerformance));
        let (provider, requests, task) = server(responses);
        let mut state = if direct { State::Prepared } else { State::Requested };
        assert!(matches!(
            provider.ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |next| {
                if matches!(next, State::Bound { .. }) {
                    Err(CloudError::Persistence)
                } else {
                    Ok(())
                }
            }),
            Err(CloudError::Persistence)
        ));
        assert_eq!(state, State::Requested);
        cleanup(&provider, &mut state);
        task.join().unwrap();
        assert_eq!(
            requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.starts_with("POST "))
                .count(),
            usize::from(direct)
        );
    }
}

#[test]
fn malformed_api_tier_never_grants_legacy_compatibility_or_creation_authority() {
    for tier in [None, Some(Value::Null), Some(json!("UNKNOWN"))] {
        let mut api = serde_json::to_value(volume(None)).unwrap();
        if let Some(tier) = tier {
            api["type"] = tier;
        }
        for direct in [false, true] {
            let responses = if direct {
                vec![
                    (200, endpoints(&json!([]))),
                    (200, volumes(&json!([]))),
                    (201, api.to_string()),
                ]
            } else {
                vec![(200, volumes(&json!([api])))]
            };
            let (provider, _, task) = server(responses);
            let mut state = if direct { State::Prepared } else { State::Requested };
            assert!(
                provider
                    .ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(()))
                    .is_err()
            );
            assert_eq!(state, State::Requested);
            task.join().unwrap();
        }
        let (provider, _, task) = server(vec![(200, api.to_string())]);
        let mut state = State::Bound {
            volume: volume(None),
            creation: None,
        };
        assert!(
            provider
                .ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(()))
                .is_err()
        );
        task.join().unwrap();
    }
}

#[test]
fn unrelated_high_performance_volume_does_not_block_standard_allocation() {
    let mut unrelated = volume(Some(Tier::HighPerformance));
    unrelated.id = "other-volume".into();
    unrelated.name = "unrelated".into();
    let (provider, _, task) = server(vec![
        (200, endpoints(&json!([]))),
        (200, volumes(&json!([unrelated]))),
        (201, body(Tier::Standard)),
    ]);
    let mut state = State::Prepared;
    provider
        .ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(()))
        .unwrap();
    assert!(state.creation_receipt(&volume_spec()).unwrap().is_some());
    task.join().unwrap();
}

#[test]
fn legacy_receipts_remain_unchanged_and_recoverable_for_both_observed_tiers() {
    for tier in [Tier::Standard, Tier::HighPerformance] {
        for stage in ["bound", "deleting"] {
            let legacy = volume(None);
            let original =
                json!({"state":stage,"volume":legacy,"creation":{"version":1,"spec":volume_spec(),"volume":legacy}});
            let mut state: State = serde_json::from_value(original.clone()).unwrap();
            state.verify(&volume_spec()).unwrap();
            assert_eq!(serde_json::to_value(&state).unwrap(), original);
            let mut responses = Vec::new();
            if stage == "bound" {
                responses.push((200, body(tier)));
                responses.push((200, endpoints(&json!([]))));
            }
            responses.extend(cleanup_responses(tier));
            let (provider, _, task) = server(responses);
            if stage == "bound" {
                assert_eq!(
                    provider
                        .ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| panic!(
                            "read-only inspection"
                        ))
                        .unwrap(),
                    legacy
                );
                assert_eq!(serde_json::to_value(&state).unwrap(), original);
                assert!(state.creation_receipt(&volume_spec()).unwrap().is_some());
            }
            cleanup(&provider, &mut state);
            task.join().unwrap();
        }
    }
}

#[test]
fn recorded_tier_drift_blocks_use_and_deletion() {
    let (provider, requests, task) = server(vec![(200, body(Tier::HighPerformance)); 2]);
    let mut state = State::Bound {
        volume: volume(Some(Tier::Standard)),
        creation: None,
    };
    assert!(matches!(
        provider.ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(())),
        Err(CloudError::IdentityMismatch)
    ));
    assert!(matches!(
        provider.terminate_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| Ok(())),
        Err(CloudError::IdentityMismatch)
    ));
    task.join().unwrap();
    assert!(requests.lock().unwrap().iter().all(|r| r.starts_with("GET ")));
}
