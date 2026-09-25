use super::*;
use crate::cloud_runtime::project_reservations::{
    self as reservations, Change, Error as ReservationError, Reservation,
};
use horizon_cloud_protocol::{
    ProjectId, ProjectIdentity,
    bootstrap::RecoveryRequest,
    membership::{Manifest, Receipt, State},
};

fn ready() -> CoordinatorFixture {
    let mut f = CoordinatorFixture::new();
    f.record.phase = Phase::Completed;
    f.record.save(&mut f.owner).unwrap();
    f
}
fn reservation(f: &CoordinatorFixture, name: &str, port: u16) -> Reservation {
    Reservation {
        project: ProjectIdentity::new(ProjectId::generate(), "session".into(), "workspace".into(), name.into())
            .unwrap(),
        image_digest: f.record.spec.image_digest.clone(),
        capabilities: serde_json::from_str("{}").unwrap(),
        ports: [port].into(),
    }
}
fn remote(f: &CoordinatorFixture) -> Manifest {
    Manifest::empty(f.record.startup.clone().unwrap(), "worker-one".into())
}
fn apply(worker: &mut Manifest, bytes: &[u8]) -> Vec<u8> {
    let request: RecoveryRequest = serde_json::from_slice(bytes).unwrap();
    let (next, receipt) = worker.next(&request.message, &request.payload).unwrap();
    *worker = next;
    serde_json::to_vec(&receipt).unwrap()
}
fn transact(
    f: &mut CoordinatorFixture,
    change: &Change,
    exchange: &mut impl FnMut(&Connection, &str, &[u8]) -> reservations::Result<Vec<u8>>,
) -> reservations::Result<Receipt> {
    let target = resolve(&f.record).unwrap();
    reservations::coordinate(&mut f.owner, &target, &f.record.spec.image_digest, change, exchange)
}
fn journal(owner: &Owner) -> serde_json::Value {
    owner.load().unwrap()["project_reservations"].clone()
}

#[test]
fn lost_reply_reopens_exact_operation_and_blocks_changed_successors() {
    let mut f = ready();
    let request = reservation(&f, "one", 8000);
    let mut worker = remote(&f);
    let mut first = Vec::new();
    assert!(
        transact(&mut f, &Change::Reserve(request.clone()), &mut |_, command, bytes| {
            assert_eq!(command, "horizon-cloud-worker reserve-project");
            first = bytes.to_vec();
            apply(&mut worker, bytes);
            Err(ReservationError::Invalid)
        })
        .is_err()
    );
    assert!(reservations::started(&f.owner).unwrap());
    assert_bootstrap_fenced(&mut f);
    assert_eq!(journal(&f.owner)["manifest"]["revision"], 0);
    assert_eq!(journal(&f.owner)["pending"]["next"]["revision"], 1);
    let changed = reservation(&f, "two", 8001);
    assert!(matches!(
        transact(&mut f, &Change::Reserve(changed), &mut |_, _, _| panic!(
            "pending successor"
        )),
        Err(ReservationError::Pending)
    ));
    assert!(matches!(
        transact(&mut f, &Change::Cancel(request.project.clone()), &mut |_, _, _| panic!(
            "pending cancellation"
        )),
        Err(ReservationError::Pending)
    ));
    let mut f = f.reopen();
    let receipt = transact(&mut f, &Change::Resume, &mut |_, _, bytes| {
        assert_eq!(bytes, first);
        Ok(apply(&mut worker, bytes))
    })
    .unwrap();
    assert_eq!(receipt.revision, 1);
    assert!(journal(&f.owner)["pending"].is_null());
    assert_eq!(journal(&f.owner)["manifest"], serde_json::to_value(&worker).unwrap());
    let repeated = transact(&mut f, &Change::Reserve(request.clone()), &mut |_, _, bytes| {
        assert_eq!(bytes, first);
        Ok(apply(&mut worker, bytes))
    })
    .unwrap();
    assert_eq!(repeated, receipt);
    assert_eq!(worker.operations.len(), 1);
    let mut changed = request;
    changed.ports.insert(8002);
    assert!(
        transact(&mut f, &Change::Reserve(changed), &mut |_, _, _| panic!(
            "changed completed reserve"
        ))
        .is_err()
    );
}

