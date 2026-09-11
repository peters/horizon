use super::*;

#[test]
fn ensure_creates_once_behind_the_fence_and_then_reuses() {
    let s = Scenario::new();
    let created = s.client(true, None).ensure_worker(&s.request).expect("created");
    let InteractiveWorkerEnsure::Created(status) = &created else {
        panic!("first ensure creates: {created:?}");
    };
    assert_eq!(status.lifecycle, Lifecycle::Provisioning);
    assert_eq!(status.worker.identity.resource_id, s.group_id);
    assert_eq!(
        (&status.worker.target, &status.worker.lifetime),
        (&s.request.target, &InteractiveWorkerLifetime::Persistent)
    );
    let calls = s.plane.calls();
    let lookup = Call::GetGroup(s.group.clone());
    assert_eq!(
        &calls[..3],
        [
            lookup.clone(),
            lookup.clone(),
            Call::CreateGroup(s.group.clone(), s.tags.clone())
        ]
    );
    let put = calls
        .iter()
        .position(|call| matches!(call, Call::PutDeployment(..)))
        .expect("submission");
    let Call::PutDeployment(group, name, parameters) = &calls[put] else {
        unreachable!()
    };
    assert_eq!((group.as_str(), name.as_str()), (s.group.as_str(), DEPLOYMENT_NAME));
    assert_eq!(parameters["adminPublicKey"]["value"], s.request.ssh_public_key);
    let fresh = [
        Call::GetDeployment(s.group.clone()),
        Call::GetVm(s.group.clone()),
        lookup.clone(),
    ];
    assert!(
        calls[3..put].ends_with(&fresh),
        "a complete observation immediately precedes the submission: {calls:?}"
    );
    assert_eq!(&calls[put + 1..], [lookup], "a final re-read follows the submission");
    assert_eq!(s.plane.mutations().len(), 2, "one creation, one submission");
    s.plane.lock().calls.clear();
    let reused = s.client(false, None).ensure_worker(&s.request).expect("reused");
    assert!(matches!(reused, InteractiveWorkerEnsure::Reused(_)));
    assert_eq!(
        reused.status().lifecycle,
        Lifecycle::Provisioning,
        "deployment accepted, VM pending"
    );
    assert!(s.plane.mutations().is_empty(), "{:?}", s.plane.calls());
}

#[test]
fn ensure_repairs_only_a_group_it_owns_and_never_one_being_deleted() {
    // A claim lost to a crash before the group appeared: a second look at ARM, then unresolved.
    let s = Scenario::new();
    assert_eq!(
        s.client(false, None).ensure_worker(&s.request),
        Err(AzureError::CreationUnresolved)
    );
    assert_eq!(
        s.plane.calls(),
        [Call::GetGroup(s.group.clone()), Call::GetGroup(s.group.clone())]
    );
    // A claim lost after the group appeared is adopted through the same tag proof.
    s.plane.script(Some(owned(&s)), None, vec![None]);
    let adopted = s.client(true, None).ensure_worker(&s.request).expect("adopted");
    assert!(matches!(adopted, InteractiveWorkerEnsure::Reused(_)));
    assert!(
        matches!(s.plane.mutations().as_slice(), [Call::PutDeployment(..)]),
        "only the missing deployment"
    );
    // A group that appears between the first lookup and a denied claim is observed, not re-created.
    let raced = Scenario::new();
    raced.plane.lock().deployment = Some(deployment("Succeeded", "203.0.113.9"));
    raced.plane.lock().vm_states = vec![Some(vm("running", &raced.tags))];
    raced.plane.lock().appear_on_second_lookup = Some(owned(&raced));
    let observed = raced
        .client(false, None)
        .ensure_worker(&raced.request)
        .expect("observed");
    assert!(matches!(observed, InteractiveWorkerEnsure::Reused(_)), "{observed:?}");
    assert_eq!(observed.status().lifecycle, Lifecycle::Provisioning);
    assert!(
        raced.plane.mutations().is_empty(),
        "nothing is resubmitted for a finished worker"
    );
    // The same race with a granted claim: the group is adopted, never overwritten by the upsert.
    let granted = Scenario::new();
    granted.plane.lock().deployment = Some(deployment("Succeeded", "203.0.113.9"));
    granted.plane.lock().vm_states = vec![Some(vm("running", &granted.tags))];
    granted.plane.lock().appear_on_second_lookup = Some(owned(&granted));
    let adopted = granted
        .client(true, None)
        .ensure_worker(&granted.request)
        .expect("adopted");
    assert!(matches!(adopted, InteractiveWorkerEnsure::Reused(_)), "{adopted:?}");
    assert!(
        granted.plane.mutations().is_empty(),
        "no PUT over a group that appeared: {:?}",
        granted.plane.calls()
    );
    // A foreign group that appears in the window is refused, never overwritten.
    let foreign = Scenario::new();
    let mut other_tags = foreign.tags.clone();
    other_tags.insert(TAG_JOB.into(), CloudJobId::new().to_string());
    foreign.plane.lock().appear_on_second_lookup = Some(AzureGroupInfo {
        tags: other_tags,
        ..owned(&foreign)
    });
    assert_eq!(
        foreign.client(true, None).ensure_worker(&foreign.request),
        Err(MISMATCH)
    );
    assert!(foreign.plane.mutations().is_empty());
    // A throttled submission keeps the owned group; the next ensure repairs it.
    let t = Scenario::new();
    t.plane.lock().fail_deployment = true;
    let error = t.client(true, None).ensure_worker(&t.request).expect_err("submission");
    assert!(
        matches!(&error, AzureError::CreationIncomplete { cause } if matches!(**cause, AzureError::UnexpectedStatus { status: 429, .. }))
    );
    assert!(
        std::error::Error::source(&error).is_some(),
        "the transport failure stays reachable as the error source"
    );
    t.plane.lock().fail_deployment = false;
    t.plane.lock().calls.clear();
    let repaired = t.client(false, None).ensure_worker(&t.request).expect("repaired");
    assert!(matches!(repaired, InteractiveWorkerEnsure::Reused(_)));
    assert!(matches!(t.plane.mutations().as_slice(), [Call::PutDeployment(..)]));
}

