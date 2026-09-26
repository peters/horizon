use super::*;

fn runtime_observation(worker: &Manifest, bytes: &[u8]) -> Vec<u8> {
    use horizon_cloud_protocol::{
        session_runtime::{Observation, Request, Status},
        signed::{SignedIntent, Target},
    };
    let request: RecoveryRequest = serde_json::from_slice(bytes).unwrap();
    let signed = SignedIntent::parse(request.message.as_bytes()).unwrap();
    let intent = signed
        .verify(&worker.startup.controller, request.payload.as_bytes())
        .unwrap();
    let query: Request = serde_json::from_str(&request.payload).unwrap();
    let Target::Project { identity } = intent.target() else {
        panic!("project scope")
    };
    let launch = worker
        .operations
        .iter()
        .find(|e| {
            &e.receipt.identity == identity
                && serde_json::from_str::<horizon_cloud_protocol::membership::Request>(&e.payload).unwrap()
                    == (horizon_cloud_protocol::membership::Request::StartSession {
                        session_id: query.session_id,
                    })
        })
        .map(|e| e.receipt.operation);
    let stopping = worker
        .members
        .iter()
        .find(|m| &m.identity == identity)
        .unwrap()
        .stops
        .contains(&query.session_id);
    serde_json::to_vec(&Observation {
        version: 1,
        startup: worker.startup.clone(),
        worker_id: worker.worker_id.clone(),
        project: identity.clone(),
        session_id: query.session_id,
        operation: intent.operation(),
        fingerprint: intent.fingerprint().unwrap(),
        revision: worker.revision,
        launch,
        status: if stopping {
            Status::Stopping
        } else if launch.is_some() {
            Status::Launching
        } else {
            Status::NotStarted
        },
    })
    .unwrap()
}

#[test]
fn session_runtime_pending_queries_preserve_exact_launch_and_stop_journals() {
    use reservations::session_runtime::inspect_with;
    let mut f = ready();
    let mut request = reservation(&f, "one", 8000);
    request.ports.clear();
    request.capabilities.agents = [Agent::Claude].into();
    let mut worker = remote(&f);
    prepare_host(&mut f, &request, &mut worker);
    let descriptor = declared_source();
    let session = Session::new(Agent::Claude, descriptor.revision.clone());
    for change in [
        Change::ImportSource(request.project.clone(), descriptor),
        Change::ReserveSession(request.project.clone(), session.clone()),
        Change::PrepareSession(request.project.clone(), session.id),
    ] {
        transact(&mut f, &change, &mut |_, _, bytes| Ok(apply(&mut worker, bytes))).unwrap();
    }
    for change in [
        Change::StartSession(request.project.clone(), session.id),
        Change::StopSession(request.project.clone(), session.id),
    ] {
        let before_worker = worker.clone();
        let mut wire = Vec::new();
        assert!(
            transact(&mut f, &change, &mut |_, _, bytes| {
                wire = bytes.to_vec();
                apply(&mut worker, bytes);
                Err(ReservationError::Invalid)
            })
            .is_err()
        );
        f = f.reopen();
        let before = Journal::load(&f.owner).unwrap();
        let target = resolve(&f.record).unwrap();
        for observed in [&before_worker, &worker] {
            let reply = inspect_with(&f.owner, &target, &request.project, session.id, &mut |_, bytes| {
                Ok(runtime_observation(observed, bytes))
            })
            .unwrap();
            assert_eq!(reply.revision, observed.revision);
            assert!(Journal::load(&f.owner).unwrap() == before);
        }
        assert!(
            inspect_with(&f.owner, &target, &request.project, session.id, &mut |_, bytes| {
                let mut reply: serde_json::Value =
                    serde_json::from_slice(&runtime_observation(&worker, bytes)).unwrap();
                reply["revision"] = serde_json::json!(worker.revision + 1);
                Ok(serde_json::to_vec(&reply).unwrap())
            })
            .is_err()
        );
        transact(&mut f, &Change::Resume, &mut |_, _, bytes| {
            assert_eq!(bytes, wire);
            Ok(apply(&mut worker, bytes))
        })
        .unwrap();
        transact(&mut f, &change, &mut |_, _, bytes| {
            assert_eq!(bytes, wire);
            Ok(apply(&mut worker, bytes))
        })
        .unwrap();
    }
    assert!(
        transact(
            &mut f,
            &Change::StartSession(request.project.clone(), uuid::Uuid::new_v4()),
            &mut |_, _, _| panic!("unknown session")
        )
        .is_err()
    );
}

