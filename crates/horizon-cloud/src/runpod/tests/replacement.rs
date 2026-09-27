use super::*;
use crate::{
    ImageSide,
    runpod::{
        replacement::{Observed, may_have_applied},
        volumes::Volume,
    },
};

const OTHER_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICAhIiMkJSYnKCkqKywtLi8wMTIzNDU2Nzg5Ojs8PT4/";

fn next(current: &WorkerSpec) -> WorkerSpec {
    WorkerSpec {
        image_digest: format!("example/worker@sha256:{}", "b".repeat(64)),
        ..current.clone()
    }
}
fn third() -> String {
    format!("example/worker@sha256:{}", "c".repeat(64))
}
/// A pod as the provider reports it, on `on`'s image, carrying the environment
/// created for `current`.
fn pod(current: &WorkerSpec, on: &WorkerSpec) -> Value {
    let mut value = worker(current);
    value["image"] = json!(on.image_digest);
    value["env"] = json!({"HORIZON_CLOUD_OPERATION": current.operation_id, "PUBLIC_KEY": current.public_key});
    value
}
fn body(request: &str) -> Value {
    serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}
fn methods(requests: &Mutex<Vec<String>>) -> Vec<String> {
    requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.split(" HTTP/").next().unwrap().to_owned())
        .collect()
}
const INSPECT: &str = "GET /pods/worker1";
const UPDATE: &str = "PATCH /pods/worker1";
const MOUNTS: &str = "GET /pods/worker1";

#[test]
fn update_sends_only_the_new_image_to_a_verified_running_worker() {
    let current = spec();
    let next = next(&current);
    // A worker that already reports the new image is updated again when continuing.
    for on in [&current, &next] {
        let (provider, requests, task) = server(vec![(200, pod(&current, on).to_string()), (200, "{}".into())]);
        provider
            .replace_image(&current, &next, "worker1", &Cancellation::default())
            .unwrap();
        task.join().unwrap();
        assert_eq!(methods(&requests), [INSPECT, UPDATE]);
        assert_eq!(body(&requests.lock().unwrap()[1]), json!({"image": next.image_digest}));
    }
}

#[test]
fn registry_credential_is_sent_only_when_it_changes() {
    let mut current = spec();
    for (previous, replacement, sent) in [
        (None, Some("auth-new"), Some("auth-new")),
        (Some("auth-old"), Some("auth-new"), Some("auth-new")),
        (Some("auth-old"), Some("auth-old"), None),
        (None, None, None),
    ] {
        current.registry_auth_id = previous.map(Into::into);
        let mut next = next(&current);
        next.registry_auth_id = replacement.map(Into::into);
        let (provider, requests, task) = server(vec![(200, pod(&current, &current).to_string()), (200, "{}".into())]);
        provider
            .replace_image(&current, &next, "worker1", &Cancellation::default())
            .unwrap();
        task.join().unwrap();
        let mut expected = json!({"image": next.image_digest});
        if let Some(id) = sent {
            expected["registry"] = json!(id);
        }
        assert_eq!(body(&requests.lock().unwrap()[1]), expected);
    }
    current.registry_auth_id = Some("auth-old".into());
    let mut next = next(&current);
    next.registry_auth_id = None;
    let (provider, requests, task) = server(vec![]);
    assert!(matches!(
        provider.replace_image(&current, &next, "worker1", &Cancellation::default()),
        Err(CloudError::Invalid(
            "An image replacement cannot remove the registry credential"
        ))
    ));
    task.join().unwrap();
    assert!(requests.lock().unwrap().is_empty());
}

#[test]
fn definite_refusals_stay_distinguishable_from_uncertain_updates() {
    let current = spec();
    let next = next(&current);
    let inspected = pod(&current, &current).to_string();
    for status in [400, 422, 401, 403, 404, 405, 409, 429, 500, 502, 503] {
        let (provider, requests, task) = server(vec![(200, inspected.clone()), (status, "{}".into())]);
        let error = provider
            .replace_image(&current, &next, "worker1", &Cancellation::default())
            .unwrap_err();
        task.join().unwrap();
        assert_eq!(methods(&requests), [INSPECT, UPDATE]);
        match status {
            400 | 422 => assert!(matches!(error, CloudError::Rejected(_))),
            401 | 403 => assert!(matches!(error, CloudError::Unauthorized)),
            _ => assert!(matches!(error, CloudError::Http(code, _) if code == status)),
        }
        assert_eq!(may_have_applied(&error), status >= 500, "HTTP {status}");
    }
    // The provider applied the update, but its response never arrived.
    let (mut provider, requests, task) = delayed_server(
        vec![(200, inspected), (200, "{}".into())],
        Some((1, Duration::from_millis(1500))),
    );
    provider.agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_millis(500)))
            .http_status_as_error(false)
            .max_redirects(0)
            .build(),
    );
    let error = provider
        .replace_image(&current, &next, "worker1", &Cancellation::default())
        .unwrap_err();
    assert!(matches!(error, CloudError::Transport));
    assert!(may_have_applied(&error));
    task.join().unwrap();
    assert_eq!(methods(&requests), [INSPECT, UPDATE]);
}

