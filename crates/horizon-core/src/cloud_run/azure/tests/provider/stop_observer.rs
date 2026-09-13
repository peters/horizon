//! `Check saved Stop`: one read-only observation of the exact saved worker. Retained
//! Stop needs deallocated compute, the retained data disk and the saved address; absence
//! is absence, everything uncertain is pending, and nothing is ever mutated.
use super::*;
use crate::cloud_run::{
    interactive_worker::InteractiveWorkerSshEndpoint,
    interactive_worker_start::{InteractiveWorkerStart, InteractiveWorkerStartProvider},
    interactive_worker_stop::{
        InteractiveWorkerStop, InteractiveWorkerStopExpectation, InteractiveWorkerStopObservation as Observation,
        InteractiveWorkerStopObserver, InteractiveWorkerStopProvider,
    },
    runpod::RunPodNetworkVolumeExpectation,
};

const HOST: &str = "203.0.113.9";

fn pin() -> InteractiveWorkerSshEndpoint {
    InteractiveWorkerSshEndpoint {
        host: HOST.into(),
        port: SSH_PORT,
        username: SSH_USERNAME.into(),
        host_key: host_key(),
    }
}

/// The scripted VM with the retained disk under the scenario's own group.
fn retained_vm(s: &Scenario, power: &str) -> AzureVmView {
    let mut view = vm(power, &s.tags);
    view.data_disks = vec![AzureDataDisk {
        id: format!("{}/providers/Microsoft.Compute/disks/worker-data", s.group_id),
        ..retained_disk()
    }];
    view
}

fn observe(s: &Scenario, pin: &InteractiveWorkerSshEndpoint) -> Result<Observation, AzureError> {
    let worker = s.persisted();
    s.client(false, Some(host_key()))
        .observe_worker_stop(InteractiveWorkerStopExpectation {
            worker: &worker,
            ssh: pin,
            network_volume: None,
        })
}

fn assert_read_only(s: &Scenario) {
    assert!(
        s.plane.mutations().is_empty() && !s.plane.calls().iter().any(|call| matches!(call, Call::Run(..))),
        "an observation never mutates, runs a command or connects: {:?}",
        s.plane.calls()
    );
}

#[test]
fn only_deallocated_compute_with_the_retained_disk_and_address_is_retained_stopped() {
    let s = Scenario::new();
    let cases = [
        ("deallocated", Ok(Observation::RetainedStopped)),
        ("running", Ok(Observation::Pending)),
        ("starting", Ok(Observation::Pending)),
        ("deallocating", Ok(Observation::Pending)),
        // Guest halted but compute still allocated and billed: not a retained stop.
        ("stopped", Ok(Observation::Pending)),
        ("unknown", Ok(Observation::Pending)),
    ];
    for (power, expected) in cases {
        s.plane.script(
            Some(owned(&s)),
            Some(deployment("Succeeded", HOST)),
            vec![Some(retained_vm(&s, power))],
        );
        assert_eq!(observe(&s, &pin()), expected, "{power}");
        assert_read_only(&s);
    }
    // A failed deployment cannot vouch for retention: an error, never a state.
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Failed", HOST)),
        vec![Some(retained_vm(&s, "deallocated"))],
    );
    assert_eq!(observe(&s, &pin()), Err(AzureError::StopUnverified));
    // A group being deleted is not retained storage.
    s.plane.script(
        Some(group_info(&s.group, s.tags.clone(), "Deleting")),
        Some(deployment("Succeeded", HOST)),
        vec![Some(retained_vm(&s, "deallocated"))],
    );
    assert_eq!(observe(&s, &pin()), Ok(Observation::Pending));
    assert_read_only(&s);
}

