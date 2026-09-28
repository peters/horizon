use super::*;

#[test]
fn power_actions_announce_only_after_the_identity_check_and_before_the_request() {
    let spec = spec();
    let (provider, requests, task) = server(vec![(200, worker(&spec).to_string()), (200, String::new())]);
    let announced = std::cell::Cell::new(false);
    provider
        .stop_announced(&spec, "worker1", &Cancellation::default(), || {
            assert_eq!(
                requests.lock().unwrap().len(),
                1,
                "after the identity check, before the POST"
            );
            announced.set(true);
            Ok(())
        })
        .unwrap();
    task.join().unwrap();
    assert!(announced.get());
    assert!(requests.lock().unwrap()[1].starts_with("POST /pods/worker1/action "));
    // A failed identity check announces nothing; a refused announcement sends nothing.
    let mut wrong = worker(&spec);
    wrong["name"] = json!("unrelated-worker");
    let (provider, _, task) = server(vec![(200, wrong.to_string())]);
    let result = provider.start_announced(&spec, "worker1", &Cancellation::default(), || {
        panic!("announced after a failed identity check")
    });
    assert!(matches!(result, Err(CloudError::IdentityMismatch)));
    task.join().unwrap();
    let (provider, requests, task) = server(vec![(200, worker(&spec).to_string())]);
    let result = provider.stop_announced(&spec, "worker1", &Cancellation::default(), || {
        Err(CloudError::Persistence)
    });
    assert!(matches!(result, Err(CloudError::Persistence)));
    task.join().unwrap();
    assert_eq!(
        requests.lock().unwrap().len(),
        1,
        "no POST after a refused announcement"
    );
}
