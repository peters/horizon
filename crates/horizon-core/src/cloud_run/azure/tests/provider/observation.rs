use super::*;

#[test]
fn recovery_maps_every_observed_state_without_creating() {
    let s = Scenario::new();
    let up = || Some(deployment("Succeeded", "203.0.113.9"));
    let dep = |state: &str| Some(deployment(state, ""));
    let power = |state: &str| Some(vm(state, &s.tags));
    let cases = [
        (up(), power("deallocated"), Lifecycle::Stopped, "deallocated"),
        (
            up(),
            power("stopped"),
            Lifecycle::Unknown,
            "billed halt is never a retained stop",
        ),
        (
            dep("Failed"),
            power("running"),
            Lifecycle::Failed,
            "a VM left behind a failed deployment",
        ),
        (
            dep("Canceled"),
            power("deallocated"),
            Lifecycle::Failed,
            "a VM left behind a canceled deployment",
        ),
        (
            dep("Running"),
            power("running"),
            Lifecycle::Provisioning,
            "deployment still running",
        ),
        (dep("Failed"), None, Lifecycle::Failed, "failed deployment"),
        (dep("Canceled"), None, Lifecycle::Failed, "canceled deployment"),
        (up(), None, Lifecycle::Unknown, "finished deployment without its VM"),
        (
            None,
            None,
            Lifecycle::Unknown,
            "owned group with nothing in it: recovery never submits",
        ),
    ];
    for (deployment, vm_state, lifecycle, label) in cases {
        s.plane.script(Some(owned(&s)), deployment, vec![vm_state]);
        assert_eq!(s.reconcile().lifecycle, lifecycle, "{label}");
        assert!(s.plane.mutations().is_empty(), "{label}");
    }
    s.plane.script(None, None, vec![None]);
    assert_eq!(s.client(false).reconcile_worker(&s.request), Ok(None));
}
#[test]
fn recovery_treats_unusable_addresses_as_unknown() {
    let s = Scenario::new();
    for address in [
        "not-an-ip",
        "0.0.0.0",
        "127.0.0.1",
        "::",
        "fe80::1",
        "169.254.1.1",
        "10.0.0.1",
        "fd00::1",
        "::ffff:10.0.0.1",
    ] {
        s.plane.script(
            Some(owned(&s)),
            Some(deployment("Succeeded", address)),
            vec![Some(vm("running", &s.tags))],
        );
        assert_eq!(s.reconcile().lifecycle, Lifecycle::Unknown, "{address}");
        assert!(s.plane.mutations().is_empty(), "{address}");
    }
}

#[test]
fn foreign_or_tampered_resources_are_rejected_before_any_mutation() {
    let s = Scenario::new();
    let mut other_client = s.tags.clone();
    other_client.insert(TAG_CLIENT_KEY_DIGEST.into(), "0".repeat(64));
    s.plane
        .script(Some(group_info(&s.group, other_client, "Succeeded")), None, vec![None]);
    let client = s.client(true);
    let worker = s.persisted();
    assert_eq!(client.ensure_worker(&s.request), Err(MISMATCH));
    assert_eq!(client.reconcile_worker(&s.request), Err(MISMATCH));
    assert_eq!(client.inspect_worker(&worker), Err(MISMATCH));
    assert_eq!(client.delete_worker(&worker), Err(MISMATCH));
    assert!(
        s.plane.calls().iter().all(|call| matches!(call, Call::GetGroup(_))),
        "{:?}",
        s.plane.calls()
    );
    let mut other_job = s.tags.clone();
    other_job.insert(TAG_JOB.into(), CloudJobId::new().to_string());
    s.plane
        .script(Some(owned(&s)), None, vec![Some(vm("running", &other_job))]);
    assert_eq!(client.reconcile_worker(&s.request), Err(MISMATCH), "VM tags disagree");
    let mut recased = worker.clone();
    recased.identity.resource_id = format!("/subscriptions/{SUB}/resourcegroups/{}", s.group.to_ascii_uppercase());
    s.plane
        .script(Some(owned(&s)), None, vec![Some(vm("running", &s.tags))]);
    assert!(
        client.inspect_worker(&recased).expect("inspect").is_some(),
        "ARM ids compare case-insensitively"
    );
    let mut limited = s.request.clone();
    limited.target.lifetime = WorkerLifetime::TimeLimited { seconds: 600 };
    assert_eq!(client.ensure_worker(&limited), Err(AzureError::UnsupportedLifetime));
    let mut bad_key = s.request.clone();
    bad_key.ssh_public_key = "ssh-rsa AAAA".into();
    assert_eq!(client.ensure_worker(&bad_key), Err(AzureError::InvalidTarget));
    let mut moved = owned(&s);
    moved.location = "westeurope".into();
    s.plane.script(Some(moved), None, vec![None]);
    assert_eq!(
        client.ensure_worker(&s.request),
        Err(AzureError::PlacementMismatch),
        "group in another region is never repaired"
    );
    assert_eq!(client.reconcile_worker(&s.request), Err(AzureError::PlacementMismatch));
    assert!(s.plane.mutations().is_empty());
}

