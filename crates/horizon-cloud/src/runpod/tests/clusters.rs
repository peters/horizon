use super::*;

fn member(spec: &WorkerSpec) -> Value {
    let mut pod = worker(spec);
    pod["cluster"] = json!({"id":"cluster1","rank":0});
    pod
}

#[test]
fn cluster_members_cannot_be_adopted_from_lifecycle_lists() {
    for original in [CreateState::Prepared, CreateState::Requested] {
        let spec = spec();
        let (provider, requests, task) = server(vec![(200, pods(&json!([member(&spec)])))]);
        let mut state = original.clone();
        let result = provider.ensure(
            &spec,
            &mut state,
            &Cancellation::default(),
            |_| panic!("cluster membership must not be adopted"),
            |_| {},
        );
        assert!(matches!(result, Err(CloudError::Invalid(_))));
        assert_eq!(state, original);
        task.join().unwrap();
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("GET /pods?includeClusterPods=false "));
    }
}

#[test]
fn a_cluster_member_hint_cannot_resolve_an_uncertain_creation() {
    let spec = spec();
    let (provider, requests, task) = server(vec![(200, pods(&json!([]))), (200, member(&spec).to_string())]);
    let mut state = CreateState::Requested;
    let result = provider.reconcile(&spec, &mut state, Some("worker1"), &Cancellation::default(), |_| {
        panic!("cluster hint must not clear the fence")
    });
    assert!(matches!(result, Err(CloudError::Invalid(_))));
    assert_eq!(state, CreateState::Requested);
    task.join().unwrap();
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.starts_with("GET "))
    );
}

#[test]
fn cluster_members_are_refused_before_lifecycle_mutations() {
    for action in ["start", "stop", "terminate"] {
        let spec = spec();
        let (provider, requests, task) = server(vec![(200, member(&spec).to_string())]);
        let cancel = Cancellation::default();
        let mut state = CreateState::Bound {
            worker_id: "worker1".into(),
        };
        let original = state.clone();
        let result = match action {
            "start" => provider.start(&spec, "worker1", &cancel),
            "stop" => provider.stop(&spec, "worker1", &cancel),
            _ => provider.terminate(&spec, &mut state, &cancel, |_| panic!("must retain bound identity")),
        };
        assert!(matches!(result, Err(CloudError::Invalid(_))));
        assert_eq!(state, original);
        task.join().unwrap();
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("GET /pods/worker1 "));
    }
}

#[test]
fn unexpected_cluster_membership_in_a_create_response_keeps_the_fence() {
    let spec = spec();
    let (provider, requests, task) = server(vec![(200, pods(&json!([]))), (201, member(&spec).to_string())]);
    let mut state = CreateState::Prepared;
    let mut transitions = Vec::new();
    let result = provider.ensure(
        &spec,
        &mut state,
        &Cancellation::default(),
        |next| {
            transitions.push(next.clone());
            Ok(())
        },
        |_| {},
    );
    assert!(matches!(result, Err(CloudError::CreationUnresolved)));
    assert_eq!(state, CreateState::Requested);
    assert_eq!(transitions, vec![CreateState::Requested]);
    task.join().unwrap();
    assert_eq!(
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.starts_with("POST "))
            .count(),
        1
    );
}

#[test]
fn an_unusable_cluster_marker_does_not_certify_standalone_ownership() {
    for marker in [Value::Null, json!(false), json!({})] {
        let mut pod = worker(&spec());
        pod["cluster"] = marker;
        assert!(matches!(wire::worker(pod), Err(CloudError::Invalid(_))));
    }
}
