use super::*;
use crate::cloud_run::interactive_worker_stop::{InteractiveWorkerStop, InteractiveWorkerStopProvider};

#[test]
fn ready_requires_a_running_vm_a_usable_address_and_a_pinned_host_key() {
    let s = Scenario::new();
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Succeeded", "203.0.113.9")),
        vec![Some(vm("running", &s.tags))],
    );
    let pending = s.reconcile(None);
    assert_eq!(
        (pending.lifecycle, pending.ssh.is_some()),
        (Lifecycle::Provisioning, false)
    );
    let ready = s.reconcile(Some(host_key()));
    assert_eq!(ready.lifecycle, Lifecycle::Ready);
    let ssh = ready.ssh.as_ref().expect("endpoint");
    assert_eq!(
        (ssh.host.as_str(), ssh.port, ssh.username.as_str()),
        ("203.0.113.9", SSH_PORT, SSH_USERNAME)
    );
    assert_eq!(ssh.host_key, host_key());
    let now = time::OffsetDateTime::now_utc();
    assert!(ready.is_ready_for(&s.request, now));
    let mut other = s.request.clone();
    other.ssh_public_key = ed25519_key(8, "");
    assert!(
        !ready.is_ready_for(&other, now),
        "another client key is never attachable"
    );
    assert_eq!(
        s.reconcile(Some("ssh-rsa AAAA".into())).lifecycle,
        Lifecycle::Provisioning,
        "non-Ed25519 host key"
    );
    let ensured = s
        .client(false, Some(host_key()))
        .ensure_worker(&s.request)
        .expect("ensure");
    assert!(matches!(ensured, InteractiveWorkerEnsure::Reused(_)));
    assert_eq!(ensured.status().lifecycle, Lifecycle::Ready);
    assert!(
        s.plane.mutations().is_empty(),
        "recovery and reuse never create: {:?}",
        s.plane.calls()
    );
    // The key is paired with the address only if the same worker is still there once
    // the (slow) key read returns.
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Succeeded", "203.0.113.9")),
        vec![Some(vm("running", &s.tags))],
    );
    s.plane.lock().deployment_on_second_read = Some(deployment("Succeeded", "203.0.113.10"));
    let moved = s.reconcile(Some(host_key()));
    assert_eq!(
        (moved.lifecycle, moved.ssh.is_some()),
        (Lifecycle::Provisioning, false),
        "address changed while the key was read"
    );
    let mut foreign = vm("running", &s.tags);
    foreign.tags.insert(TAG_JOB.into(), CloudJobId::new().to_string());
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Succeeded", "203.0.113.9")),
        vec![Some(vm("running", &s.tags)), Some(foreign)],
    );
    assert_eq!(
        s.client(false, Some(host_key())).reconcile_worker(&s.request),
        Err(MISMATCH),
        "VM replaced while the key was read"
    );
}

#[test]
fn stop_deallocates_once_and_verifies_the_retained_inactive_state() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false, None);
    assert_eq!(client.stop_worker(&worker), Ok(InteractiveWorkerStop::AlreadyAbsent));
    let running = || Some(vm("running", &s.tags));
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Succeeded", "203.0.113.9")),
        vec![
            running(),
            Some(vm("deallocating", &s.tags)),
            Some(vm("deallocated", &s.tags)),
        ],
    );
    assert_eq!(client.stop_worker(&worker), Ok(InteractiveWorkerStop::Stopped));
    assert_eq!(s.plane.mutations(), [Call::Deallocate(s.group.clone())]);
    assert_eq!(
        s.plane
            .calls()
            .iter()
            .filter(|call| matches!(call, Call::GetVm(_)))
            .count(),
        3
    );
    s.plane
        .script(Some(owned(&s)), None, vec![Some(vm("deallocated", &s.tags))]);
    assert_eq!(
        client.stop_worker(&worker),
        Ok(InteractiveWorkerStop::Stopped),
        "idempotent on a deallocated VM"
    );
    assert!(s.plane.mutations().is_empty());
    s.plane.script(
        Some(owned(&s)),
        None,
        vec![Some(vm("deallocating", &s.tags)), Some(vm("deallocated", &s.tags))],
    );
    assert_eq!(
        client.stop_worker(&worker),
        Ok(InteractiveWorkerStop::Stopped),
        "a retry only awaits an in-flight deallocation"
    );
    assert!(s.plane.mutations().is_empty(), "no second deallocate");
    s.plane.script(
        Some(owned(&s)),
        None,
        vec![Some(vm("stopped", &s.tags)), Some(vm("deallocated", &s.tags))],
    );
    assert_eq!(
        client.stop_worker(&worker),
        Ok(InteractiveWorkerStop::Stopped),
        "a billed guest halt is deallocated"
    );
    assert_eq!(s.plane.mutations(), [Call::Deallocate(s.group.clone())]);
    s.plane
        .script(Some(owned(&s)), None, vec![running(), Some(vm("deallocated", &s.tags))]);
    s.plane.lock().deallocate = Some(Ok(Some(AzureLongRunningState::Completed)));
    assert_eq!(
        client.stop_worker(&worker),
        Ok(InteractiveWorkerStop::Stopped),
        "synchronous completion"
    );
    // Stoppability follows the VM, not the endpoint: a running VM behind an unusable
    // address or a failed deployment is still billed.
    for (deployment, label) in [
        (deployment("Succeeded", "10.0.0.5"), "unusable address"),
        (deployment("Failed", ""), "failed deployment"),
    ] {
        s.plane.script(
            Some(owned(&s)),
            Some(deployment),
            vec![running(), Some(vm("deallocated", &s.tags))],
        );
        s.plane.lock().deallocate = None;
        assert_eq!(
            client.stop_worker(&worker),
            Ok(InteractiveWorkerStop::Stopped),
            "{label}"
        );
        assert_eq!(s.plane.mutations(), [Call::Deallocate(s.group.clone())], "{label}");
    }
}