#[test]
fn a_failed_read_before_the_update_is_definite() {
    let current = spec();
    let next = next(&current);
    let unsent = |error: &CloudError| {
        matches!(
            error,
            CloudError::Invalid("Could not confirm the worker before switching its image; no update was sent")
        ) && !may_have_applied(error)
    };
    // The server failed the read.
    let (provider, requests, task) = server(vec![(503, "{}".into())]);
    let error = provider
        .replace_image(&current, &next, "worker1", &Cancellation::default())
        .unwrap_err();
    task.join().unwrap();
    assert!(unsent(&error), "{error}");
    assert_eq!(methods(&requests), [INSPECT]);
    // The read timed out in transit.
    let (mut provider, requests, task) = delayed_server(
        vec![(200, pod(&current, &current).to_string())],
        Some((0, Duration::from_millis(1500))),
    );
    provider.agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_millis(500)))
            .http_status_as_error(false)
            .max_redirects(0)
            .build(),
    );
    let error = provider
        .replace_image(&current, &next, "worker1", &Cancellation::default())
        .unwrap_err();
    task.join().unwrap();
    assert!(unsent(&error), "{error}");
    assert_eq!(methods(&requests), [INSPECT]);
}

#[test]
fn refuses_to_update_a_stopped_lost_or_unverified_worker() {
    let current = spec();
    let next = next(&current);
    let unverified: fn(&CloudError) -> bool = |error| matches!(error, CloudError::IdentityMismatch);
    let mut cases = Vec::new();
    let mut stopped = pod(&current, &current);
    stopped["status"] = json!("EXITED");
    let not_running: fn(&CloudError) -> bool = |error| {
        matches!(
            error,
            CloudError::Invalid("Worker must be running to replace its image")
        )
    };
    cases.push((200, stopped, not_running));
    let mut renamed = pod(&current, &current);
    renamed["name"] = json!("unrelated-worker");
    cases.push((200, renamed, unverified));
    let mut unknown = pod(&current, &current);
    unknown["image"] = json!(third());
    cases.push((200, unknown, unverified));
    let mut foreign = pod(&current, &current);
    foreign["env"]["HORIZON_CLOUD_OPERATION"] = json!("other-operation");
    cases.push((200, foreign, unverified));
    let mut unmarked = pod(&current, &current);
    unmarked["env"] = json!({});
    cases.push((200, unmarked, unverified));
    cases.push((404, json!({}), |error| matches!(error, CloudError::WorkerLost)));
    for (status, inspected, expected) in cases {
        let (provider, requests, task) = server(vec![(status, inspected.to_string())]);
        let error = provider
            .replace_image(&current, &next, "worker1", &Cancellation::default())
            .unwrap_err();
        task.join().unwrap();
        assert!(expected(&error), "{error} for {inspected}");
        assert!(!may_have_applied(&error));
        assert_eq!(methods(&requests), [INSPECT], "{inspected}");
    }
    let cancel = Cancellation::default();
    cancel.cancel();
    let (provider, requests, task) = server(vec![]);
    let error = provider.replace_image(&current, &next, "worker1", &cancel).unwrap_err();
    assert!(matches!(error, CloudError::Cancelled));
    assert!(!may_have_applied(&error));
    task.join().unwrap();
    assert!(requests.lock().unwrap().is_empty());
}

fn identity_changes(current: &WorkerSpec) -> Vec<WorkerSpec> {
    let edits: [fn(&mut WorkerSpec); 9] = [
        |spec| spec.operation_id = "other-operation".into(),
        |spec| spec.profile.cpu += 1,
        |spec| spec.profile.memory_gb += 1,
        |spec| spec.profile.storage.volume_gb += 1,
        |spec| spec.profile.capabilities.desktop = !spec.profile.capabilities.desktop,
        |spec| spec.public_key = OTHER_KEY.into(),
        |spec| spec.gpu_types = vec!["NVIDIA RTX A5000".into()],
        |spec| spec.cpu_flavors = vec!["cpu5c".into()],
        |spec| spec.data_centers = vec!["EU-TEST-1".into()],
    ];
    edits
        .into_iter()
        .map(|edit| {
            let mut spec = next(current);
            edit(&mut spec);
            spec
        })
        .collect()
}