#[test]
fn ensure_never_deploys_into_a_group_that_changed_under_it() {
    // A group that starts deleting, or is retagged, between observation and repair is never deployed into.
    let racing = Scenario::new();
    racing.plane.lock().group = Some(owned(&racing));
    racing.plane.lock().group_after_first_lookup = Some(AzureGroupInfo {
        provisioning_state: "Deleting".into(),
        ..owned(&racing)
    });
    let observed = racing
        .client(false, None)
        .ensure_worker(&racing.request)
        .expect("observed");
    assert_eq!(observed.status().lifecycle, Lifecycle::Deleting);
    assert!(
        racing.plane.mutations().is_empty(),
        "no submission into a deleting group"
    );
    let mut foreign_tags = racing.tags.clone();
    foreign_tags.insert(TAG_JOB.into(), CloudJobId::new().to_string());
    racing.plane.script(Some(owned(&racing)), None, vec![None]);
    racing.plane.lock().group_after_first_lookup = Some(AzureGroupInfo {
        tags: foreign_tags,
        ..owned(&racing)
    });
    assert_eq!(racing.client(false, None).ensure_worker(&racing.request), Err(MISMATCH));
    assert!(
        racing.plane.mutations().is_empty(),
        "no submission into a retagged group"
    );
    // A deployment that appears between the observation reads and the repair is observed, not overwritten.
    let appeared = Scenario::new();
    appeared.plane.lock().group = Some(owned(&appeared));
    appeared.plane.lock().deployment_on_second_read = Some(deployment("Running", ""));
    let observed = appeared
        .client(false, None)
        .ensure_worker(&appeared.request)
        .expect("observed");
    assert_eq!(observed.status().lifecycle, Lifecycle::Provisioning);
    assert!(appeared.plane.mutations().is_empty(), "no second deployment upsert");
    // A group that vanishes during the reads is absent for the non-creating APIs.
    let vanished = Scenario::new();
    vanished.plane.script(
        Some(owned(&vanished)),
        Some(deployment("Succeeded", "203.0.113.9")),
        vec![Some(vm("running", &vanished.tags))],
    );
    vanished.plane.lock().vanish_after_first_lookup = true;
    assert_eq!(
        vanished.client(false, None).reconcile_worker(&vanished.request),
        Ok(None)
    );
    vanished.plane.script(Some(owned(&vanished)), None, vec![None]);
    vanished.plane.lock().vanish_after_first_lookup = true;
    assert_eq!(
        vanished.client(false, None).inspect_worker(&vanished.persisted()),
        Ok(None)
    );
    vanished.plane.script(Some(owned(&vanished)), None, vec![None]);
    vanished.plane.lock().vanish_after_first_lookup = true;
    assert_eq!(
        vanished.client(false, None).ensure_worker(&vanished.request),
        Err(AzureError::CreationUnresolved),
        "a lost worker during ensure is not an absence"
    );
    assert!(vanished.plane.mutations().is_empty());
    // A VM that appears between the observation reads and the repair is observed, never written over.
    let vm_appeared = Scenario::new();
    vm_appeared.plane.lock().group = Some(owned(&vm_appeared));
    let mut foreign_vm = vm("running", &vm_appeared.tags);
    foreign_vm.tags.insert(TAG_JOB.into(), CloudJobId::new().to_string());
    vm_appeared.plane.lock().vm_states = vec![None, Some(foreign_vm)];
    assert_eq!(
        vm_appeared.client(false, None).ensure_worker(&vm_appeared.request),
        Err(MISMATCH)
    );
    assert!(
        vm_appeared.plane.mutations().is_empty(),
        "no submission over a VM with foreign tags"
    );
    // A failed deployment behind a VM does not hide a deletion that began during the reads.
    let failed_then_deleting = Scenario::new();
    failed_then_deleting.plane.script(
        Some(owned(&failed_then_deleting)),
        Some(deployment("Failed", "")),
        vec![Some(vm("running", &failed_then_deleting.tags))],
    );
    failed_then_deleting.plane.lock().group_after_first_lookup = Some(AzureGroupInfo {
        provisioning_state: "Deleting".into(),
        ..owned(&failed_then_deleting)
    });
    assert_eq!(failed_then_deleting.reconcile(None).lifecycle, Lifecycle::Deleting);
}