#[test]
fn stop_never_deletes_and_reports_unverified_states_honestly() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false, None);
    let conflict = |status| {
        Some(Err(AzureError::UnexpectedStatus {
            operation: "virtual machine deallocation",
            status,
        }))
    };
    s.plane
        .script(Some(owned(&s)), None, vec![Some(vm("running", &s.tags))]);
    s.plane.lock().deallocate = conflict(409);
    assert_eq!(
        client.stop_worker(&worker),
        Err(AzureError::StopUnverified),
        "409 is awaited, then unverified within the bound"
    );
    s.plane.lock().deallocate = conflict(403);
    assert!(matches!(
        client.stop_worker(&worker),
        Err(AzureError::UnexpectedStatus { status: 403, .. })
    ));
    s.plane
        .script(Some(owned(&s)), None, vec![Some(vm("running", &s.tags))]);
    s.plane.lock().deallocate = Some(Ok(None));
    assert_eq!(
        client.stop_worker(&worker),
        Err(AzureError::StopUnverified),
        "a VM that vanished before the POST was never verified"
    );
    s.plane.lock().deallocate = None;
    for (states, label) in [
        (vec![Some(vm("running", &s.tags))], "never deallocated within the bound"),
        (vec![None], "no VM to stop"),
        (vec![Some(vm("running", &s.tags)), None], "VM vanished while stopping"),
        (vec![Some(vm("starting", &s.tags))], "starting is not stoppable"),
    ] {
        s.plane.script(Some(owned(&s)), None, states);
        assert_eq!(client.stop_worker(&worker), Err(AzureError::StopUnverified), "{label}");
        assert!(
            !s.plane.calls().iter().any(|call| matches!(call, Call::DeleteGroup(_))),
            "{label}"
        );
    }
    s.plane
        .script(Some(owned(&s)), None, vec![Some(vm("running", &s.tags))]);
    s.plane.lock().vanish_after_first_lookup = true;
    assert_eq!(
        client.stop_worker(&worker),
        Err(AzureError::StopUnverified),
        "a group that vanishes right after its proof is a race, not an absence"
    );
    assert!(s.plane.mutations().is_empty());
    let mut failed = vm("running", &s.tags);
    failed.provisioning_state = "Failed".into();
    s.plane.script(Some(owned(&s)), None, vec![Some(failed)]);
    assert_eq!(
        client.stop_worker(&worker),
        Err(AzureError::StopUnverified),
        "failed VM"
    );
    assert!(s.plane.mutations().is_empty(), "no deallocate on a failed VM");
    let mut foreign = worker.clone();
    foreign.identity.resource_id = format!("{}-other", worker.identity.resource_id);
    assert_eq!(client.stop_worker(&foreign), Err(AzureError::InvalidPersistedWorker));
    let mut other_tags = s.tags.clone();
    other_tags.insert("horizon-job-id".into(), "other".into());
    s.plane
        .script(Some(group_info(&s.group, other_tags, "Succeeded")), None, vec![None]);
    assert_eq!(client.stop_worker(&worker), Err(MISMATCH));
    assert!(s.plane.mutations().is_empty());
}