#[test]
fn failed_pending_anchor_never_sends_and_completion_failure_reuses_request() {
    for writes in [0, 1] {
        let mut f = ready();
        let request = reservation(&f, "one", 8000);
        f.vault.fail_after(Some(writes));
        assert!(
            transact(&mut f, &Change::Reserve(request.clone()), &mut |_, _, _| panic!(
                "unanchored send"
            ))
            .is_err()
        );
        let mut f = f.reopen();
        let mut worker = remote(&f);
        transact(&mut f, &Change::Reserve(request), &mut |_, _, bytes| {
            Ok(apply(&mut worker, bytes))
        })
        .unwrap();
        assert_eq!(worker.operations.len(), 1);
    }
    for writes in [0, 1] {
        let mut f = ready();
        let request = reservation(&f, "one", 8000);
        let vault = f.vault.clone();
        let mut worker = remote(&f);
        let mut first = Vec::new();
        assert!(
            transact(&mut f, &Change::Reserve(request.clone()), &mut |_, _, bytes| {
                first = bytes.to_vec();
                let reply = apply(&mut worker, bytes);
                vault.fail_after(Some(writes));
                Ok(reply)
            })
            .is_err()
        );
        let mut f = f.reopen();
        let receipt = transact(&mut f, &Change::Reserve(request), &mut |_, _, bytes| {
            assert_eq!(bytes, first);
            Ok(apply(&mut worker, bytes))
        })
        .unwrap();
        assert_eq!(receipt.revision, 1);
        assert_eq!(worker.operations.len(), 1);
    }
}

#[test]
fn bad_receipts_retain_pending_and_changed_pins_block_completion() {
    for variant in 0..4 {
        let mut f = ready();
        let request = reservation(&f, "one", 8000);
        let mut worker = remote(&f);
        let pin_path = f.record.request.known_hosts.clone();
        let pin = fs::read(&pin_path).unwrap();
        assert!(
            transact(&mut f, &Change::Reserve(request), &mut |_, _, bytes| {
                let mut reply: serde_json::Value = serde_json::from_slice(&apply(&mut worker, bytes)).unwrap();
                match variant {
                    0 => reply["revision"] = 2.into(),
                    1 => reply["state"] = "removed".into(),
                    2 => return Ok(vec![b' '; 64 * 1024 + 1]),
                    _ => fs::write(&pin_path, b"changed during transport").unwrap(),
                }
                Ok(serde_json::to_vec(&reply).unwrap())
            })
            .is_err()
        );
        assert_eq!(journal(&f.owner)["manifest"]["revision"], 0);
        fs::write(pin_path, pin).unwrap();
        let mut f = f.reopen();
        assert_eq!(
            transact(&mut f, &Change::Resume, &mut |_, _, bytes| Ok(apply(
                &mut worker,
                bytes
            )))
            .unwrap()
            .revision,
            1
        );
    }
}

