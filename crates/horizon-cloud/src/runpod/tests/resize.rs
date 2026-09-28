use super::*;
use crate::runpod::{
    resize::Replacement,
    volumes::{Tier, Volume},
};

fn current() -> WorkerSpec {
    let mut spec = spec();
    spec.profile.gpu = false;
    spec.profile.cpu = 2;
    spec.profile.memory_gb = 4;
    spec.profile.storage.volume_gb = 20;
    spec.cpu_flavors = vec!["cpu3c".into()];
    spec
}
fn next() -> WorkerSpec {
    let mut spec = current();
    spec.profile.cpu = 4;
    spec.profile.memory_gb = 8;
    spec
}
fn volume() -> Volume {
    Volume {
        id: "volume1".into(),
        name: "horizon-volume-test-operation".into(),
        size: 20,
        data_center_id: "test-region".into(),
        tier: Some(Tier::Standard),
    }
}
fn intent() -> Replacement {
    Replacement::new(current(), next(), "worker1".into(), volume()).unwrap()
}
fn observed(spec: &WorkerSpec, id: &str) -> Value {
    let mut value = worker(spec);
    value["id"] = json!(id);
    value["cpu"] = json!({"vcpuCount":spec.profile.cpu,"memory":spec.profile.memory_gb});
    value["dataCenterId"] = json!("test-region");
    value["mounts"] = json!({"network":[{"volumeId":"volume1","path":"/workspace"}]});
    value
}
fn empty_attachments() -> Vec<(u16, String)> {
    vec![(200, endpoints(&json!([]))), (200, pods(&json!([])))]
}
fn prefix(old: &Value) -> Vec<(u16, String)> {
    let mut rows = vec![
        (200, serde_json::to_string(&volume()).unwrap()),
        (200, old.to_string()),
        (200, old.to_string()),
    ];
    rows.extend(empty_attachments());
    rows
}
fn termination(old: &Value) -> Vec<(u16, String)> {
    vec![(200, old.to_string()), (204, String::new()), (404, String::new())]
}
fn creation() -> Vec<(u16, String)> {
    let mut rows = empty_attachments();
    rows.extend([
        (200, pods(&json!([]))),
        (201, observed(&next(), "worker2").to_string()),
        (200, observed(&next(), "worker2").to_string()),
    ]);
    rows.extend(empty_attachments());
    rows
}
fn provider(rows: Vec<(u16, String)>) -> (RunPod, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
    let (mut provider, requests, task) = server(rows);
    provider.api_endpoint = provider.endpoint.clone();
    (provider, requests, task)
}

#[test]
fn resizing_retains_the_exact_volume_and_fences_delete_and_create() {
    let old = observed(&current(), "worker1");
    let mut rows = prefix(&old);
    rows.extend(termination(&old));
    rows.extend(creation());
    let (provider, requests, task) = provider(rows);
    let mut intent = intent();
    let mut journal = Vec::new();
    let result = provider
        .resize_cpu(
            &mut intent,
            &Cancellation::default(),
            |next| {
                journal.push(serde_json::to_value(next).unwrap());
                Ok(())
            },
            |_| {},
        )
        .unwrap();
    task.join().unwrap();
    assert_eq!(result.id, "worker2");
    assert!(intent.completed());
    assert_eq!(result.network_volume.unwrap().id.as_deref(), Some("volume1"));
    assert_eq!(
        journal.iter().map(|s| s["phase"].as_str().unwrap()).collect::<Vec<_>>(),
        ["terminating", "creating", "creating", "creating", "completed"]
    );
    let requests = requests.lock().unwrap();
    let mutations = requests
        .iter()
        .filter(|r| r.starts_with("DELETE ") || r.starts_with("POST "))
        .collect::<Vec<_>>();
    assert_eq!(mutations.len(), 2);
    assert!(mutations[0].starts_with("DELETE /pods/worker1 "));
    let body: Value = serde_json::from_str(mutations[1].split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["mounts"]["network"][0]["volumeId"], "volume1");
}