#[test]
fn preparation_journal_recovers_lost_reply_and_completion_save_with_same_session_request() {
    let mut f = ready();
    let mut reservation = reservation(&f, "one", 8000);
    reservation.capabilities.agents = [Agent::Codex, Agent::Claude].into();
    let mut worker = remote(&f);
    prepare_host(&mut f, &reservation, &mut worker);
    let descriptor = declared_source();
    transact(
        &mut f,
        &Change::ImportSource(reservation.project.clone(), descriptor.clone()),
        &mut |_, _, bytes| Ok(apply(&mut worker, bytes)),
    )
    .unwrap();
    let mut requests = Vec::new();
    for (index, agent) in [Agent::Codex, Agent::Claude].into_iter().enumerate() {
        let session = Session::new(agent, descriptor.revision.clone());
        transact(
            &mut f,
            &Change::ReserveSession(reservation.project.clone(), session.clone()),
            &mut |_, _, bytes| Ok(apply(&mut worker, bytes)),
        )
        .unwrap();
        let change = Change::PrepareSession(reservation.project.clone(), session.id);
        let vault = f.vault.clone();
        let mut sent = Vec::new();
        assert!(
            transact(&mut f, &change, &mut |_, command, bytes| {
                assert_eq!(command, "horizon-cloud-worker prepare-project-session");
                sent = bytes.to_vec();
                let reply = apply(&mut worker, bytes);
                if index == 0 {
                    Err(ReservationError::Invalid)
                } else {
                    vault.fail_after(Some(0));
                    Ok(reply)
                }
            })
            .is_err()
        );
        assert!(
            transact(
                &mut f,
                &Change::PrepareSession(reservation.project.clone(), uuid::Uuid::new_v4()),
                &mut |_, _, _| panic!("changed pending identity")
            )
            .is_err()
        );
        assert!(
            transact(
                &mut f,
                &Change::Cancel(reservation.project.clone()),
                &mut |_, _, _| panic!("pending cancellation")
            )
            .is_err()
        );
        f = f.reopen();
        let journal = Journal::load(&f.owner).unwrap().unwrap();
        assert_eq!(
            reservations::timeout_limit(&Change::Resume, Some(&journal)),
            Source::CONTROLLER_TIMEOUT
        );
        let receipt = transact(&mut f, &Change::Resume, &mut |_, _, bytes| {
            assert_eq!(bytes, sent);
            Ok(apply(&mut worker, bytes))
        })
        .unwrap();
        requests.push((change, sent, receipt));
    }
    for (change, sent, receipt) in requests {
        assert_eq!(
            transact(&mut f, &change, &mut |_, _, bytes| {
                assert_eq!(bytes, sent);
                Ok(apply(&mut worker, bytes))
            })
            .unwrap(),
            receipt
        );
    }
    assert_eq!(worker.members[0].preparations.len(), 2);
    transact(
        &mut f,
        &Change::Cancel(reservation.project.clone()),
        &mut |_, _, bytes| Ok(apply(&mut worker, bytes)),
    )
    .unwrap();
    let session_id = worker.members[0].preparations[0];
    assert!(
        transact(
            &mut f,
            &Change::PrepareSession(reservation.project, session_id),
            &mut |_, _, _| panic!("cancelled preparation")
        )
        .is_err()
    );
}

