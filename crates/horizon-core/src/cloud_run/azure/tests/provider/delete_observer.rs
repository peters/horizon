use super::*;
use crate::cloud_run::{
    interactive_worker::InteractiveWorkerLease,
    interactive_worker_delete::{InteractiveWorkerDeleteObserver, InteractiveWorkerDeletionObservation as Observation},
};

fn client(s: &Scenario) -> AzureClient {
    AzureClient::with_transport(
        profile(),
        s.plane.clone(),
        |_: CloudWorkflowId, _: CloudJobId, _: &WorkerTarget, _: &str| {
            panic!("deletion observation must not consult the creation fence")
        },
        |_: &AzureWorker, _: &str, _: &str| {
            panic!("deletion observation must not obtain a host key or execute a guest command")
        },
    )
    .expect("client")
    .on_clock(s.clock.clone())
}

fn assert_one_group_read(s: &Scenario) {
    assert_eq!(s.plane.calls(), [Call::GetGroup(s.group.clone())]);
    assert!(s.clock.sleeps().is_empty(), "no polling or retry");
}

#[test]
fn surviving_group_is_present_regardless_of_compute_or_deletion_state() {
    let s = Scenario::new();
    let client = client(&s);
    for state in ["Succeeded", "Deleting", "Creating", "Failed", "unknown"] {
        for power in [None, Some("running"), Some("deallocated")] {
            s.plane.script(
                Some(group_info(&s.group, s.tags.clone(), state)),
                Some(deployment("Succeeded", "203.0.113.9")),
                vec![power.map(|power| vm(power, &s.tags))],
            );
            assert_eq!(
                client.observe_worker_deletion(&s.persisted()),
                Ok(Observation::Present),
                "{state}, {power:?}"
            );
            assert_one_group_read(&s);
        }
    }
}

#[test]
fn only_group_absence_is_absent_and_a_read_is_not_retried() {
    let s = Scenario::new();
    let client = client(&s);
    s.plane.script(None, None, vec![Some(vm("running", &s.tags))]);
    s.plane.lock().appear_on_second_lookup = Some(owned(&s));
    assert_eq!(client.observe_worker_deletion(&s.persisted()), Ok(Observation::Absent));
    assert_one_group_read(&s);
    // The observation is point-in-time, not a promise that the scope stays absent.
    s.plane.script(Some(owned(&s)), None, vec![None]);
    s.plane.lock().vanish_after_first_lookup = true;
    assert_eq!(client.observe_worker_deletion(&s.persisted()), Ok(Observation::Present));
    assert_one_group_read(&s);
}

#[test]
fn every_owned_group_tag_is_required_and_must_match() {
    let s = Scenario::new();
    let client = client(&s);
    for key in s.tags.keys() {
        for missing in [false, true] {
            let mut group = owned(&s);
            if missing {
                group.tags.remove(key);
            } else {
                group.tags.insert(key.clone(), "other-owner-or-target".into());
            }
            s.plane.script(Some(group), None, vec![None]);
            assert_eq!(client.observe_worker_deletion(&s.persisted()), Err(MISMATCH), "{key}");
            assert_one_group_read(&s);
        }
    }
    let mut group = owned(&s);
    group.tags.insert("unrelated-label".into(), "kept".into());
    s.plane.script(Some(group), None, vec![None]);
    assert_eq!(client.observe_worker_deletion(&s.persisted()), Ok(Observation::Present));
    assert_one_group_read(&s);
}

#[test]
fn observed_group_id_must_equal_the_saved_group_not_just_match_tags() {
    let s = Scenario::new();
    let client = client(&s);
    for id in [
        format!("/subscriptions/{OTHER_SUB}/resourceGroups/{}", s.group),
        format!("/subscriptions/{SUB}/resourceGroups/another-group"),
        format!("{}/providers/Microsoft.Compute/virtualMachines/worker", s.group_id),
        String::new(),
    ] {
        let mut group = owned(&s);
        group.id = id;
        s.plane.script(Some(group), None, vec![None]);
        assert_eq!(client.observe_worker_deletion(&s.persisted()), Err(MISMATCH));
        assert_one_group_read(&s);
    }
    let mut group = owned(&s);
    group.id.make_ascii_uppercase();
    s.plane.script(Some(group), None, vec![None]);
    assert_eq!(client.observe_worker_deletion(&s.persisted()), Ok(Observation::Present));
    assert_one_group_read(&s);
}

#[test]
fn malformed_or_foreign_saved_handles_are_rejected_before_any_lookup() {
    let s = Scenario::new();
    let client = client(&s);
    let worker = s.persisted();
    let mut foreign = worker.clone();
    foreign.identity.provider = CloudProvider::RunPod;
    let mut subscription = worker.clone();
    subscription.identity.resource_id = format!("/subscriptions/{OTHER_SUB}/resourceGroups/{}", s.group);
    let mut group = worker.clone();
    group.identity.resource_id = format!("/subscriptions/{SUB}/resourceGroups/other");
    let mut malformed = worker.clone();
    malformed.identity.resource_id.push_str("?query=not-an-arm-id");
    let mut key = worker.clone();
    key.ssh_public_key = "ssh-rsa invalid".into();
    let mut image = worker.clone();
    image.target.image = "registry.example/worker:latest".into();
    let mut timed = worker;
    timed.target.lifetime = WorkerLifetime::TimeLimited { seconds: 60 };
    timed.lifetime = InteractiveWorkerLifetime::TimeLimited(InteractiveWorkerLease {
        terminate_after: "2026-01-01T00:00:00Z".into(),
    });
    for invalid in [foreign, subscription, group, malformed, key, image, timed] {
        assert_eq!(
            client.observe_worker_deletion(&invalid),
            Err(AzureError::InvalidPersistedWorker)
        );
        assert!(s.plane.calls().is_empty(), "invalid handle cannot even look up absence");
    }
}

#[test]
fn lookup_failures_are_errors_not_absence_and_never_retry() {
    let s = Scenario::new();
    let client = client(&s);
    let operation = "resource group lookup";
    let mut failures: Vec<_> = [401, 403, 408, 429, 500, 503]
        .into_iter()
        .map(|status| AzureError::UnexpectedStatus { operation, status })
        .collect();
    failures.extend([
        AzureError::RequestFailed { operation },
        AzureError::InvalidResponse { operation },
        AzureError::OperationTimedOut { operation },
        AzureError::CredentialUnavailable {
            reason: "synthetic refusal",
        },
    ]);
    for error in failures {
        s.plane.script(None, None, vec![None]);
        s.plane.lock().group_error = Some(error.clone());
        assert_eq!(client.observe_worker_deletion(&s.persisted()), Err(error));
        assert_one_group_read(&s);
    }
}
