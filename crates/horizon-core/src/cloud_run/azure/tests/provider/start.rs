use super::*;
use crate::cloud_run::interactive_worker_start::{InteractiveWorkerStart, InteractiveWorkerStartProvider};

#[test]
fn start_posts_once_awaits_running_and_reobserves_through_the_readiness_path() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false, Some(host_key()));
    assert_eq!(client.start_worker(&worker), Ok(InteractiveWorkerStart::AlreadyAbsent));
    assert!(s.plane.mutations().is_empty(), "absence never allocates");
    let address = || Some(deployment("Succeeded", "203.0.113.9"));
    s.plane.script(
        Some(owned(&s)),
        address(),
        vec![
            Some(vm("deallocated", &s.tags)),
            Some(vm("starting", &s.tags)),
            Some(vm("running", &s.tags)),
        ],
    );
    let started = client.start_worker(&worker).expect("start");
    let InteractiveWorkerStart::Started(status) = &started else {
        panic!("a deallocated worker is started: {started:?}");
    };
    assert_eq!(
        status.lifecycle,
        Lifecycle::Ready,
        "ready only through the attested key"
    );
    assert_eq!(
        status.ssh.as_ref().map(|ssh| ssh.host_key.as_str()),
        Some(host_key().as_str())
    );
    assert_eq!(s.plane.mutations(), [Call::Start(s.group.clone())]);
    // Already running: observed, never re-posted.
    s.plane
        .script(Some(owned(&s)), address(), vec![Some(vm("running", &s.tags))]);
    let running = client.start_worker(&worker).expect("running");
    assert!(
        matches!(running, InteractiveWorkerStart::AlreadyRunning(_)),
        "{running:?}"
    );
    assert_eq!(running.status().map(|status| status.lifecycle), Some(Lifecycle::Ready));
    assert!(s.plane.mutations().is_empty(), "no start against a running worker");
    // Without an attested key the start is real but the worker is still provisioning.
    let pending = s.client(false, None);
    s.plane.script(
        Some(owned(&s)),
        address(),
        vec![Some(vm("deallocated", &s.tags)), Some(vm("running", &s.tags))],
    );
    let started = pending.start_worker(&worker).expect("start");
    assert_eq!(
        started.status().map(|status| (status.lifecycle, status.ssh.is_some())),
        Some((Lifecycle::Provisioning, false))
    );
    // A guest-halted (billed) VM, a synchronous completion and an in-flight start.
    s.plane.script(
        Some(owned(&s)),
        address(),
        vec![Some(vm("stopped", &s.tags)), Some(vm("running", &s.tags))],
    );
    s.plane.lock().start = Some(Ok(Some(AzureLongRunningState::Completed)));
    assert!(matches!(
        client.start_worker(&worker),
        Ok(InteractiveWorkerStart::Started(_))
    ));
    assert_eq!(s.plane.mutations(), [Call::Start(s.group.clone())]);
    s.plane.lock().start = None;
    s.plane.script(
        Some(owned(&s)),
        address(),
        vec![Some(vm("starting", &s.tags)), Some(vm("running", &s.tags))],
    );
    assert!(matches!(
        client.start_worker(&worker),
        Ok(InteractiveWorkerStart::Started(_))
    ));
    assert!(
        s.plane.mutations().is_empty(),
        "a start already in flight is awaited, not re-posted"
    );
}

#[test]
fn start_never_allocates_and_reports_unverified_states_honestly() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false, Some(host_key()));
    let conflict = |status| {
        Some(Err(AzureError::UnexpectedStatus {
            operation: "virtual machine start",
            status,
        }))
    };
    let address = || Some(deployment("Succeeded", "203.0.113.9"));
    s.plane.script(
        Some(owned(&s)),
        address(),
        vec![Some(vm("deallocated", &s.tags)), Some(vm("running", &s.tags))],
    );
    s.plane.lock().start = conflict(409);
    assert!(
        matches!(client.start_worker(&worker), Ok(InteractiveWorkerStart::Started(_))),
        "409 means a start is already in flight; it is awaited"
    );
    s.plane.lock().start = conflict(403);
    s.plane
        .script(Some(owned(&s)), address(), vec![Some(vm("deallocated", &s.tags))]);
    assert!(matches!(
        client.start_worker(&worker),
        Err(AzureError::UnexpectedStatus { status: 403, .. })
    ));
    s.plane.lock().start = Some(Ok(None));
    assert_eq!(
        client.start_worker(&worker),
        Err(AzureError::StartUnverified),
        "a VM that vanished before the POST is unverified, never re-created"
    );
    s.plane.lock().start = None;
    for (states, label) in [
        (vec![Some(vm("deallocated", &s.tags))], "never running within the bound"),
        (vec![None], "no VM to start"),
        (
            vec![Some(vm("deallocated", &s.tags)), None],
            "VM vanished while starting",
        ),
        (vec![Some(vm("deallocating", &s.tags))], "still deallocating"),
    ] {
        s.plane.script(Some(owned(&s)), address(), states);
        assert_eq!(
            client.start_worker(&worker),
            Err(AzureError::StartUnverified),
            "{label}"
        );
        assert!(
            !s.plane.calls().iter().any(|call| matches!(
                call,
                Call::DeleteGroup(_) | Call::CreateGroup(..) | Call::PutDeployment(..)
            )),
            "{label}"
        );
    }
}