pub(super) fn native_prepared(
    mut f: CoordinatorFixture,
    target: &crate::cloud_runtime::bootstrap_recovery::Target,
    runner: &Runner<'_>,
    directory: &Path,
    request: &Reservation,
    index: usize,
) -> CoordinatorFixture {
    let sessions = Journal::load(&f.owner).unwrap().unwrap().manifest.members[index]
        .sessions
        .clone();
    for (agent_index, session) in sessions.iter().enumerate() {
        let change = Change::PrepareSession(request.project.clone(), session.id);
        let vault = f.vault.clone();
        let mut wire = Vec::new();
        let result = reservations::coordinate(
            &mut f.owner,
            target,
            &request.image_digest,
            &change,
            &mut |connection, command, bytes| {
                assert_eq!(command, "horizon-cloud-worker prepare-project-session");
                wire = bytes.to_vec();
                let reply = runner.private_exchange(
                    &mut connection.pinned_command(command),
                    bytes,
                    Source::CONTROLLER_TIMEOUT,
                )?;
                if agent_index == 0 && index == 0 {
                    return Err(ReservationError::Invalid);
                }
                if agent_index == 0 && index == 2 {
                    vault.fail_after(Some(0));
                }
                Ok(reply)
            },
        );
        let root = directory
            .join("workspace/projects")
            .join(request.project.project_id().to_string())
            .join("worktrees")
            .join(session.id.to_string());
        let checkout = root.join("checkout");
        verify_checkout(&checkout, session, request, ["one", "two", "three"][index]);
        fs::write(checkout.join("value"), "retained dirty checkout").unwrap();
        fs::write(checkout.join("untracked"), "retained untracked data").unwrap();
        fs::write(root.join("home/config"), "retained home").unwrap();
        if agent_index == 0 && index != 1 {
            assert!(result.is_err());
            fs::write(directory.join("restart"), "session preparation recovery").unwrap();
            f = f.reopen();
            reservations::coordinate(
                &mut f.owner,
                target,
                &request.image_digest,
                &Change::Resume,
                &mut |connection, command, bytes| {
                    assert_eq!(bytes, wire);
                    Ok(runner.private_exchange(
                        &mut connection.pinned_command(command),
                        bytes,
                        Source::CONTROLLER_TIMEOUT,
                    )?)
                },
            )
            .unwrap();
        } else {
            result.unwrap();
        }
        if agent_index == 0 {
            reservations::coordinate(
                &mut f.owner,
                target,
                &request.image_digest,
                &change,
                &mut |connection, command, bytes| {
                    assert_eq!(bytes, wire);
                    Ok(runner.private_exchange(
                        &mut connection.pinned_command(command),
                        bytes,
                        Source::CONTROLLER_TIMEOUT,
                    )?)
                },
            )
            .unwrap();
        }
        assert_eq!(fs::read(checkout.join("value")).unwrap(), b"retained dirty checkout");
        assert_eq!(
            fs::read(checkout.join("untracked")).unwrap(),
            b"retained untracked data"
        );
        assert_eq!(fs::read(root.join("home/config")).unwrap(), b"retained home");
    }
    f
}

pub(super) fn extend_fixture(repo: &Path) -> String {
    use std::os::unix::fs::symlink;
    let module = repo.join("module");
    let nested = module.join("nested");
    init(&nested);
    fs::write(nested.join("value"), "nested committed source").unwrap();
    git(&nested, &["add", "value"]);
    git(&nested, &["commit", "-qm", "Nested source"]);
    let pin = git(&nested, &["rev-parse", "HEAD"]);
    git(
        &module,
        &["update-index", "--add", "--cacheinfo", &format!("160000,{pin},nested")],
    );
    git(&module, &["commit", "--amend", "--no-edit", "-q"]);
    let pin = git(&module, &["rev-parse", "HEAD"]);
    git(
        repo,
        &["update-index", "--add", "--cacheinfo", &format!("160000,{pin},module")],
    );
    symlink("value", repo.join("link")).unwrap();
    git(repo, &["add", "link"]);
    git(repo, &["commit", "--amend", "--no-edit", "-q"]);
    git(repo, &["rev-parse", "HEAD"])
}

fn verify_checkout(checkout: &Path, session: &Session, request: &Reservation, name: &str) {
    assert_eq!(git(checkout, &["rev-parse", "HEAD"]), session.revision);
    assert_eq!(
        fs::read_to_string(checkout.join("value")).unwrap(),
        format!("committed {name}")
    );
    assert_eq!(
        fs::read_to_string(checkout.join("module/value")).unwrap(),
        format!("module {name}")
    );
    assert_eq!(
        fs::read_to_string(checkout.join("module/nested/value")).unwrap(),
        "nested committed source"
    );
    assert_eq!(fs::read_link(checkout.join("link")).unwrap(), Path::new("value"));
    assert_eq!(
        fs::read_to_string(checkout.join("asset")).unwrap(),
        format!("large asset {name}")
    );
    // This fixture clears global Git configuration: LFS safety must be local.
    fs::write(checkout.join("asset"), "changed LFS asset").unwrap();
    git(checkout, &["add", "asset"]);
    let pointer = git(checkout, &["show", ":asset"]);
    assert!(pointer.starts_with("version https://git-lfs.github.com/spec/v1\noid sha256:"));
    assert!(pointer.ends_with("size 17"));
    assert_eq!(
        git(checkout, &["symbolic-ref", "HEAD"]),
        format!("refs/heads/projects/{}/{}", request.project.project_id(), session.id)
    );
}