#[test]
fn only_the_image_and_its_credential_may_change() {
    let current = spec();
    for changed in identity_changes(&current) {
        assert!(matches!(
            current.verify_replacement(&changed),
            Err(CloudError::IdentityChange)
        ));
        let (provider, requests, task) = server(vec![]);
        assert!(matches!(
            provider.replace_image(&current, &changed, "worker1", &Cancellation::default()),
            Err(CloudError::IdentityChange)
        ));
        assert!(matches!(
            provider.observe_image(
                "worker1",
                &current,
                &changed,
                None,
                &Cancellation::default(),
                Duration::from_secs(2)
            ),
            Err(CloudError::IdentityChange)
        ));
        task.join().unwrap();
        assert!(requests.lock().unwrap().is_empty());
    }
    let same = current.clone();
    let (provider, requests, task) = server(vec![]);
    assert!(matches!(
        provider.replace_image(&current, &same, "worker1", &Cancellation::default()),
        Err(CloudError::Invalid("Replacement image is the worker's current image"))
    ));
    let mut mutable = next(&current);
    mutable.image_digest = "example/worker:latest".into();
    assert!(matches!(
        provider.replace_image(&current, &mutable, "worker1", &Cancellation::default()),
        Err(CloudError::Invalid(_))
    ));
    task.join().unwrap();
    assert!(requests.lock().unwrap().is_empty());
}

#[test]
fn either_image_verifies_while_plain_verification_stays_strict() {
    let current = spec();
    let next = next(&current);
    let parse = |value: Value| wire::worker(value).unwrap();
    let previous = parse(pod(&current, &current));
    let replaced = parse(pod(&current, &next));
    assert_eq!(previous.verify_either(&current, &next).unwrap(), ImageSide::Previous);
    assert_eq!(replaced.verify_either(&current, &next).unwrap(), ImageSide::Next);
    replaced.verify(&next).unwrap();
    assert!(matches!(replaced.verify(&current), Err(CloudError::IdentityMismatch)));
    assert!(matches!(previous.verify(&next), Err(CloudError::IdentityMismatch)));
    // Like `verify`, an absent operation marker is tolerated; observation is stricter.
    let mut unmarked = pod(&current, &next);
    unmarked["env"] = json!({});
    assert_eq!(parse(unmarked).verify_either(&current, &next).unwrap(), ImageSide::Next);
    let mut rejected = Vec::new();
    for (field, value) in [("image", json!(third())), ("name", json!("unrelated-worker"))] {
        let mut changed = pod(&current, &next);
        changed[field] = value;
        rejected.push(changed);
    }
    let mut foreign = pod(&current, &next);
    foreign["env"]["HORIZON_CLOUD_OPERATION"] = json!("other-operation");
    rejected.push(foreign);
    for value in rejected {
        assert!(matches!(
            parse(value).verify_either(&current, &next),
            Err(CloudError::IdentityMismatch)
        ));
    }
    for changed in identity_changes(&current) {
        assert!(matches!(
            replaced.verify_either(&current, &changed),
            Err(CloudError::IdentityChange)
        ));
    }
    assert!(matches!(
        previous.verify_either(&current, &current),
        Err(CloudError::Invalid(_))
    ));
}

fn observe(
    provider: &RunPod,
    current: &WorkerSpec,
    next: &WorkerSpec,
    volume: Option<&Volume>,
) -> Result<Observed, CloudError> {
    provider.observe_image(
        "worker1",
        current,
        next,
        volume,
        &Cancellation::default(),
        Duration::from_secs(2),
    )
}

#[test]
fn observation_reports_the_recorded_image_of_the_pair() {
    let current = spec();
    let next = next(&current);
    let mut restarting = pod(&current, &next);
    restarting["ssh"]["direct"] = Value::Null;
    for (inspected, expected) in [
        (pod(&current, &current), Observed::Previous),
        (pod(&current, &next), Observed::Next),
        (restarting, Observed::Next),
    ] {
        let (provider, requests, task) = server(vec![(200, inspected.to_string())]);
        assert_eq!(observe(&provider, &current, &next, None).unwrap(), expected);
        task.join().unwrap();
        assert_eq!(methods(&requests), [INSPECT]);
    }
}