#[test]
fn placement_and_subscription_policy_apply_to_request_paths_only() {
    let s = Scenario::new();
    let client = s.client(false);
    let worker = s.persisted();
    let mut other_subscription = profile();
    other_subscription.subscription_id = OTHER_SUB.into();
    other_subscription.image_pull_identity_id = profile().image_pull_identity_id.replace(SUB, OTHER_SUB);
    let never = |_: CloudWorkflowId, _: CloudJobId, _: &WorkerTarget, _: &str| Ok(false);
    assert!(
        AzureClient::with_transport(other_subscription, s.plane.clone(), never).is_err(),
        "transport bound to the profile's subscription"
    );
    let mut relocated = owned(&s);
    relocated.location = "westeurope".into();
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Succeeded", "203.0.113.9")),
        vec![Some(vm("running", &s.tags))],
    );
    s.plane.lock().group_after_first_lookup = Some(relocated);
    assert_eq!(
        client.reconcile_worker(&s.request),
        Err(AzureError::PlacementMismatch),
        "recreated elsewhere mid-read"
    );
    let mut resized = vm("running", &s.tags);
    resized.vm_size = "Standard_D8s_v3".into();
    s.plane.script(Some(owned(&s)), None, vec![Some(resized)]);
    assert_eq!(
        client.reconcile_worker(&s.request),
        Err(AzureError::PlacementMismatch),
        "resized out of band"
    );
    assert_eq!(client.ensure_worker(&s.request), Err(AzureError::PlacementMismatch));
    assert!(
        client.inspect_worker(&worker).expect("inspect").is_some(),
        "the persisted handle stays inspectable"
    );
    let mut resized = vm("running", &s.tags);
    resized.vm_size = "Standard_D8s_v3".into();
    s.plane
        .script(Some(owned(&s)), Some(deployment("Failed", "")), vec![Some(resized)]);
    assert_eq!(
        client
            .reconcile_worker(&s.request)
            .map(|status| status.map(|status| status.lifecycle)),
        Ok(Some(Lifecycle::Failed)),
        "a failed deployment is reported before any placement verdict on its leftovers"
    );
    assert!(s.plane.mutations().is_empty());
    let mut bigger = s.request.clone();
    bigger.target.disk_gib = 64;
    s.plane
        .script(Some(owned(&s)), None, vec![Some(vm("running", &s.tags))]);
    assert_eq!(
        client.ensure_worker(&bigger),
        Err(MISMATCH),
        "a different disk size is another worker, never adopted"
    );
    assert_eq!(client.reconcile_worker(&bigger), Err(MISMATCH));
    let mut renamed = worker.clone();
    renamed.target.profile = "cpu-north-b".into();
    let mut other_profile = profile();
    other_profile.name = "cpu-north-b".into();
    let never = |_: CloudWorkflowId, _: CloudJobId, _: &WorkerTarget, _: &str| Ok(false);
    let other = AzureClient::with_transport(other_profile, s.plane.clone(), never).expect("client");
    assert_eq!(
        other.inspect_worker(&renamed),
        Err(MISMATCH),
        "a handle under another profile name is not this worker"
    );
    assert!(s.plane.mutations().is_empty());
}

#[test]
fn inspect_and_delete_act_only_on_the_exact_owned_group() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false);
    assert_eq!(client.inspect_worker(&worker), Ok(None));
    assert_eq!(
        client.delete_worker(&worker),
        Ok(InteractiveWorkerCleanup::AlreadyAbsent)
    );
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Succeeded", "203.0.113.9")),
        vec![Some(vm("running", &s.tags))],
    );
    let status = client.inspect_worker(&worker).expect("inspect").expect("present");
    assert_eq!(
        (status.lifecycle, status.worker),
        (Lifecycle::Provisioning, worker.clone())
    );
    assert!(s.plane.mutations().is_empty());
    s.plane.script(Some(owned(&s)), None, vec![None]);
    assert_eq!(client.delete_worker(&worker), Ok(InteractiveWorkerCleanup::Deleted));
    let lookup = Call::GetGroup(s.group.clone());
    assert_eq!(
        s.plane.calls(),
        [lookup.clone(), lookup, Call::DeleteGroup(s.group.clone())],
        "proof, re-proof, delete"
    );
    assert_eq!(
        client.delete_worker(&worker),
        Ok(InteractiveWorkerCleanup::AlreadyAbsent)
    );
    assert_eq!(client.inspect_worker(&worker), Ok(None));
    s.plane.script(Some(owned(&s)), None, vec![None]);
    s.plane.lock().delete_status = Some(404);
    assert_eq!(
        client.delete_worker(&worker),
        Ok(InteractiveWorkerCleanup::AlreadyAbsent),
        "removed concurrently"
    );
    s.plane.lock().delete_status = Some(409);
    assert!(matches!(
        client.delete_worker(&worker),
        Err(AzureError::UnexpectedStatus { status: 409, .. })
    ));
    s.plane.lock().delete_status = None;
    let mut other_tags = s.tags.clone();
    other_tags.insert(TAG_JOB.into(), CloudJobId::new().to_string());
    s.plane.script(Some(owned(&s)), None, vec![None]);
    s.plane.lock().group_after_first_lookup = Some(AzureGroupInfo {
        tags: other_tags,
        ..owned(&s)
    });
    assert_eq!(
        client.delete_worker(&worker),
        Err(MISMATCH),
        "replaced between proof and delete"
    );
    assert!(!s.plane.calls().iter().any(|call| matches!(call, Call::DeleteGroup(_))));
    s.plane
        .script(Some(group_info(&s.group, s.tags.clone(), "Deleting")), None, vec![None]);
    assert_eq!(
        client.delete_worker(&worker),
        Ok(InteractiveWorkerCleanup::Deleted),
        "an accepted deletion in flight"
    );
    assert!(
        s.plane.mutations().is_empty(),
        "no second DELETE while ARM is still deleting"
    );
}
