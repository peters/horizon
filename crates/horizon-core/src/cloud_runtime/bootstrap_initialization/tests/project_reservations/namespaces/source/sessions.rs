use super::*;

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
    assert_eq!(
        git(checkout, &["symbolic-ref", "HEAD"]),
        format!("refs/heads/projects/{}/{}", request.project.project_id(), session.id)
    );
}
