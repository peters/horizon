use super::*;
use crate::runpod::volumes::{Spec, State, Volume};

fn volume_spec(size: u32) -> Spec {
    Spec {
        operation_id: spec().operation_id,
        size,
        data_center_id: "EU-TEST-1".into(),
    }
}
fn volume(spec: &Spec) -> Volume {
    Volume {
        id: "volume1".into(),
        name: spec.name(),
        size: spec.size,
        data_center_id: spec.data_center_id.clone(),
    }
}
fn server(responses: Vec<(u16, String)>) -> (RunPod, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
    let (mut provider, requests, task) = super::server(responses);
    provider.api_endpoint.clone_from(&provider.endpoint);
    provider.catalog_endpoint.clone_from(&provider.endpoint);
    provider.graphql_endpoint.clone_from(&provider.endpoint);
    (provider, requests, task)
}
fn assert_size_error<T: std::fmt::Debug>(result: Result<T, CloudError>) {
    let error = result.unwrap_err();
    assert!(
        matches!(
            error,
            CloudError::Invalid("CPU workspace volume must be between 10 and 4000 GB")
        ),
        "{error:?}"
    );
}

#[test]
fn invalid_sizes_fail_before_placement_worker_or_volume_io_and_leave_journals_prepared() {
    let (provider, requests, task) = server(Vec::new());
    task.join().unwrap();
    for size in [1, 9, 4001] {
        let mut worker = spec();
        worker.profile.storage.volume_gb = size;
        assert_size_error(provider.workspace_volume_spec(&worker, &Cancellation::default()));
        let mut worker_state = CreateState::Prepared;
        assert_size_error(provider.ensure(
            &worker,
            &mut worker_state,
            &Cancellation::default(),
            |_| panic!("invalid worker request must not be journaled"),
            |_| {},
        ));
        assert_eq!(worker_state, CreateState::Prepared);
        let mut volume_state = State::Prepared;
        let result = provider.ensure_volume(
            &volume_spec(u32::from(size)),
            &mut volume_state,
            &Cancellation::default(),
            |_| panic!("invalid volume request must not be journaled"),
        );
        if size > 4000 {
            // The historical structural upper bound also rejects this request.
            assert!(matches!(result, Err(CloudError::Invalid(_))));
        } else {
            assert_size_error(result);
        }
        assert_eq!(volume_state, State::Prepared);
        assert!(worker.validate().is_ok());
        worker.profile.gpu = true;
        assert!(worker.validate_request().is_ok());
    }
    assert!(requests.lock().unwrap().is_empty());
}

#[test]
fn boundary_sizes_can_allocate_and_bind() {
    for size in [10, 4000] {
        let spec = volume_spec(size);
        let expected = volume(&spec);
        let (provider, requests, task) = server(vec![
            (200, "[]".into()),
            (201, serde_json::to_string(&expected).unwrap()),
        ]);
        let mut state = State::Prepared;
        assert_eq!(
            provider
                .ensure_volume(&spec, &mut state, &Cancellation::default(), |_| Ok(()))
                .unwrap(),
            expected
        );
        assert!(matches!(&state, State::Bound { volume, creation: Some(_) } if volume == &expected));
        state.verify(&spec).unwrap();
        task.join().unwrap();
        assert_eq!(requests.lock().unwrap().len(), 2);
        assert!(requests.lock().unwrap()[1].starts_with("POST /networkvolumes "));
    }
}

#[test]
fn historical_small_uncertain_volume_stays_fenced_then_can_reconcile_and_delete() {
    let spec = volume_spec(1);
    let expected = volume(&spec);
    let (provider, requests, task) = server(vec![
        (200, "[]".into()),
        (200, json!([expected]).to_string()),
        (200, serde_json::to_string(&expected).unwrap()),
        (200, "[]".into()),
        (204, String::new()),
        (404, "{}".into()),
    ]);
    let mut state = State::Requested;
    let error = provider
        .ensure_volume(&spec, &mut state, &Cancellation::default(), |_| Ok(()))
        .unwrap_err();
    assert!(error.to_string().contains("allocation is unresolved"));
    assert_eq!(state, State::Requested);
    assert_eq!(
        provider
            .ensure_volume(&spec, &mut state, &Cancellation::default(), |_| Ok(()))
            .unwrap(),
        expected
    );
    provider
        .terminate_volume(&spec, &mut state, &Cancellation::default(), |_| Ok(()), |_| {})
        .unwrap();
    assert_eq!(state, State::Deleted);
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 6);
    assert!(requests.iter().all(|request| !request.starts_with("POST ")));
    assert_eq!(
        requests.iter().filter(|request| request.starts_with("DELETE ")).count(),
        1
    );
}