#[test]
fn absence_stays_absence_and_uncertain_storage_stays_pending() {
    let s = Scenario::new();
    s.plane.script(None, None, vec![None]);
    assert_eq!(observe(&s, &pin()), Ok(Observation::Absent), "no owned group");
    // The group is there but the VM is not (deployment still provisioning): pending.
    s.plane
        .script(Some(owned(&s)), Some(deployment("Running", HOST)), vec![None]);
    assert_eq!(observe(&s, &pin()), Ok(Observation::Pending));
    // Storage that cannot be recognised as the retained disk is never promoted.
    let other_group =
        format!("/subscriptions/{SUB}/resourceGroups/other/providers/Microsoft.Compute/disks/worker-data");
    let retained = retained_vm(&s, "deallocated").data_disks.remove(0);
    let doubtful: Vec<(&str, Vec<AzureDataDisk>)> = vec![
        ("no disk", vec![]),
        ("two disks", vec![retained.clone(), retained.clone()]),
        (
            "deleted with the VM",
            vec![AzureDataDisk {
                delete_option: "Delete".into(),
                ..retained.clone()
            }],
        ),
        (
            "other LUN",
            vec![AzureDataDisk {
                lun: Some(1),
                ..retained.clone()
            }],
        ),
        (
            "no LUN",
            vec![AzureDataDisk {
                lun: None,
                ..retained.clone()
            }],
        ),
        (
            "other name",
            vec![AzureDataDisk {
                id: retained.id.replace("worker-data", "scratch"),
                ..retained.clone()
            }],
        ),
        (
            "other group",
            vec![AzureDataDisk {
                id: other_group,
                ..retained.clone()
            }],
        ),
    ];
    for (label, disks) in doubtful {
        let mut view = retained_vm(&s, "deallocated");
        view.data_disk_count = disks.len();
        view.data_disks = disks;
        s.plane
            .script(Some(owned(&s)), Some(deployment("Succeeded", HOST)), vec![Some(view)]);
        assert_eq!(observe(&s, &pin()), Ok(Observation::Pending), "{label}");
    }
    // A storage entry the view could not represent (no managed-disk ID) is storage
    // nobody vouched for: the count says two, the list says one.
    let mut view = retained_vm(&s, "deallocated");
    view.data_disk_count = 2;
    s.plane
        .script(Some(owned(&s)), Some(deployment("Succeeded", HOST)), vec![Some(view)]);
    assert_eq!(observe(&s, &pin()), Ok(Observation::Pending), "unrepresented entry");
    // No current deployment address (in flight or unusable): uncertain, so pending, not
    // a mismatch.
    for state in [deployment("Running", HOST), deployment("Succeeded", "10.0.0.5")] {
        s.plane
            .script(Some(owned(&s)), Some(state), vec![Some(retained_vm(&s, "deallocated"))]);
        assert_eq!(observe(&s, &pin()), Ok(Observation::Pending));
    }
    s.plane
        .script(Some(owned(&s)), None, vec![Some(retained_vm(&s, "deallocated"))]);
    assert_eq!(observe(&s, &pin()), Ok(Observation::Pending), "no deployment at all");
    // The disk ID compares case-insensitively, as every ARM path does.
    let mut view = retained_vm(&s, "deallocated");
    view.data_disks[0].id = view.data_disks[0].id.to_uppercase();
    s.plane
        .script(Some(owned(&s)), Some(deployment("Succeeded", HOST)), vec![Some(view)]);
    assert_eq!(observe(&s, &pin()), Ok(Observation::RetainedStopped));
    assert_read_only(&s);
}

#[test]
fn saved_identity_and_pin_are_validated_before_and_during_the_read() {
    let s = Scenario::new();
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Succeeded", HOST)),
        vec![Some(retained_vm(&s, "deallocated"))],
    );
    // Malformed or foreign expectations never reach the control plane.
    let mut wrong_port = pin();
    wrong_port.port = 22;
    let mut wrong_user = pin();
    wrong_user.username = "horizon".into();
    let mut wrong_host = pin();
    wrong_host.host = "worker.example".into();
    let mut wrong_key = pin();
    wrong_key.host_key = "ssh-rsa AAAA".into();
    for (label, broken) in [
        ("port", wrong_port),
        ("user", wrong_user),
        ("host", wrong_host),
        ("key", wrong_key),
    ] {
        s.plane.lock().calls.clear();
        assert_eq!(observe(&s, &broken), Err(AzureError::InvalidPersistedWorker), "{label}");
        assert!(s.plane.calls().is_empty(), "{label}: refused before any request");
    }
    let worker = s.persisted();
    let volume = RunPodNetworkVolumeExpectation {
        volume_id: "vol".into(),
        data_center_id: "dc".into(),
        minimum_size_gb: 10,
    };
    assert_eq!(
        s.client(false, Some(host_key()))
            .observe_worker_stop(InteractiveWorkerStopExpectation {
                worker: &worker,
                ssh: &pin(),
                network_volume: Some(&volume),
            }),
        Err(AzureError::InvalidTarget),
        "a network volume is a RunPod binding"
    );
    let mut foreign = worker.clone();
    foreign.identity.resource_id = format!("/subscriptions/{OTHER_SUB}/resourceGroups/{}", s.group);
    assert_eq!(
        s.client(false, Some(host_key()))
            .observe_worker_stop(InteractiveWorkerStopExpectation {
                worker: &foreign,
                ssh: &pin(),
                network_volume: None,
            }),
        Err(AzureError::InvalidPersistedWorker)
    );
    // The saved address must still be the one the deployment reports, whatever the
    // compute is doing: a moved address on a running or transitioning worker is a
    // mismatch too, never something to keep retrying against.
    let mut moved = pin();
    moved.host = "203.0.113.10".into();
    assert_eq!(observe(&s, &moved), Err(AzureError::ResourceIdentityMismatch));
    for power in ["running", "starting", "stopped"] {
        s.plane.script(
            Some(owned(&s)),
            Some(deployment("Succeeded", HOST)),
            vec![Some(retained_vm(&s, power))],
        );
        assert_eq!(
            observe(&s, &moved),
            Err(AzureError::ResourceIdentityMismatch),
            "{power}"
        );
    }
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Succeeded", HOST)),
        vec![Some(retained_vm(&s, "deallocated"))],
    );
    // A retagged group or VM at the same path is not this worker.
    let mut retagged = s.tags.clone();
    retagged.insert(TAG_JOB.into(), CloudJobId::new().to_string());
    s.plane.script(
        Some(group_info(&s.group, retagged.clone(), "Succeeded")),
        Some(deployment("Succeeded", HOST)),
        vec![Some(retained_vm(&s, "deallocated"))],
    );
    assert_eq!(observe(&s, &pin()), Err(MISMATCH));
    let mut foreign_vm = retained_vm(&s, "deallocated");
    foreign_vm.tags = retagged;
    s.plane.script(
        Some(owned(&s)),
        Some(deployment("Succeeded", HOST)),
        vec![Some(foreign_vm)],
    );
    assert_eq!(observe(&s, &pin()), Err(MISMATCH));
    assert_read_only(&s);
}