#[test]
fn completed_journal_recovers_without_replacing_again_when_the_observation_was_lost() {
    let old = observed(&current(), "worker1");
    let replacement = observed(&next(), "worker2");
    let mut rows = prefix(&old);
    rows.extend(termination(&old));
    rows.extend(creation());
    rows.extend([
        (200, serde_json::to_string(&volume()).unwrap()),
        (200, replacement.to_string()),
        (200, replacement.to_string()),
    ]);
    rows.extend(empty_attachments());
    let (provider, requests, task) = provider(rows);
    let mut journal = Vec::new();
    let mut intent = intent();
    // The caller crashes after Completed was persisted, before storing the returned worker.
    drop(
        provider
            .resize_cpu(
                &mut intent,
                &Cancellation::default(),
                |next| {
                    journal = serde_json::to_vec(next).unwrap();
                    Ok(())
                },
                |_| {},
            )
            .unwrap(),
    );
    drop(intent);
    let before_retry = requests.lock().unwrap().len();
    let mut recovered: Replacement = serde_json::from_slice(&journal).unwrap();
    assert!(recovered.completed());
    let worker = provider
        .resize_cpu(&mut recovered, &Cancellation::default(), |_| Ok(()), |_| {})
        .unwrap();
    task.join().unwrap();
    assert_eq!(worker.id, "worker2");
    recovered.verify_result(&worker).unwrap();
    assert!(recovered.completed());
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len() - before_retry, 5);
    assert!(
        requests[before_retry..]
            .iter()
            .all(|request| request.starts_with("GET "))
    );
    assert_eq!(
        requests.iter().filter(|request| request.starts_with("DELETE ")).count(),
        1
    );
    assert_eq!(
        requests.iter().filter(|request| request.starts_with("POST ")).count(),
        1
    );
}

#[test]
fn legacy_standard_volume_can_resize_only_after_live_tier_confirmation() {
    let mut legacy = volume();
    legacy.tier = None;
    let old = observed(&current(), "worker1");
    let mut rows = prefix(&old);
    rows.extend(termination(&old));
    rows.extend(creation());
    let (provider, _, task) = provider(rows);
    let mut intent = Replacement::new(current(), next(), "worker1".into(), legacy.clone()).unwrap();
    let worker = provider
        .resize_cpu(&mut intent, &Cancellation::default(), |_| Ok(()), |_| {})
        .unwrap();
    task.join().unwrap();
    intent.verify_origin(&current(), "worker1", &legacy).unwrap();
    intent.verify_result(&worker).unwrap();

    let mut premium = volume();
    premium.tier = Some(Tier::HighPerformance);
    let (provider, requests, task) = self::provider(vec![(200, serde_json::to_string(&premium).unwrap())]);
    let mut intent = Replacement::new(current(), next(), "worker1".into(), legacy).unwrap();
    assert!(matches!(
        provider.resize_cpu(
            &mut intent,
            &Cancellation::default(),
            |_| panic!("No mutation authority"),
            |_| {}
        ),
        Err(CloudError::IdentityMismatch)
    ));
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
fn stopped_original_refuses_before_authorizing_a_prepared_replacement() {
    let mut old = observed(&current(), "worker1");
    old["status"] = json!("EXITED");
    let (provider, requests, task) = provider(prefix(&old));
    let mut intent = intent();
    assert!(matches!(
        provider.resize_cpu(
            &mut intent,
            &Cancellation::default(),
            |_| panic!("No mutation authority"),
            |_| {}
        ),
        Err(CloudError::Invalid("The original worker is not running"))
    ));
    task.join().unwrap();
    assert!(intent.unstarted());
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.starts_with("GET "))
    );
}

#[test]
fn persisted_termination_can_finish_after_the_original_worker_stops() {
    let mut old = observed(&current(), "worker1");
    old["status"] = json!("EXITED");
    let mut rows = prefix(&old);
    rows.extend(termination(&old));
    rows.extend(creation());
    let (provider, _, task) = provider(rows);
    let mut saved = serde_json::to_value(intent()).unwrap();
    saved["phase"] = json!("terminating");
    let mut intent = serde_json::from_value(saved).unwrap();
    assert!(
        provider
            .resize_cpu(&mut intent, &Cancellation::default(), |_| Ok(()), |_| {})
            .is_ok()
    );
    task.join().unwrap();
    assert!(intent.completed());
}

