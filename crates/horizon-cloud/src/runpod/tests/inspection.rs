use super::*;
use crate::runpod::volumes::{Spec, State, Volume};

fn volume_spec() -> Spec {
    Spec {
        operation_id: spec().operation_id,
        size: 80,
        data_center_id: "test-region".into(),
    }
}
fn volume() -> Volume {
    Volume {
        id: "owned-volume".into(),
        name: volume_spec().name(),
        size: 80,
        data_center_id: "test-region".into(),
        tier: Some(crate::runpod::volumes::Tier::Standard),
    }
}
fn volume_body() -> String {
    json!({"id":"owned-volume","name":volume_spec().name(),"size":80,"dataCenter":"test-region","type":"STANDARD"})
        .to_string()
}

#[test]
fn any_serverless_endpoint_blocks_storage_admission_and_deletion() {
    // Current mounts or zero active workers cannot exclude a stale/scaled-down worker.
    for endpoint in [
        json!({"id":"endpoint1","networkVolumes":[]}),
        json!({"id":"endpoint1","networkVolumes":["owned-volume"],"workers":{"min":0,"max":0}}),
    ] {
        for prepared in [true, false] {
            let response = endpoints(&json!([endpoint]));
            let responses = if prepared {
                vec![(200, response)]
            } else {
                vec![(200, volume_body()), (200, response)]
            };
            let (provider, requests, task) = server(responses);
            let mut state = if prepared {
                State::Prepared
            } else {
                State::Bound {
                    volume: volume(),
                    creation: None,
                }
            };
            let original = state.clone();
            let result = if prepared {
                provider
                    .ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| {
                        panic!("no mutation is allowed")
                    })
                    .map(|_| ())
            } else {
                provider.terminate_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| {
                    panic!("no mutation is allowed")
                })
            };
            assert!(matches!(result,Err(CloudError::Invalid(message)) if message.contains("serverless endpoints")));
            assert_eq!(state, original);
            task.join().unwrap();
            assert!(requests.lock().unwrap().iter().all(|r| r.starts_with("GET ")));
        }
    }
}

#[test]
fn endpoint_on_a_later_page_blocks_admission_even_after_an_empty_page() {
    let first = json!({"endpoints":[],"pagination":{"hasNextPage":true,"nextCursor":"next/+?"}}).to_string();
    let (provider, requests, task) = server(vec![(200, first), (200, endpoints(&json!([{"id":"later"}])))]);
    let mut state = State::Prepared;
    assert!(
        provider
            .ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| panic!(
                "must not persist"
            ))
            .is_err()
    );
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[1].starts_with("GET /serverless?cursor=next%2F%2B%3F "));
    assert_eq!(state, State::Prepared);
}

#[test]
fn uncertain_serverless_visibility_never_allows_storage_mutation() {
    for response in [
        (403, "{}".into()),
        (503, "{}".into()),
        (200, "{}".into()),
        (200, json!({"endpoints":[]}).to_string()),
    ] {
        let (provider, requests, task) = server(vec![response]);
        let mut state = State::Prepared;
        assert!(
            provider
                .ensure_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| panic!(
                    "must not persist"
                ))
                .is_err()
        );
        assert_eq!(state, State::Prepared);
        task.join().unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1);
    }
}

#[test]
fn cluster_pod_attachment_prevents_storage_deletion() {
    let mut attached = worker(&spec());
    attached["cluster"] = json!({"id":"cluster1"});
    attached["mounts"] = json!({"network":[{"volumeId":"owned-volume","path":"/workspace"}]});
    let (provider, requests, task) = server(vec![
        (200, volume_body()),
        (200, endpoints(&json!([]))),
        (200, pods(&json!([attached]))),
    ]);
    let mut state = State::Bound {
        volume: volume(),
        creation: None,
    };
    assert!(
        provider
            .terminate_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| panic!(
                "must not delete"
            ))
            .is_err()
    );
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[2].starts_with("GET /pods?includeClusterPods=true "));
    assert!(requests.iter().all(|r| r.starts_with("GET ")));
}

#[test]
fn inspection_exposes_unrequested_mounts_for_both_compute_profiles() {
    for gpu in [false, true] {
        let mut spec = spec();
        spec.profile.gpu = gpu;
        let mut assigned = worker(&spec);
        assigned[if gpu { "gpu" } else { "cpu" }] =
            json!({"id":"fixture","vcpuCount":spec.profile.cpu,"memory":spec.profile.memory_gb,"count":1});
        assigned["mounts"] = json!({"network":[{"volumeId":"external-volume","path":"/workspace"}]});
        let (provider, requests, task) = server(vec![(200, assigned.to_string())]);
        let inspected = provider
            .inspect_with_timeout("worker1", &Cancellation::default(), Duration::from_secs(2))
            .unwrap()
            .unwrap();
        inspected.verify(&spec).unwrap();
        assert!(inspected.verify_resources(&spec).is_err());
        task.join().unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1);
    }
}

#[test]
fn unsupported_ssh_on_an_unrelated_pod_does_not_hide_its_attachment() {
    for (host, username) in [("worker.example.invalid", "root"), ("192.0.2.1", "custom-user")] {
        let mut attached = worker(&spec());
        attached["name"] = json!("unrelated");
        attached["ssh"] = json!({"direct":{"host":host,"port":22,"username":username}});
        attached["mounts"] = json!({"network":[{"volumeId":"owned-volume","path":"/workspace"}]});
        let (provider, requests, task) = server(vec![
            (200, volume_body()),
            (200, endpoints(&json!([]))),
            (200, pods(&json!([attached]))),
        ]);
        let mut state = State::Bound {
            volume: volume(),
            creation: None,
        };
        assert!(matches!(
            provider.terminate_volume(&volume_spec(), &mut state, &Cancellation::default(), |_| panic!(
                "still attached"
            )),
            Err(CloudError::Invalid(
                "Workspace volume is still attached to a worker; storage was not deleted"
            ))
        ));
        task.join().unwrap();
        assert!(requests.lock().unwrap().iter().all(|r| r.starts_with("GET ")));
    }
}