fn runtime_status(
    f: &CoordinatorFixture,
    target: &crate::cloud_runtime::bootstrap_recovery::Target,
    runner: &Runner<'_>,
    project: &ProjectIdentity,
    id: uuid::Uuid,
) -> horizon_cloud_protocol::session_runtime::Observation {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let result =
            reservations::session_runtime::inspect_with(&f.owner, target, project, id, &mut |connection, bytes| {
                Ok(runner.private_exchange(
                    &mut connection.pinned_command("horizon-cloud-worker inspect-project-session"),
                    bytes,
                    Duration::from_secs(30),
                )?)
            });
        match result {
            Ok(observation) => return observation,
            Err(error) if Instant::now() >= deadline => panic!("status unavailable: {error:?}"),
            Err(_) => std::thread::sleep(Duration::from_millis(150)),
        }
    }
}
fn runtime_wait(
    f: &CoordinatorFixture,
    target: &crate::cloud_runtime::bootstrap_recovery::Target,
    runner: &Runner<'_>,
    project: &ProjectIdentity,
    id: uuid::Uuid,
    expected: &horizon_cloud_protocol::session_runtime::Status,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = runtime_status(f, target, runner, project, id).status;
        if &status == expected {
            return;
        }
        assert!(Instant::now() < deadline, "expected {expected:?}, observed {status:?}");
        std::thread::sleep(Duration::from_millis(150));
    }
}
fn runtime_change(
    f: &mut CoordinatorFixture,
    target: &crate::cloud_runtime::bootstrap_recovery::Target,
    runner: &Runner<'_>,
    change: &Change,
) -> reservations::Result<Receipt> {
    reservations::coordinate(
        &mut f.owner,
        target,
        &f.record.spec.image_digest,
        change,
        &mut |connection, command, bytes| {
            Ok(runner.private_exchange(&mut connection.pinned_command(command), bytes, Duration::from_secs(30))?)
        },
    )
}

