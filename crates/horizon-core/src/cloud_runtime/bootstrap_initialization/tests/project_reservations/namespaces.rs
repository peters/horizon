#[cfg(target_os = "linux")]
mod source;
use super::*;

#[test]
fn pending_preparation_reopens_exact_action_and_does_not_replay_reserve() {
    let mut f = ready();
    let request = reservation(&f, "one", 8000);
    let mut worker = remote(&f);
    let reserve = transact(&mut f, &Change::Reserve(request.clone()), &mut |_, _, bytes| {
        Ok(apply(&mut worker, bytes))
    })
    .unwrap();
    let mut sent = Vec::new();
    assert!(
        transact(
            &mut f,
            &Change::PrepareNamespace(request.project.clone()),
            &mut |_, command, bytes| {
                assert_eq!(command, "horizon-cloud-worker prepare-project-namespace");
                sent = bytes.to_vec();
                apply(&mut worker, bytes);
                Err(ReservationError::Invalid)
            }
        )
        .is_err()
    );
    assert_eq!(worker.members[0].state, State::Preparing);
    assert!(matches!(
        transact(&mut f, &Change::Cancel(request.project.clone()), &mut |_, _, _| panic!(
            "pending"
        )),
        Err(ReservationError::Pending)
    ));
    f = f.reopen();
    let receipt = transact(&mut f, &Change::Resume, &mut |_, command, bytes| {
        assert_eq!(command, "horizon-cloud-worker prepare-project-namespace");
        assert_eq!(bytes, sent);
        Ok(apply(&mut worker, bytes))
    })
    .unwrap();
    assert_eq!(receipt.state, State::Preparing);
    assert_eq!(
        transact(&mut f, &Change::Reserve(request.clone()), &mut |_, command, bytes| {
            assert_eq!(command, "horizon-cloud-worker reserve-project");
            Ok(apply(&mut worker, bytes))
        })
        .unwrap(),
        reserve
    );
    assert_eq!(worker.members[0].state, State::Preparing);
    assert_eq!(journal(&f.owner)["manifest"], serde_json::to_value(&worker).unwrap());
    transact(&mut f, &Change::Cancel(request.project.clone()), &mut |_, _, bytes| {
        Ok(apply(&mut worker, bytes))
    })
    .unwrap();
    assert!(
        transact(
            &mut f,
            &Change::PrepareNamespace(request.project),
            &mut |_, _, _| panic!("terminal")
        )
        .is_err()
    );
}

#[test]
fn namespace_anchor_failures_retain_exact_operation() {
    for fail_completion in [false, true] {
        let mut f = ready();
        let request = reservation(&f, "one", 8000);
        let mut worker = remote(&f);
        transact(&mut f, &Change::Reserve(request.clone()), &mut |_, _, bytes| {
            Ok(apply(&mut worker, bytes))
        })
        .unwrap();
        let vault = f.vault.clone();
        if !fail_completion {
            vault.fail_after(Some(0));
        }
        let mut sent = Vec::new();
        assert!(
            transact(
                &mut f,
                &Change::PrepareNamespace(request.project.clone()),
                &mut |_, _, bytes| {
                    assert!(fail_completion, "send before durable intent");
                    sent = bytes.to_vec();
                    let reply = apply(&mut worker, bytes);
                    vault.fail_after(Some(0));
                    Ok(reply)
                }
            )
            .is_err()
        );
        f = f.reopen();
        let before = journal(&f.owner);
        if fail_completion {
            assert!(!before["pending"].is_null());
        }
        transact(
            &mut f,
            &Change::PrepareNamespace(request.project),
            &mut |_, _, bytes| {
                if fail_completion {
                    assert_eq!(bytes, sent);
                }
                Ok(apply(&mut worker, bytes))
            },
        )
        .unwrap();
        assert_eq!(worker.operations.len(), 2);
    }
}