/// The retained disk through a whole Stop, Check saved Stop, Start cycle on one
/// instance: the plane keeps the disk attached across deallocation (the template's
/// `Detach` delete option), the observer confirms it exactly then, and the started
/// worker comes back on the same instance with the same disk. The observation itself
/// issues no mutation at any point of the cycle.
#[test]
fn the_retained_disk_survives_a_stop_observe_start_cycle_on_the_same_instance() {
    let s = Scenario::new();
    let worker = s.persisted();
    let client = s.client(false, Some(host_key()));
    let address = || Some(deployment("Succeeded", HOST));
    let disk_of = |view: &AzureVmView| (view.instance_id.clone(), view.data_disks.clone(), view.data_disk_count);
    let running = retained_vm(&s, "running");
    let retained = disk_of(&running);
    // Running: observed only, nothing to confirm yet.
    s.plane.script(Some(owned(&s)), address(), vec![Some(running.clone())]);
    assert_eq!(observe(&s, &pin()), Ok(Observation::Pending));
    assert_read_only(&s);
    // Stop: the deallocation is posted once and the disk stays attached throughout.
    s.plane.script(
        Some(owned(&s)),
        address(),
        vec![
            Some(running.clone()),
            Some(retained_vm(&s, "deallocating")),
            Some(retained_vm(&s, "deallocated")),
        ],
    );
    assert_eq!(client.stop_worker(&worker), Ok(InteractiveWorkerStop::Stopped));
    assert_eq!(s.plane.mutations(), [Call::Deallocate(s.group.clone())]);
    // Check saved Stop against the state the stop left behind, not a fresh script:
    // deallocated compute, the same disk, the saved address.
    let after_stop = s.plane.lock().vm_states.clone();
    assert_eq!(
        after_stop.iter().flatten().map(disk_of).collect::<Vec<_>>(),
        std::slice::from_ref(&retained),
        "the stop's final VM state keeps the disk on the same instance"
    );
    s.plane.lock().calls.clear();
    assert_eq!(observe(&s, &pin()), Ok(Observation::RetainedStopped));
    assert_read_only(&s);
    // Start: the same instance comes back with the same disk and is ready through the
    // attested key; the observer then reports the running worker as pending again.
    s.plane.script(
        Some(owned(&s)),
        address(),
        vec![
            Some(retained_vm(&s, "deallocated")),
            Some(retained_vm(&s, "starting")),
            Some(running),
        ],
    );
    let started = client.start_worker(&worker).expect("start");
    let InteractiveWorkerStart::Started(status) = &started else {
        panic!("the retained worker starts: {started:?}");
    };
    assert_eq!(status.lifecycle, Lifecycle::Ready);
    assert_eq!(s.plane.mutations(), [Call::Start(s.group.clone())]);
    let after_start = s.plane.lock().vm_states.clone();
    assert_eq!(
        after_start.iter().flatten().map(disk_of).collect::<Vec<_>>(),
        [retained],
        "the started instance carries the retained disk"
    );
    s.plane.lock().calls.clear();
    assert_eq!(observe(&s, &pin()), Ok(Observation::Pending));
    assert_read_only(&s);
    // A rebuilt worker on another instance without the retained disk is never confirmed
    // as the saved Stop: retention is proven by the disk, not by the power state.
    let mut replaced = retained_vm(&s, "deallocated");
    replaced.instance_id = Some("7d6c5b4a-3928-4170-a6b5-c4d3e2f1a0b9".into());
    replaced.data_disks.clear();
    replaced.data_disk_count = 0;
    s.plane.script(Some(owned(&s)), address(), vec![Some(replaced)]);
    assert_eq!(observe(&s, &pin()), Ok(Observation::Pending));
    assert_read_only(&s);
}