#[test]
fn stop_reproves_ownership_on_every_poll_of_the_wait() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false, None);
    let mut retagged = s.tags.clone();
    retagged.insert("horizon-job-id".into(), "other".into());
    let mut foreign_vm = vm("deallocated", &s.tags);
    foreign_vm.tags = retagged.clone();
    s.plane.script(
        Some(owned(&s)),
        None,
        vec![Some(vm("running", &s.tags)), Some(foreign_vm)],
    );
    assert_eq!(
        client.stop_worker(&worker),
        Err(MISMATCH),
        "a retagged VM at the same path is not this worker stopping"
    );
    s.plane.script(
        Some(owned(&s)),
        None,
        vec![Some(vm("running", &s.tags)), Some(vm("deallocated", &s.tags))],
    );
    s.plane.lock().group_after_deallocate = Some(GroupChange::Replaced(group_info(&s.group, retagged, "Succeeded")));
    assert_eq!(
        client.stop_worker(&worker),
        Err(MISMATCH),
        "group retagged during the wait"
    );
    s.plane.script(
        Some(owned(&s)),
        None,
        vec![Some(vm("running", &s.tags)), Some(vm("deallocated", &s.tags))],
    );
    s.plane.lock().group_after_deallocate = Some(GroupChange::Deleted);
    assert_eq!(
        client.stop_worker(&worker),
        Err(AzureError::StopUnverified),
        "group deleted during the wait"
    );
    s.plane.script(
        Some(owned(&s)),
        None,
        vec![Some(vm("running", &s.tags)), Some(vm("deallocated", &s.tags))],
    );
    s.plane.lock().group_after_deallocate =
        Some(GroupChange::Replaced(group_info(&s.group, s.tags.clone(), "Deleting")));
    assert_eq!(
        client.stop_worker(&worker),
        Err(AzureError::StopUnverified),
        "a deallocated VM inside a group being deleted is not retained"
    );
}

#[test]
fn run_command_host_keys_accept_only_one_ed25519_key_read_from_the_exact_vm() {
    use crate::cloud_run::azure::tests::IMAGE;
    use crate::cloud_run::azure::{AzureHostKeySource, AzureRunCommandHostKeys};
    let s = Scenario::new();
    let source = AzureRunCommandHostKeys::new(std::sync::Arc::new(s.plane.clone()));
    let worker = AzureWorker {
        workflow_id: CloudWorkflowId::new(),
        job_id: CloudJobId::new(),
        subscription_id: SUB.into(),
        resource_group: s.group.clone(),
        group_id: s.group_id.clone(),
        image: IMAGE.into(),
        lifetime: InteractiveWorkerLifetime::Persistent,
    };
    let key = ed25519_key(3, "");
    let client_key = s.request.ssh_public_key.clone();
    let read = |output: Option<&str>| {
        s.plane.lock().run_output = output.map(str::to_string);
        s.plane.lock().calls.clear();
        source.host_key(&worker, "203.0.113.9", &client_key)
    };
    assert_eq!(read(Some(&key)), Some(key.clone()));
    assert_eq!(
        read(Some(&format!("{key} root@worker\n"))),
        Some(key.clone()),
        "the comment is dropped"
    );
    assert_eq!(
        s.plane.calls(),
        [Call::Run(s.group.clone(), AzureRunCommand::HostKey)],
        "a fixed script against the exact group"
    );
    assert_eq!(read(None), None, "absent VM");
    assert_eq!(read(Some("")), None, "no key yet");
    assert_eq!(
        read(Some(&format!("{key}\n{}", ed25519_key(4, "")))),
        None,
        "two keys are ambiguous"
    );
    assert_eq!(read(Some("ssh-rsa AAAAB3 host")), None, "wrong algorithm");
    assert_eq!(read(Some("cat: no such file")), None, "an error message is not a key");
    assert_eq!(read(Some("ssh-ed25519 not-base64")), None);
    let elsewhere = AzureWorker {
        subscription_id: OTHER_SUB.into(),
        ..worker.clone()
    };
    s.plane.lock().run_output = Some(key.clone());
    s.plane.lock().calls.clear();
    assert_eq!(
        source.host_key(&elsewhere, "203.0.113.9", &client_key),
        None,
        "never run in another subscription"
    );
    assert!(s.plane.calls().is_empty());
}