#[test]
#[ignore = "requires scripts/cloud-initialization-smoke.py --scenario namespaces and an isolated mount namespace"]
fn native_ssh_project_namespaces() {
    let directory = PathBuf::from(std::env::var_os("HORIZON_INITIALIZATION_FIXTURE").unwrap());
    let port = u16::try_from(serde_json::from_slice::<serde_json::Value>(&fs::read(directory.join("fixture.json")).unwrap()).unwrap()["port"].as_u64().unwrap()).unwrap();
    let mut f = native_fixture(&directory);
    let cancellation = Cancellation::default();
    let runner = runner(&cancellation);
    let target = target(&f.record, ([127, 0, 0, 1], port).into()).unwrap();
    enroll(&f.record, &target, &runner, startup_deadline()).unwrap();
    initialize(&mut f.owner, &mut f.record, &target, &runner, startup_deadline()).unwrap();
    complete(&mut f.owner, &mut f.record, &target, &cancellation, startup_deadline()).unwrap();
    let mut projects = Vec::new();
    for (index, name) in ["one", "two", "three"].iter().enumerate() {
        let request = reservation(&f, name, 8000 + u16::try_from(index).unwrap());
        f = native_prepare(f, &target, &runner, &directory, &request, index);
        let root = directory
            .join("workspace/projects")
            .join(request.project.project_id().to_string());
        fs::write(root.join("repository/dirty"), name).unwrap();
        projects.push(request);
    }
    let mut exchange = |connection: &Connection, command: &str, bytes: &[u8]| -> reservations::Result<Vec<u8>> {
        Ok(runner.private_exchange(&mut connection.pinned_command(command), bytes, Duration::from_secs(30))?)
    };
    let image = projects[0].image_digest.clone();
    reservations::coordinate(
        &mut f.owner,
        &target,
        &image,
        &Change::Cancel(projects[0].project.clone()),
        &mut exchange,
    )
    .unwrap();
    assert!(
        reservations::coordinate(
            &mut f.owner,
            &target,
            &image,
            &Change::PrepareNamespace(projects[0].project.clone()),
            &mut |_, _, _| panic!("delayed preparation")
        )
        .is_err()
    );
    reservations::coordinate(
        &mut f.owner,
        &target,
        &image,
        &Change::PrepareNamespace(projects[1].project.clone()),
        &mut exchange,
    )
    .unwrap();
    for (request, expected) in projects.iter().zip(["one", "two", "three"]) {
        assert_eq!(
            fs::read_to_string(
                directory
                    .join("workspace/projects")
                    .join(request.project.project_id().to_string())
                    .join("repository/dirty")
            )
            .unwrap(),
            expected
        );
    }
    let worker: Manifest =
        serde_json::from_slice(&fs::read(directory.join("workspace/.horizon-allocation/membership.json")).unwrap())
            .unwrap();
    worker.validate().unwrap();
    assert_eq!(worker.revision, 7);
    assert_eq!(worker.members[0].state, State::Removed);
    assert!(
        worker.members[1..]
            .iter()
            .all(|member| member.state == State::Preparing)
    );
    assert_eq!(journal(&f.owner)["manifest"], serde_json::to_value(worker).unwrap());
    assert_bootstrap_fenced(&mut f);
}

fn native_prepare(
    mut f: CoordinatorFixture,
    target: &bootstrap_recovery::Target,
    runner: &Runner<'_>,
    directory: &std::path::Path,
    request: &Reservation,
    index: usize,
) -> CoordinatorFixture {
    let image = request.image_digest.clone();
    let mut exchange = |connection: &Connection, command: &str, bytes: &[u8]| -> reservations::Result<Vec<u8>> {
        Ok(runner.private_exchange(&mut connection.pinned_command(command), bytes, Duration::from_secs(30))?)
    };
    reservations::coordinate(
        &mut f.owner,
        target,
        &image,
        &Change::Reserve(request.clone()),
        &mut exchange,
    )
    .unwrap();
    let vault = f.vault.clone();
    let mut sent = Vec::new();
    let result = reservations::coordinate(
        &mut f.owner,
        target,
        &image,
        &Change::PrepareNamespace(request.project.clone()),
        &mut |connection, command, bytes| {
            sent = bytes.to_vec();
            let response = exchange(connection, command, bytes)?;
            if index == 0 {
                return Err(ReservationError::Invalid);
            }
            if index == 2 {
                vault.fail_after(Some(0));
            }
            Ok(response)
        },
    );
    if index == 1 {
        result.unwrap();
    } else {
        assert!(result.is_err());
        fs::write(
            directory.join("restart"),
            b"restart after uncertain namespace preparation",
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
                exchange(connection, command, bytes)
            },
        )
        .unwrap();
        assert_eq!(receipt.state, State::Preparing);
    }
    f
}