#[test]
fn foreign_and_corrupt_journals_cannot_send() {
    let mut source = ready();
    let request = reservation(&source, "one", 8000);
    assert!(
        transact(&mut source, &Change::Reserve(request), &mut |_, _, _| Err(
            ReservationError::Invalid
        ))
        .is_err()
    );
    let payload = source.owner.load().unwrap();
    for variant in 0..5 {
        let mut f = source.reopen();
        let mut changed = payload.clone();
        match variant {
            0 => changed["project_reservations"]["pending"]["receipt"]["revision"] = 99.into(),
            1 => changed["project_reservations"]["pending"]["request"] = "{}".into(),
            2 => changed["project_reservations"]["pending"]["next"]["members"][0]["namespace"] = "sibling".into(),
            3 => changed["project_reservations"]["version"] = 99.into(),
            _ => changed["project_reservations"]["manifest"]["worker_id"] = "other".into(),
        }
        f.owner.save(changed).unwrap();
        assert!(transact(&mut f, &Change::Resume, &mut |_, _, _| panic!("corrupt journal")).is_err());
        f.owner.save(payload.clone()).unwrap();
        source = f;
    }
    let mut foreign = ready();
    let mut value = foreign.owner.load().unwrap();
    value["project_reservations"] = payload["project_reservations"].clone();
    foreign.owner.save(value).unwrap();
    assert!(
        transact(&mut foreign, &Change::Resume, &mut |_, _, _| panic!(
            "foreign controller"
        ))
        .is_err()
    );
}

#[test]
fn cancellation_is_terminal_and_all_reservation_history_fences_bootstrap() {
    let mut f = ready();
    let first = reservation(&f, "one", 8000);
    let second = reservation(&f, "two", 8001);
    let mut worker = remote(&f);
    for request in [&first, &second] {
        transact(&mut f, &Change::Reserve(request.clone()), &mut |_, _, bytes| {
            Ok(apply(&mut worker, bytes))
        })
        .unwrap();
    }
    let sibling = worker.members[1].clone();
    let cancel = Change::Cancel(first.project.clone());
    let receipt = transact(&mut f, &cancel, &mut |_, command, bytes| {
        assert_eq!(command, "horizon-cloud-worker cancel-project-reservation");
        Ok(apply(&mut worker, bytes))
    })
    .unwrap();
    assert_eq!(receipt.state, State::Removed);
    let mut f = f.reopen();
    assert_eq!(
        receipt,
        transact(&mut f, &cancel, &mut |_, _, bytes| Ok(apply(&mut worker, bytes))).unwrap()
    );
    assert_eq!(worker.members[1], sibling);
    assert!(
        transact(&mut f, &Change::Reserve(first), &mut |_, _, _| panic!(
            "terminal attach"
        ))
        .is_err()
    );
    transact(&mut f, &Change::Cancel(second.project), &mut |_, _, bytes| {
        Ok(apply(&mut worker, bytes))
    })
    .unwrap();
    assert!(worker.members.iter().all(|member| member.state == State::Removed));
    assert_bootstrap_fenced(&mut f);
    let mut payload = f.owner.load().unwrap();
    payload["project_reservations"] = serde_json::Value::Null;
    f.owner.save(payload).unwrap();
    assert_bootstrap_fenced(&mut f);
}
fn assert_bootstrap_fenced(f: &mut CoordinatorFixture) {
    assert!(matches!(
        cleanup_with(
            &mut f.owner,
            &mut f.record,
            &mut |_| panic!("provider resolve"),
            &mut |_, _| panic!("abandon send"),
            &mut |_, _| panic!("provider delete")
        ),
        Err(Error::Membership)
    ));
    let cancel = Cancellation::default();
    assert!(matches!(
        resume(&mut f.owner, &f.record.request, &cancel, Duration::from_secs(1)),
        Err(Error::Membership)
    ));
    assert!(matches!(
        inspection::inspect(
            &f.owner,
            &f.record.request,
            &f.record.spec.image_digest,
            &Capabilities::default(),
            &cancel,
            Duration::from_secs(1)
        ),
        Err(Error::Membership)
    ));
}