pub(super) fn runtime_smoke(
    mut f: CoordinatorFixture,
    target: &crate::cloud_runtime::bootstrap_recovery::Target,
    runner: &Runner<'_>,
    directory: &Path,
    projects: &[(Reservation, PathBuf)],
) -> CoordinatorFixture {
    use horizon_cloud_protocol::session_runtime::Status;
    let manifest = Journal::load(&f.owner).unwrap().unwrap().manifest;
    let mut sessions = Vec::new();
    for member in &manifest.members {
        for session in &member.sessions {
            let root = directory
                .join("workspace/projects")
                .join(member.identity.project_id().to_string())
                .join("worktrees")
                .join(session.id.to_string());
            sessions.push((member.identity.clone(), session.id, root));
        }
    }
    if std::env::var("HORIZON_RUNTIME_FAULT").as_deref() == Ok("stop-race") {
        let (project, id, root) = &sessions[0];
        runtime_change(&mut f, target, runner, &Change::StartSession(project.clone(), *id)).unwrap();
        runtime_change(&mut f, target, runner, &Change::StopSession(project.clone(), *id)).unwrap();
        runtime_wait(&f, target, runner, project, *id, &Status::Stopped);
        runtime_change(&mut f, target, runner, &Change::StartSession(project.clone(), *id)).unwrap();
        assert!(!root.join("home/launch-count").exists());
        return f;
    }
    // Image-wide managed configuration can defeat customization suppression.
    // Reject it before consuming one-shot launch authority, retaining the exact
    // pending request so removing the configuration can safely resume it.
    let managed = directory.join("managed/managed-settings.json");
    fs::write(&managed, "{}").unwrap();
    let (project, id, _) = &sessions[0];
    assert!(runtime_change(&mut f, target, runner, &Change::StartSession(project.clone(), *id)).is_err());
    assert_eq!(
        runtime_status(&f, target, runner, project, *id).status,
        Status::NotStarted
    );
    assert!(
        !directory
            .join(format!("workspace/.horizon-allocation/runtime-{id}.json"))
            .exists()
    );
    fs::remove_file(&managed).unwrap();
    f = runtime_start(f, target, runner, &sessions);
    let lengths: Vec<_> = sessions
        .iter()
        .map(|(_, _, root)| fs::metadata(root.join("checkout/runtime-progress")).unwrap().len())
        .collect();
    f = f.reopen();
    fs::write(directory.join("restart"), "SSH restart preserves processes").unwrap();
    std::thread::sleep(Duration::from_millis(250));
    for ((project, id, root), before) in sessions.iter().zip(lengths) {
        runtime_wait(&f, target, runner, project, *id, &Status::Running);
        assert!(fs::metadata(root.join("checkout/runtime-progress")).unwrap().len() > before);
        assert_eq!(fs::read_to_string(root.join("home/launch-count")).unwrap(), "launch\n");
        assert_eq!(
            fs::read(root.join("checkout/value")).unwrap(),
            b"retained dirty checkout"
        );
        assert_eq!(fs::read(root.join("home/config")).unwrap(), b"retained home");
    }
    fs::write(sessions[0].2.join("home/exit-agent"), "exit").unwrap();
    runtime_wait(
        &f,
        target,
        runner,
        &sessions[0].0,
        sessions[0].1,
        &Status::Exited { code: Some(17) },
    );
    let descendant = sessions[0].2.join("home/descendant-progress");
    let before = fs::metadata(&descendant).unwrap().len();
    std::thread::sleep(Duration::from_millis(150));
    assert!(fs::metadata(&descendant).unwrap().len() > before);
    for (project, id, root) in &sessions[..5] {
        runtime_change(&mut f, target, runner, &Change::StopSession(project.clone(), *id)).unwrap();
        runtime_wait(&f, target, runner, project, *id, &Status::Stopped);
        let path = root.join("home/descendant-progress");
        let before = fs::metadata(&path).unwrap().len();
        runtime_change(&mut f, target, runner, &Change::StartSession(project.clone(), *id)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(fs::metadata(&path).unwrap().len(), before);
        assert_eq!(fs::read_to_string(root.join("home/launch-count")).unwrap(), "launch\n");
        runtime_wait(&f, target, runner, &sessions[5].0, sessions[5].1, &Status::Running);
    }
    let (project, id, root) = &sessions[5];
    fs::write(
        root.join("home/lose-runtime"),
        std::env::var("HORIZON_RUNTIME_FAULT").unwrap_or_else(|_| "supervisor".into()),
    )
    .unwrap();
    runtime_wait(&f, target, runner, project, *id, &Status::Uncertain);
    runtime_change(&mut f, target, runner, &Change::StopSession(project.clone(), *id)).unwrap();
    runtime_change(&mut f, target, runner, &Change::StartSession(project.clone(), *id)).unwrap();
    runtime_wait(&f, target, runner, project, *id, &Status::Uncertain);
    assert_eq!(fs::read_to_string(root.join("home/launch-count")).unwrap(), "launch\n");
    if std::env::var("HORIZON_RUNTIME_FAULT").as_deref() == Ok("socket") {
        let allocation = directory.join("workspace/.horizon-allocation");
        let bytes = fs::read(allocation.join(format!("runtime-{id}.json"))).unwrap();
        let record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let socket = allocation.join(format!("r-{}/s", record["nonce"].as_str().unwrap()));
        assert_eq!(fs::read(socket).unwrap(), b"foreign socket replacement");
    }
    assert_eq!(projects.len(), 3);
    f
}

fn runtime_start(
    mut f: CoordinatorFixture,
    target: &crate::cloud_runtime::bootstrap_recovery::Target,
    runner: &Runner<'_>,
    sessions: &[(ProjectIdentity, uuid::Uuid, PathBuf)],
) -> CoordinatorFixture {
    use horizon_cloud_protocol::session_runtime::Status;
    for (index, (project, id, _)) in sessions.iter().enumerate() {
        let change = Change::StartSession(project.clone(), *id);
        let mut wire = Vec::new();
        let vault = f.vault.clone();
        let result = reservations::coordinate(
            &mut f.owner,
            target,
            &f.record.spec.image_digest,
            &change,
            &mut |connection, command, bytes| {
                wire = bytes.to_vec();
                let result =
                    runner.private_exchange(&mut connection.pinned_command(command), bytes, Duration::from_secs(30))?;
                if index == 0 {
                    return Err(ReservationError::Invalid);
                }
                if index == 2 {
                    vault.fail_after(Some(0));
                }
                Ok(result)
            },
        );
        if index == 0 || index == 2 {
            assert!(result.is_err());
            f = f.reopen();
            runtime_wait(&f, target, runner, project, *id, &Status::Running);
            reservations::coordinate(
                &mut f.owner,
                target,
                &f.record.spec.image_digest,
                &Change::Resume,
                &mut |connection, command, bytes| {
                    assert_eq!(bytes, wire);
                    Ok(runner.private_exchange(
                        &mut connection.pinned_command(command),
                        bytes,
                        Duration::from_secs(30),
                    )?)
                },
            )
            .unwrap();
        } else {
            result.unwrap();
        }
        runtime_wait(&f, target, runner, project, *id, &Status::Running);
    }
    f
}

pub(super) fn runtime_cancel_uncertain(
    f: &mut CoordinatorFixture,
    target: &crate::cloud_runtime::bootstrap_recovery::Target,
    runner: &Runner<'_>,
    request: &Reservation,
) {
    if std::env::var("HORIZON_RUNTIME_FAULT").as_deref() == Ok("stop-race") {
        return;
    }
    assert!(runtime_change(f, target, runner, &Change::Cancel(request.project.clone())).is_err());
    assert!(Journal::load(&f.owner).unwrap().unwrap().pending.is_some());
}