#[test]
fn start_never_starts_a_failed_worker_and_rejects_foreign_handles_before_any_mutation() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false, Some(host_key()));
    let address = || Some(deployment("Succeeded", "203.0.113.9"));
    let mut failed = vm("deallocated", &s.tags);
    failed.provisioning_state = "Failed".into();
    s.plane.script(Some(owned(&s)), address(), vec![Some(failed)]);
    assert_eq!(client.start_worker(&worker), Err(AzureError::StartUnverified));
    assert!(s.plane.mutations().is_empty(), "no start on a failed VM");
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Failed", "")),
        vec![Some(vm("deallocated", &s.tags))],
    );
    assert_eq!(
        client.start_worker(&worker),
        Err(AzureError::StartUnverified),
        "a failed deployment's leftover VM is not a stopped worker"
    );
    assert!(s.plane.mutations().is_empty(), "no start behind a failed deployment");
    s.plane.script(
        Some(owned(&s)),
        address(),
        vec![Some(vm("deallocated", &s.tags)), Some(vm("running", &s.tags))],
    );
    s.plane.lock().deployment_on_second_read = Some(deployment("Failed", ""));
    assert_eq!(
        client.start_worker(&worker),
        Err(AzureError::StartUnverified),
        "a deployment that failed during the wait is never reported as started"
    );
    s.plane.script(
        Some(group_info(&s.group, s.tags.clone(), "Deleting")),
        address(),
        vec![Some(vm("deallocated", &s.tags))],
    );
    assert_eq!(
        client.start_worker(&worker),
        Err(AzureError::StartUnverified),
        "group deleting"
    );
    assert!(s.plane.mutations().is_empty());
    s.plane
        .script(Some(owned(&s)), address(), vec![Some(vm("deallocated", &s.tags))]);
    s.plane.lock().vanish_after_first_lookup = true;
    assert_eq!(
        client.start_worker(&worker),
        Err(AzureError::StartUnverified),
        "a group that vanishes right after its proof is a race, not an absence"
    );
    assert!(s.plane.mutations().is_empty());
    let mut foreign = worker.clone();
    foreign.identity.resource_id = format!("{}-other", worker.identity.resource_id);
    assert_eq!(client.start_worker(&foreign), Err(AzureError::InvalidPersistedWorker));
    let mut other_tags = s.tags.clone();
    other_tags.insert("horizon-job-id".into(), "other".into());
    s.plane.script(
        Some(group_info(&s.group, other_tags, "Succeeded")),
        address(),
        vec![Some(vm("deallocated", &s.tags))],
    );
    assert_eq!(client.start_worker(&worker), Err(MISMATCH));
    assert!(s.plane.mutations().is_empty());
}

#[test]
fn start_reproves_ownership_on_every_poll_and_before_answering() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false, Some(host_key()));
    let address = || Some(deployment("Succeeded", "203.0.113.9"));
    let mut retagged = s.tags.clone();
    retagged.insert("horizon-job-id".into(), "other".into());
    let mut foreign_vm = vm("running", &s.tags);
    foreign_vm.tags = retagged.clone();
    s.plane.script(
        Some(owned(&s)),
        address(),
        vec![Some(vm("deallocated", &s.tags)), Some(foreign_vm.clone())],
    );
    assert_eq!(
        client.start_worker(&worker),
        Err(MISMATCH),
        "a retagged VM at the same path is not this worker starting"
    );
    let two_step = || vec![Some(vm("deallocated", &s.tags)), Some(vm("running", &s.tags))];
    s.plane.script(Some(owned(&s)), address(), two_step());
    s.plane.lock().group_after_start = Some(GroupChange::Replaced(group_info(&s.group, retagged, "Succeeded")));
    assert_eq!(
        client.start_worker(&worker),
        Err(MISMATCH),
        "group retagged during the wait"
    );
    s.plane.script(Some(owned(&s)), address(), two_step());
    s.plane.lock().group_after_start = Some(GroupChange::Deleted);
    assert_eq!(
        client.start_worker(&worker),
        Err(AzureError::StartUnverified),
        "group deleted during the wait"
    );
    s.plane.script(Some(owned(&s)), address(), two_step());
    s.plane.lock().group_after_start = Some(GroupChange::Replaced(group_info(&s.group, s.tags.clone(), "Deleting")));
    assert_eq!(
        client.start_worker(&worker),
        Err(AzureError::StartUnverified),
        "a running VM inside a group being deleted is not a started worker"
    );
    // Running is reached, but the re-observation finds a replaced VM: nothing is claimed.
    s.plane.script(
        Some(owned(&s)),
        address(),
        vec![
            Some(vm("deallocated", &s.tags)),
            Some(vm("running", &s.tags)),
            Some(foreign_vm),
        ],
    );
    assert_eq!(
        client.start_worker(&worker),
        Err(MISMATCH),
        "VM replaced before the answer"
    );
    assert!(
        !s.plane.calls().iter().any(|call| matches!(
            call,
            Call::DeleteGroup(_) | Call::CreateGroup(..) | Call::PutDeployment(..)
        )),
        "start never deletes or creates"
    );
}

#[test]
fn start_answers_only_for_a_vm_observed_physically_running_after_the_wait() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false, Some(host_key()));
    // Running but behind an unusable address: started, not reachable, so Provisioning.
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Succeeded", "10.0.0.5")),
        vec![Some(vm("deallocated", &s.tags)), Some(vm("running", &s.tags))],
    );
    let started = client.start_worker(&worker).expect("start");
    assert_eq!(
        started.status().map(|status| (status.lifecycle, status.ssh.is_some())),
        Some((Lifecycle::Provisioning, false)),
        "{started:?}"
    );
    // The wait saw running, but the answer-time observation cannot read a power state.
    let mut unreadable = vm("running", &s.tags);
    unreadable.power_state = None;
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Succeeded", "203.0.113.9")),
        vec![
            Some(vm("deallocated", &s.tags)),
            Some(vm("running", &s.tags)),
            Some(unreadable),
        ],
    );
    assert_eq!(
        client.start_worker(&worker),
        Err(AzureError::StartUnverified),
        "an unreadable VM is never reported as started"
    );
}
