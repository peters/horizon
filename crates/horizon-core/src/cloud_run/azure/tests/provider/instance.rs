//! The VM's instance identity (`vmId`) is pinned across every wait and every key read:
//! a VM deleted and recreated under the same name, carrying the very same tags, is a
//! different machine and is never served, started, stopped or attested as this worker.
use super::*;
use crate::cloud_run::{
    interactive_worker_start::InteractiveWorkerStartProvider,
    interactive_worker_stop::{InteractiveWorkerStop, InteractiveWorkerStopProvider},
};

const OTHER_INSTANCE: &str = "9b8a7c6d-1e2f-4a3b-9c8d-7e6f5a4b3c2d";

/// The same name, path and tags as the scripted worker, on a different instance.
fn recreated(power: &str, s: &Scenario) -> AzureVmView {
    AzureVmView {
        instance_id: Some(OTHER_INSTANCE.into()),
        ..vm(power, &s.tags)
    }
}

fn address() -> AzureDeploymentState {
    deployment("Succeeded", "203.0.113.9")
}

#[test]
fn a_recreated_vm_is_never_this_worker_starting() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false, Some(host_key()));
    // Recreated during the wait: the poll that first sees it stops the wait.
    s.plane.script(
        Some(owned(&s)),
        Some(address()),
        vec![
            Some(vm("deallocated", &s.tags)),
            Some(vm("starting", &s.tags)),
            Some(recreated("running", &s)),
        ],
    );
    assert_eq!(client.start_worker(&worker), Err(MISMATCH), "recreated during the wait");
    // Recreated between the wait's last poll and the answer-time observation.
    s.plane.script(
        Some(owned(&s)),
        Some(address()),
        vec![
            Some(vm("deallocated", &s.tags)),
            Some(vm("running", &s.tags)),
            Some(recreated("running", &s)),
        ],
    );
    assert_eq!(
        client.start_worker(&worker),
        Err(MISMATCH),
        "recreated before the answer"
    );
    // An instance identity that disappears is as foreign as one that changes.
    let unidentified = AzureVmView {
        instance_id: None,
        ..vm("running", &s.tags)
    };
    s.plane.script(
        Some(owned(&s)),
        Some(address()),
        vec![Some(vm("deallocated", &s.tags)), Some(unidentified)],
    );
    assert_eq!(client.start_worker(&worker), Err(MISMATCH), "instance identity lost");
    assert!(
        !s.plane.calls().iter().any(|call| matches!(
            call,
            Call::DeleteGroup(_) | Call::CreateGroup(..) | Call::PutDeployment(..)
        )),
        "start never deletes or creates: {:?}",
        s.plane.calls()
    );
}

#[test]
fn a_recreated_vm_is_never_this_worker_stopping_or_ready() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false, Some(host_key()));
    s.plane.script(
        Some(owned(&s)),
        Some(address()),
        vec![
            Some(vm("running", &s.tags)),
            Some(vm("deallocating", &s.tags)),
            Some(recreated("deallocated", &s)),
        ],
    );
    assert_eq!(
        client.stop_worker(&worker),
        Err(MISMATCH),
        "a recreated VM reaching the target state is not this worker's retained stop"
    );
    assert_eq!(s.plane.mutations(), vec![Call::Deallocate(s.group.clone())]);
    // The slow key read returns to a recreated VM behind the same address.
    s.plane.script(
        Some(owned(&s)),
        Some(address()),
        vec![Some(vm("running", &s.tags)), Some(recreated("running", &s))],
    );
    assert_eq!(
        client.reconcile_worker(&s.request),
        Err(MISMATCH),
        "the attested key never pairs with another instance's address"
    );
    // A worker observed without an instance identity pins nothing: the tags still rule.
    let unidentified = |power: &str| {
        Some(AzureVmView {
            instance_id: None,
            ..vm(power, &s.tags)
        })
    };
    s.plane
        .script(Some(owned(&s)), Some(address()), vec![unidentified("running")]);
    assert_eq!(s.reconcile(Some(host_key())).lifecycle, Lifecycle::Ready);
    s.plane
        .script(Some(owned(&s)), Some(address()), vec![unidentified("deallocated")]);
    assert_eq!(
        client.stop_worker(&worker),
        Ok(InteractiveWorkerStop::Stopped),
        "an already deallocated VM is a retained stop"
    );
}

#[test]
fn another_image_under_this_workers_tags_is_refused_before_any_mutation() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false, Some(host_key()));
    let mut other_image = vm("running", &s.tags);
    other_image.tags.insert(TAG_IMAGE_REF_DIGEST.into(), "0".repeat(64));
    for power in ["running", "deallocated"] {
        let mut vm = other_image.clone();
        vm.power_state = Some(format!("PowerState/{power}"));
        s.plane.script(Some(owned(&s)), Some(address()), vec![Some(vm)]);
        assert_eq!(client.reconcile_worker(&s.request), Err(MISMATCH), "{power}: reconcile");
        assert_eq!(client.inspect_worker(&worker), Err(MISMATCH), "{power}: inspect");
        assert_eq!(client.start_worker(&worker), Err(MISMATCH), "{power}: start");
        assert_eq!(client.stop_worker(&worker), Err(MISMATCH), "{power}: stop");
        assert!(
            s.plane.mutations().is_empty(),
            "{power}: a VM running another image is never served, started or stopped: {:?}",
            s.plane.calls()
        );
        // Deletion is scoped to the owned group, proven by the group's own tags: the
        // group goes, whatever was placed inside it.
        assert_eq!(
            client.delete_worker(&worker),
            Ok(InteractiveWorkerCleanup::Deleted),
            "{power}: delete"
        );
        assert_eq!(s.plane.mutations(), vec![Call::DeleteGroup(s.group.clone())]);
    }
}