#[test]
fn expired_changed_or_uninitialized_context_rejects_before_provider_io() {
    let mut f = ready();
    let request = reservation(&f, "one", 8000);
    let cancel = Cancellation::default();
    assert!(matches!(
        reservations::reserve(&mut f.owner, &f.record.request, &request, &cancel, Duration::ZERO),
        Err(ReservationError::Deadline)
    ));
    assert!(!reservations::started(&f.owner).unwrap());
    let mut changed = request.clone();
    changed.image_digest = format!("other/worker@sha256:{}", "b".repeat(64));
    assert!(matches!(
        reservations::reserve(
            &mut f.owner,
            &f.record.request,
            &changed,
            &cancel,
            Duration::from_secs(1)
        ),
        Err(ReservationError::Bootstrap(Error::Invalid))
    ));
    f.record.phase = Phase::Requested;
    f.record.save(&mut f.owner).unwrap();
    assert!(matches!(
        reservations::reserve(
            &mut f.owner,
            &f.record.request,
            &request,
            &cancel,
            Duration::from_secs(1)
        ),
        Err(ReservationError::Bootstrap(Error::Invalid))
    ));
    assert!(!reservations::started(&f.owner).unwrap());
}

#[test]
#[ignore = "requires scripts/cloud-initialization-smoke.py --scenario host-reservations and an isolated mount namespace"]
fn native_ssh_host_reservation_recovery() {
    let directory = PathBuf::from(std::env::var_os("HORIZON_INITIALIZATION_FIXTURE").unwrap());
    let port = u16::try_from(serde_json::from_slice::<serde_json::Value>(&fs::read(directory.join("fixture.json")).unwrap()).unwrap()["port"].as_u64().unwrap()).unwrap();
    let mut f = native_fixture(&directory);
    let cancellation = Cancellation::default();
    let runner = runner(&cancellation);
    let target = target(&f.record, ([127, 0, 0, 1], port).into()).unwrap();
    enroll(&f.record, &target, &runner, startup_deadline()).unwrap();
    initialize(&mut f.owner, &mut f.record, &target, &runner, startup_deadline()).unwrap();
    complete(&mut f.owner, &mut f.record, &target, &cancellation, startup_deadline()).unwrap();
    let key = fs::read(directory.join("workspace/.horizon-allocation/ssh-host-key")).unwrap();
    f = native_reservations(f, &target, &runner, &directory);
    assert_eq!(
        fs::read(directory.join("workspace/.horizon-allocation/ssh-host-key")).unwrap(),
        key
    );
    let worker: Manifest =
        serde_json::from_slice(&fs::read(directory.join("workspace/.horizon-allocation/membership.json")).unwrap())
            .unwrap();
    worker.validate().unwrap();
    assert_eq!(journal(&f.owner)["manifest"], serde_json::to_value(&worker).unwrap());
    assert_eq!(worker.revision, 4);
    assert_eq!(worker.members[0].state, State::Removed);
    assert!(
        worker.members[1..]
            .iter()
            .all(|member| member.state == State::Attaching)
    );
    assert_bootstrap_fenced(&mut f);
    assert_eq!(fs::read_dir(directory.join("workspace")).unwrap().count(), 1);
}

fn native_reservations(
    mut f: CoordinatorFixture,
    target: &bootstrap_recovery::Target,
    runner: &Runner<'_>,
    directory: &std::path::Path,
) -> CoordinatorFixture {
    let first = reservation(&f, "one", 8000);
    let image = first.image_digest.clone();
    let mut sent = Vec::new();
    assert!(
        reservations::coordinate(
            &mut f.owner,
            target,
            &image,
            &Change::Reserve(first.clone()),
            &mut |connection, command, bytes| {
                sent = bytes.to_vec();
                runner.private_exchange(&mut connection.pinned_command(command), bytes, Duration::from_secs(30))?;
                Err(ReservationError::Invalid)
            }
        )
        .is_err()
    );
    fs::write(
        directory.join("restart"),
        b"host and worker restart after lost reserve reply",
    )
    .unwrap();
    f = f.reopen();
    let receipt = reservations::coordinate(
        &mut f.owner,
        target,
        &image,
        &Change::Resume,
        &mut |connection, command, bytes| {
            assert_eq!(bytes, sent);
            Ok(runner.private_exchange(&mut connection.pinned_command(command), bytes, Duration::from_secs(30))?)
        },
    )
    .unwrap();
    assert_eq!(receipt.revision, 1);
    let second = reservation(&f, "two", 8001);
    let third = reservation(&f, "three", 8002);
    let mut exchange = |connection: &Connection, command: &str, bytes: &[u8]| -> reservations::Result<Vec<u8>> {
        Ok(runner.private_exchange(&mut connection.pinned_command(command), bytes, Duration::from_secs(30))?)
    };
    for request in [second, third] {
        reservations::coordinate(&mut f.owner, target, &image, &Change::Reserve(request), &mut exchange).unwrap();
    }
    assert_eq!(
        receipt,
        reservations::coordinate(
            &mut f.owner,
            target,
            &image,
            &Change::Reserve(first.clone()),
            &mut exchange
        )
        .unwrap()
    );
    native_cancel(f, target, runner, directory, first)
}