#[test]
fn ensure_never_deploys_into_a_group_changed_right_after_creation() {
    // A deleting group whose tags change before the re-proof is a mismatch, not Deleting.
    let retagged = Scenario::new();
    let mut deleting = owned(&retagged);
    deleting.provisioning_state = "Deleting".into();
    let mut foreign = deleting.clone();
    foreign.tags.insert(TAG_JOB.into(), CloudJobId::new().to_string());
    retagged.plane.script(Some(deleting), None, vec![None]);
    retagged.plane.lock().group_after_first_lookup = Some(foreign);
    assert_eq!(
        retagged.client(false, None).reconcile_worker(&retagged.request),
        Err(MISMATCH)
    );
    // A group retagged or deleting right after creation is never deployed into either.
    let stolen = Scenario::new();
    let mut other_tags = stolen.tags.clone();
    other_tags.insert(TAG_JOB.into(), CloudJobId::new().to_string());
    stolen.plane.lock().group_after_create = Some(AzureGroupInfo {
        tags: other_tags,
        ..owned(&stolen)
    });
    assert_eq!(stolen.client(true, None).ensure_worker(&stolen.request), Err(MISMATCH));
    assert!(
        !stolen
            .plane
            .calls()
            .iter()
            .any(|call| matches!(call, Call::PutDeployment(..)))
    );
    // A submission that ARM completes synchronously in a failed state is Failed, not Provisioning.
    let sync_failed = Scenario::new();
    sync_failed.plane.lock().submitted_state = Some("Failed");
    let failed = sync_failed
        .client(true, None)
        .ensure_worker(&sync_failed.request)
        .expect("created");
    assert!(matches!(failed, InteractiveWorkerEnsure::Created(_)));
    assert_eq!(failed.status().lifecycle, Lifecycle::Failed);
    let sync_ok = Scenario::new();
    sync_ok.plane.lock().submitted_state = Some("Succeeded");
    let created = sync_ok
        .client(true, None)
        .ensure_worker(&sync_ok.request)
        .expect("created");
    assert_eq!(
        created.status().lifecycle,
        Lifecycle::Provisioning,
        "a synchronous success is provisioning, not unknown"
    );
    // A group ARM is deleting is reported, never repaired or resubmitted.
    let t = sync_failed;
    t.plane
        .script(Some(group_info(&t.group, t.tags.clone(), "Deleting")), None, vec![None]);
    let deleting = t.client(false, None).ensure_worker(&t.request).expect("observed");
    assert_eq!(deleting.status().lifecycle, Lifecycle::Deleting);
    assert!(t.plane.mutations().is_empty());
}

#[test]
fn production_client_is_bound_to_the_profile_before_any_request() {
    use crate::cloud_run::azure::AzureAccessToken;
    let credential = || AzureAccessToken::new("synthetic-token-value", std::time::Duration::from_secs(600));
    let never = |_: CloudWorkflowId, _: CloudJobId, _: &WorkerTarget, _: &str| Ok(false);
    let client = AzureClient::new(profile(), credential, never).expect("client");
    let s = Scenario::new();
    let mut foreign = s.persisted();
    foreign.identity.provider = CloudProvider::RunPod;
    assert_eq!(
        client.inspect_worker(&foreign),
        Err(AzureError::InvalidPersistedWorker),
        "shape checks run before the network is touched"
    );
    let mut invalid = profile();
    invalid.subscription_id = "not-a-subscription".into();
    assert!(matches!(
        AzureClient::new(invalid, credential, never),
        Err(AzureError::InvalidProfile)
    ));
}