#[test]
fn observation_fails_closed_on_a_third_image_foreign_owner_or_lost_worker() {
    let current = spec();
    let next = next(&current);
    let mut unknown = pod(&current, &next);
    unknown["image"] = json!(third());
    let mut unmarked = pod(&current, &next);
    unmarked["env"] = json!({});
    let mut foreign = pod(&current, &next);
    foreign["env"]["HORIZON_CLOUD_OPERATION"] = json!("other-operation");
    for inspected in [unknown, unmarked, foreign] {
        let (provider, requests, task) = server(vec![(200, inspected.to_string())]);
        assert!(matches!(
            observe(&provider, &current, &next, None),
            Err(CloudError::IdentityMismatch)
        ));
        task.join().unwrap();
        assert_eq!(methods(&requests), [INSPECT]);
    }
    let (provider, requests, task) = server(vec![(404, "{}".into())]);
    assert!(matches!(
        observe(&provider, &current, &next, None),
        Err(CloudError::WorkerLost)
    ));
    task.join().unwrap();
    assert_eq!(methods(&requests), [INSPECT]);
    let (provider, requests, task) = server(vec![]);
    assert!(matches!(
        provider.observe_image(
            "worker1",
            &current,
            &next,
            None,
            &Cancellation::default(),
            Duration::ZERO
        ),
        Err(CloudError::Transport)
    ));
    task.join().unwrap();
    assert!(requests.lock().unwrap().is_empty());
}

mod mounted {
    use super::*;

    fn volume(current: &WorkerSpec) -> Volume {
        Volume {
            id: "volume1".into(),
            name: format!("horizon-volume-{}", current.operation_id),
            size: u32::from(current.profile.storage.volume_gb),
            data_center_id: "EU-TEST-1".into(),
            tier: Some(crate::runpod::volumes::Tier::Standard),
        }
    }
    fn current_api(current: &WorkerSpec, image: &str) -> Value {
        json!({"id": "worker1", "name": current.name(), "image": image, "dataCenterId": "EU-TEST-1",
            "mounts": {"network": [{"volumeId": "volume1", "path": "/workspace"}]}})
    }
    fn server(responses: Vec<(u16, String)>) -> (RunPod, Arc<Mutex<Vec<String>>>, thread::JoinHandle<()>) {
        let (mut provider, requests, task) = super::server(responses);
        provider.api_endpoint.clone_from(&provider.endpoint);
        (provider, requests, task)
    }

    #[test]
    fn consecutive_reads_must_report_the_same_image_before_replacement_settles() {
        let current = spec();
        let next = next(&current);
        let volume = volume(&current);
        for (listed, mounted, expected) in [
            (&current, &current, Observed::Previous),
            (&next, &next, Observed::Next),
            (&next, &current, Observed::Unsettled),
            (&current, &next, Observed::Unsettled),
        ] {
            let (provider, requests, task) = server(vec![
                (200, pod(&current, listed).to_string()),
                (200, current_api(&current, &mounted.image_digest).to_string()),
            ]);
            assert_eq!(observe(&provider, &current, &next, Some(&volume)).unwrap(), expected);
            task.join().unwrap();
            assert_eq!(methods(&requests), [INSPECT, MOUNTS]);
        }
    }

    #[test]
    fn a_third_image_or_different_mount_on_the_current_api_fails_closed() {
        let current = spec();
        let next = next(&current);
        let volume = volume(&current);
        let mut remounted = current_api(&current, &next.image_digest);
        remounted["mounts"]["network"][0]["volumeId"] = json!("volume2");
        let mut relocated = current_api(&current, &next.image_digest);
        relocated["dataCenterId"] = json!("EU-TEST-2");
        for mounted in [current_api(&current, &third()), remounted, relocated] {
            let (provider, requests, task) = server(vec![
                (200, pod(&current, &next).to_string()),
                (200, mounted.to_string()),
            ]);
            assert!(matches!(
                observe(&provider, &current, &next, Some(&volume)),
                Err(CloudError::IdentityMismatch)
            ));
            task.join().unwrap();
            assert_eq!(methods(&requests), [INSPECT, MOUNTS]);
        }
        let mut unknown = pod(&current, &next);
        unknown["image"] = json!(third());
        let (provider, requests, task) = server(vec![(200, unknown.to_string())]);
        assert!(matches!(
            observe(&provider, &current, &next, Some(&volume)),
            Err(CloudError::IdentityMismatch)
        ));
        task.join().unwrap();
        assert_eq!(methods(&requests), [INSPECT]);
        let mut foreign = volume.clone();
        foreign.name = "horizon-volume-other-operation".into();
        let (provider, requests, task) = server(vec![]);
        assert!(matches!(
            observe(&provider, &current, &next, Some(&foreign)),
            Err(CloudError::IdentityMismatch)
        ));
        task.join().unwrap();
        assert!(requests.lock().unwrap().is_empty());
    }
}