fn native_cancel(
    mut f: CoordinatorFixture,
    target: &bootstrap_recovery::Target,
    runner: &Runner<'_>,
    directory: &std::path::Path,
    first: Reservation,
) -> CoordinatorFixture {
    let image = first.image_digest.clone();
    let mut exchange = |connection: &Connection, command: &str, bytes: &[u8]| -> reservations::Result<Vec<u8>> {
        Ok(runner.private_exchange(&mut connection.pinned_command(command), bytes, Duration::from_secs(30))?)
    };
    let siblings = journal(&f.owner)["manifest"]["members"].as_array().unwrap()[1..].to_vec();
    let vault = f.vault.clone();
    assert!(
        reservations::coordinate(
            &mut f.owner,
            target,
            &image,
            &Change::Cancel(first.project.clone()),
            &mut |connection, command, bytes| {
                let response = exchange(connection, command, bytes)?;
                vault.fail_after(Some(0));
                Ok(response)
            }
        )
        .is_err()
    );
    fs::write(
        directory.join("restart"),
        b"host and worker restart after cancellation save failure",
    )
    .unwrap();
    f = f.reopen();
    let receipt = reservations::coordinate(&mut f.owner, target, &image, &Change::Resume, &mut exchange).unwrap();
    assert_eq!(receipt.state, State::Removed);
    assert_eq!(
        receipt,
        reservations::coordinate(
            &mut f.owner,
            target,
            &image,
            &Change::Cancel(first.project.clone()),
            &mut exchange
        )
        .unwrap()
    );
    assert!(
        reservations::coordinate(
            &mut f.owner,
            target,
            &image,
            &Change::Reserve(first),
            &mut |_, _, _| panic!("terminal local attach")
        )
        .is_err()
    );
    assert_eq!(
        journal(&f.owner)["manifest"]["members"].as_array().unwrap()[1..],
        siblings
    );
    f
}

#[test]
fn owner_revocation_during_valid_exchange_cannot_acknowledge_completion() {
    let mut f = ready();
    let request = reservation(&f, "one", 8000);
    let mut worker = remote(&f);
    let path = f.root.join("owner.json");
    let original = fs::read(&path).unwrap();
    assert!(
        transact(&mut f, &Change::Reserve(request), &mut |_, _, bytes| {
            let reply = apply(&mut worker, bytes);
            fs::write(&path, b"revoked during exchange").unwrap();
            Ok(reply)
        })
        .is_err()
    );
    fs::write(path, original).unwrap();
    let mut f = f.reopen();
    assert_eq!(journal(&f.owner)["manifest"]["revision"], 0);
    assert_eq!(
        transact(&mut f, &Change::Resume, &mut |_, _, bytes| Ok(apply(
            &mut worker,
            bytes
        )))
        .unwrap()
        .revision,
        1
    );
    assert_eq!(worker.operations.len(), 1);
}
