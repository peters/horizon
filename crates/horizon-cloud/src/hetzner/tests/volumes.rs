use super::*;
use crate::{CreateState, Progress};

pub(super) fn volume(id: u64, server: Option<u64>) -> Value {
    json!({"id": id, "name": "horizon-cloud-op-1", "size": 10, "location": {"name": "hel1"},
        "server": server, "linux_device": format!("/dev/disk/by-id/scsi-0HC_Volume_{id}"),
        "status": "available", "labels": {"horizon-operation": OPERATION}, "format": "ext4"})
}

#[test]
fn a_volume_is_created_unattached_as_ext4_behind_the_fence() {
    let (hetzner, requests, task) = provider(vec![
        (200, listing("volumes", json!([]))),
        (
            201,
            json!({"volume": volume(9, None), "action": action(3, "running"), "next_actions": []}),
        ),
        (200, json!({"action": action(3, "success")})),
        (200, json!({"volume": volume(9, None)})),
    ]);
    let mut state = CreateState::Prepared;
    let mut saved = Vec::new();
    let created = hetzner
        .ensure_volume(OPERATION, "hel1", 10, &mut state, &Cancellation::default(), |next| {
            saved.push(next.clone());
            Ok(())
        })
        .unwrap();
    task.join().unwrap();
    assert_eq!(created.linux_device, "/dev/disk/by-id/scsi-0HC_Volume_9");
    assert_eq!(
        saved,
        [CreateState::Requested, CreateState::Bound { worker_id: "9".into() }]
    );
    let body = request_body(&requests.lock().unwrap()[1]);
    assert_eq!(
        body,
        json!({"name": "horizon-cloud-op-1", "size": 10, "location": "hel1", "format": "ext4",
            "labels": {"horizon-operation": OPERATION}})
    );
}

#[test]
fn an_uncertain_volume_create_reconciles_and_invalid_sizes_stay_local() {
    let (hetzner, requests, task) = provider(vec![
        (200, listing("volumes", json!([]))),
        (502, Value::Null),
        (200, listing("volumes", json!([volume(9, None)]))),
    ]);
    let cancel = Cancellation::default();
    let mut state = CreateState::Prepared;
    assert!(
        hetzner
            .ensure_volume(OPERATION, "hel1", 10, &mut state, &cancel, |_| Ok(()))
            .is_err()
    );
    assert_eq!(state, CreateState::Requested);
    assert_eq!(
        hetzner
            .ensure_volume(OPERATION, "hel1", 10, &mut state, &cancel, |_| Ok(()))
            .unwrap()
            .id,
        9
    );
    for (location, size) in [("hel1", 9), ("hel1", 10_241), ("Hel 1", 10)] {
        assert!(matches!(
            hetzner.ensure_volume(OPERATION, location, size, &mut CreateState::Prepared, &cancel, |_| Ok(
                ()
            )),
            Err(CloudError::Invalid(_))
        ));
    }
    task.join().unwrap();
    assert_eq!(requests.lock().unwrap().len(), 3);
}

#[test]
fn attaching_requires_both_identities_one_location_and_a_free_volume() {
    let mut elsewhere = servers::server(42);
    elsewhere["location"]["name"] = json!("nbg1");
    let (hetzner, requests, task) = provider(vec![
        (200, json!({"volume": volume(9, None)})),
        (200, json!({"server": servers::server(42)})),
        (201, json!({"action": action(4, "success")})),
        (200, json!({"volume": volume(9, None)})),
        (200, json!({"server": elsewhere})),
        (200, json!({"volume": volume(9, Some(41))})),
        (200, json!({"server": servers::server(42)})),
        (200, json!({"volume": volume(9, Some(42))})),
        (201, json!({"action": action(5, "success")})),
    ]);
    let cancel = Cancellation::default();
    hetzner.attach(OPERATION, 9, 42, &cancel).unwrap();
    assert!(matches!(
        hetzner.attach(OPERATION, 9, 42, &cancel),
        Err(CloudError::Invalid(_))
    ));
    assert!(matches!(
        hetzner.attach(OPERATION, 9, 42, &cancel),
        Err(CloudError::Invalid(_))
    ));
    hetzner.detach(OPERATION, 9, &cancel).unwrap();
    task.join().unwrap();
    let requests = requests.lock().unwrap();
    assert!(requests[2].starts_with("POST /volumes/9/actions/attach "));
    assert_eq!(request_body(&requests[2]), json!({"server": 42, "automount": false}));
    assert!(requests[8].starts_with("POST /volumes/9/actions/detach "));
}

#[test]
fn deletion_refuses_attached_volumes_and_proves_absence() {
    let (hetzner, requests, task) = provider(vec![
        (200, json!({"volume": volume(9, Some(42))})),
        (200, json!({"volume": volume(9, None)})),
        (204, Value::Null),
        (404, error("not_found", "volume not found")),
    ]);
    let cancel = Cancellation::default();
    let mut state = CreateState::Bound { worker_id: "9".into() };
    assert!(matches!(
        hetzner.delete_volume(OPERATION, &mut state, &cancel, |_| Ok(()), |_| {}),
        Err(CloudError::Invalid(_))
    ));
    assert_eq!(state, CreateState::Bound { worker_id: "9".into() });
    let mut reported = Vec::new();
    hetzner
        .delete_volume(OPERATION, &mut state, &cancel, |_| Ok(()), |step| reported.push(step))
        .unwrap();
    assert_eq!(state, CreateState::Terminated { worker_id: "9".into() });
    assert_eq!(
        reported,
        [
            Progress::ConfirmingVolume,
            Progress::DeletingVolume,
            Progress::ConfirmingVolumeDeletion
        ]
    );
    task.join().unwrap();
    assert!(requests.lock().unwrap()[2].starts_with("DELETE /volumes/9 "));
}