#[test]
fn persistence_failure_never_terminates_and_a_foreign_mount_never_grants_authority() {
    let old = observed(&current(), "worker1");
    let (provider, requests, task) = provider(prefix(&old));
    assert!(matches!(
        provider.resize_cpu(
            &mut intent(),
            &Cancellation::default(),
            |_| Err(CloudError::Persistence),
            |_| {}
        ),
        Err(CloudError::Persistence)
    ));
    task.join().unwrap();
    assert!(requests.lock().unwrap().iter().all(|r| r.starts_with("GET ")));
    let mut changed = old.clone();
    changed["mounts"]["network"][0]["volumeId"] = json!("foreign");
    let rows = vec![
        (200, serde_json::to_string(&volume()).unwrap()),
        (200, old.to_string()),
        (200, changed.to_string()),
    ];
    let (provider, requests, task) = self::provider(rows);
    assert!(
        provider
            .resize_cpu(
                &mut intent(),
                &Cancellation::default(),
                |_| panic!("unexpected journal"),
                |_| {}
            )
            .is_err()
    );
    task.join().unwrap();
    assert!(requests.lock().unwrap().iter().all(|r| r.starts_with("GET ")));
}

#[test]
fn uncertain_create_recovers_without_another_post() {
    let old = observed(&current(), "worker1");
    let mut rows = prefix(&old);
    rows.extend(termination(&old));
    rows.extend(empty_attachments());
    rows.extend([(200, pods(&json!([]))), (503, String::new())]);
    rows.extend([
        (200, serde_json::to_string(&volume()).unwrap()),
        (200, pods(&json!([observed(&next(), "worker2")]))),
        (200, observed(&next(), "worker2").to_string()),
    ]);
    rows.extend(empty_attachments());
    let (provider, requests, task) = provider(rows);
    let mut intent = intent();
    let mut saved = Vec::new();
    assert!(
        provider
            .resize_cpu(
                &mut intent,
                &Cancellation::default(),
                |next| {
                    saved = serde_json::to_vec(next).unwrap();
                    Ok(())
                },
                |_| {}
            )
            .is_err()
    );
    intent = serde_json::from_slice(&saved).unwrap();
    assert_eq!(
        provider
            .resize_cpu(&mut intent, &Cancellation::default(), |_| Ok(()), |_| {})
            .unwrap()
            .id,
        "worker2"
    );
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
fn image_disk_location_and_gpu_changes_are_rejected_before_provider_work() {
    for change in 0..4 {
        let mut next = next();
        match change {
            0 => next.image_digest = format!("example/worker@sha256:{}", "b".repeat(64)),
            1 => next.profile.storage.volume_gb = 40,
            2 => next.data_centers = vec!["elsewhere".into()],
            _ => next.profile.gpu = true,
        }
        assert!(Replacement::new(current(), next, "worker1".into(), volume()).is_err());
    }
}

#[test]
fn lost_delete_response_recovers_from_absence_before_creating() {
    let old = observed(&current(), "worker1");
    let mut rows = prefix(&old);
    rows.extend([(200, old.to_string()), (503, String::new())]);
    rows.extend([
        (200, serde_json::to_string(&volume()).unwrap()),
        (404, String::new()),
        (404, String::new()),
    ]);
    rows.extend(creation());
    let (provider, requests, task) = provider(rows);
    let mut intent = intent();
    let mut saved = Vec::new();
    assert!(
        provider
            .resize_cpu(
                &mut intent,
                &Cancellation::default(),
                |next| {
                    saved = serde_json::to_vec(next).unwrap();
                    Ok(())
                },
                |_| {}
            )
            .is_err()
    );
    intent = serde_json::from_slice(&saved).unwrap();
    assert_eq!(
        provider
            .resize_cpu(&mut intent, &Cancellation::default(), |_| Ok(()), |_| {})
            .unwrap()
            .id,
        "worker2"
    );
    task.join().unwrap();
    assert_eq!(
        requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.starts_with("DELETE "))
            .count(),
        1
    );
}

#[test]
fn failing_creation_journal_never_allocates_after_termination() {
    let old = observed(&current(), "worker1");
    let mut rows = prefix(&old);
    rows.extend(termination(&old));
    rows.extend(empty_attachments());
    rows.push((200, pods(&json!([]))));
    let (provider, requests, task) = provider(rows);
    assert!(matches!(
        provider.resize_cpu(
            &mut intent(),
            &Cancellation::default(),
            |next| {
                if serde_json::to_value(next).unwrap()["creation"]
                    == serde_json::to_value(CreateState::Requested).unwrap()
                {
                    Err(CloudError::Persistence)
                } else {
                    Ok(())
                }
            },
            |_| {}
        ),
        Err(CloudError::Persistence)
    ));
    task.join().unwrap();
    assert!(requests.lock().unwrap().iter().all(|r| !r.starts_with("POST ")));
}
