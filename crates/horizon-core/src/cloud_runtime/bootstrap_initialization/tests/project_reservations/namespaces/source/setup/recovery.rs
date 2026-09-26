use super::*;

#[test]
fn project_setup_each_lost_reply_reuses_exact_pending_request_across_restart() {
    let mut f = ready();
    let request = setup_request(&f, "one");
    begin_setup(&mut f, &request);
    let mut worker = remote(&f);
    for index in 0..9 {
        let mut first = Vec::new();
        let vault = f.vault.clone();
        let failed = advance_exchange(
            &mut f,
            &request.project,
            &Cancellation::default(),
            &mut |_, _, bytes| {
                first = bytes.to_vec();
                let reply = apply(&mut worker, bytes);
                if index % 2 == 0 {
                    Err(ReservationError::Invalid)
                } else {
                    vault.fail_after(Some(0));
                    Ok(reply)
                }
            },
        );
        assert!(failed.is_err());
        f = f.reopen();
        assert!(setup::status(&f.owner, &request.project).unwrap().pending);
        let status = advance_exchange(
            &mut f,
            &request.project,
            &Cancellation::default(),
            &mut |_, _, bytes| {
                assert_eq!(bytes, first);
                Ok(apply(&mut worker, bytes))
            },
        )
        .unwrap();
        assert!(!status.pending);
        assert_eq!(worker.operations.len(), index + 1);
    }
    assert_eq!(setup::status(&f.owner, &request.project).unwrap().next, Step::Complete);
}

#[test]
fn project_setup_foreign_pending_and_terminal_stop_do_not_progress() {
    let mut f = ready();
    let first = setup_request(&f, "one");
    let second = setup_request(&f, "two");
    begin_setup(&mut f, &first);
    begin_setup(&mut f, &second);
    let mut worker = remote(&f);
    assert!(
        advance_exchange(&mut f, &first.project, &Cancellation::default(), &mut |_, _, bytes| {
            apply(&mut worker, bytes);
            Err(ReservationError::Invalid)
        })
        .is_err()
    );
    let saved = f.owner.load().unwrap();
    assert!(
        advance_exchange(
            &mut f,
            &second.project,
            &Cancellation::default(),
            &mut |_, _, _| panic!("unrelated pending")
        )
        .is_err()
    );
    assert_eq!(saved, f.owner.load().unwrap());
    advance_setup(&mut f, &first.project, &mut worker).unwrap();
    for _ in 0..9 {
        advance_setup(&mut f, &second.project, &mut worker).unwrap();
    }
    let second_member = worker.members[1].clone();
    for _ in 0..7 {
        advance_setup(&mut f, &first.project, &mut worker).unwrap();
    }
    let status = setup::status(&f.owner, &first.project).unwrap();
    assert_eq!(status.next, Step::StartSession(status.sessions[1].id));
    transact(
        &mut f,
        &Change::StopSession(first.project.clone(), status.sessions[0].id),
        &mut |_, _, bytes| Ok(apply(&mut worker, bytes)),
    )
    .unwrap();
    let count = worker.operations.len();
    assert_eq!(
        advance_setup(&mut f, &first.project, &mut worker).unwrap().next,
        Step::Terminal
    );
    assert_eq!(worker.operations.len(), count);
    assert_eq!(worker.members[1], second_member);
}

#[test]
fn project_setup_deadline_after_export_prevents_ssh_and_interrupted_generation_blocks() {
    for interrupted in [false, true] {
        let mut f = ready();
        let request = setup_request(&f, "one");
        begin_setup(&mut f, &request);
        let mut worker = remote(&f);
        advance_setup(&mut f, &request.project, &mut worker).unwrap();
        advance_setup(&mut f, &request.project, &mut worker).unwrap();
        let cancel = Cancellation::default();
        let deadline = Instant::now() + Duration::from_millis(300);
        let result = setup::advance_with(
            &mut f.owner,
            &request.project,
            &cancel,
            deadline,
            &mut |owner, project, repository, revision| {
                if interrupted {
                    source::prepare_with(owner, project, repository, revision, &cancel, &mut |_| {
                        Err(ReservationError::Invalid)
                    })
                } else {
                    let descriptor = source::prepare(owner, project, repository, revision, &cancel)?;
                    std::thread::sleep(deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(1));
                    Ok(descriptor)
                }
            },
            &mut |_, _, _| panic!("send after export interruption/deadline"),
        );
        assert!(result.is_err());
        f = f.reopen();
        if interrupted {
            assert!(Journal::load(&f.owner).unwrap().unwrap().generation.is_some());
            let saved = f.owner.load().unwrap();
            assert!(
                advance_exchange(&mut f, &request.project, &cancel, &mut |_, _, _| panic!(
                    "uncertain export"
                ))
                .is_err()
            );
            assert_eq!(saved, f.owner.load().unwrap());
        } else {
            assert!(matches!(result, Err(setup::Error::Deadline)));
            assert_eq!(
                setup::status(&f.owner, &request.project).unwrap().next,
                Step::ImportSource
            );
            advance_setup(&mut f, &request.project, &mut worker).unwrap();
        }
    }
}

#[test]
fn project_setup_initial_anchor_failures_have_no_remote_effects_or_changed_retry_ids() {
    for writes in [0, 1] {
        let mut f = ready();
        let request = setup_request(&f, "one");
        f.vault.fail_after(Some(writes));
        assert!(
            setup::begin_with(
                &mut f.owner,
                &resolve(&f.record).unwrap(),
                &request,
                &Cancellation::default()
            )
            .is_err()
        );
        f = f.reopen();
        assert!(!reservations::started(&f.owner).unwrap());
        let recovered = setup::status(&f.owner, &request.project).ok();
        if writes == 1 {
            assert!(recovered.is_some());
        }
        let status = begin_setup(&mut f, &request);
        if let Some(recovered) = recovered {
            assert_eq!(recovered, status);
        }
        f = f.reopen();
        assert_eq!(begin_setup(&mut f, &request), status);
    }
}

#[test]
fn project_setup_interruption_and_zero_budget_cannot_send_or_erase_progress() {
    let mut f = ready();
    let request = setup_request(&f, "one");
    let initial = begin_setup(&mut f, &request);
    let saved = f.owner.load().unwrap();
    let cancellation = Cancellation::default();
    cancellation.cancel();
    assert!(
        advance_exchange(&mut f, &request.project, &cancellation, &mut |_, _, _| panic!(
            "cancelled send"
        ))
        .is_err()
    );
    assert!(matches!(
        setup::advance(
            &mut f.owner,
            &f.record.request,
            &request.project,
            &Cancellation::default(),
            Duration::ZERO
        ),
        Err(setup::Error::Deadline)
    ));
    assert_eq!(f.owner.load().unwrap(), saved);
    f = f.reopen();
    assert_eq!(setup::status(&f.owner, &request.project).unwrap(), initial);
}

#[test]
fn project_setup_cancelled_export_keeps_a_blocked_generation_and_no_replacement() {
    let mut f = ready();
    let request = setup_request(&f, "one");
    begin_setup(&mut f, &request);
    let mut worker = remote(&f);
    advance_setup(&mut f, &request.project, &mut worker).unwrap();
    advance_setup(&mut f, &request.project, &mut worker).unwrap();
    let cancellation = Cancellation::default();
    assert!(
        setup::advance_with(
            &mut f.owner,
            &request.project,
            &cancellation,
            startup_deadline(),
            &mut |owner, project, repository, revision| source::prepare_with(
                owner,
                project,
                repository,
                revision,
                &cancellation,
                &mut |_| {
                    cancellation.cancel();
                    cancellation.check().map_err(crate::cloud_runtime::Error::from)?;
                    Ok(())
                }
            ),
            &mut |_, _, _| panic!("send after cancelled generation")
        )
        .is_err()
    );
    f = f.reopen();
    let saved = f.owner.load().unwrap();
    let paths = fs::read_dir(f.owner.artifact_root().unwrap()).unwrap().count();
    for _ in 0..2 {
        assert!(advance_setup(&mut f, &request.project, &mut worker).is_err());
        assert_eq!(saved, f.owner.load().unwrap());
        assert_eq!(fs::read_dir(f.owner.artifact_root().unwrap()).unwrap().count(), paths);
    }
}

#[test]
fn project_setup_rejects_foreign_owner_and_unplanned_sessions() {
    let mut f = ready();
    let request = setup_request(&f, "one");
    begin_setup(&mut f, &request);
    let registry = f.owner.load().unwrap()["project_setups"].clone();
    let mut foreign = ready();
    let mut payload = foreign.owner.load().unwrap();
    payload["project_setups"] = registry;
    foreign.owner.save(payload).unwrap();
    assert!(setup::status(&foreign.owner, &request.project).is_err());
    let mut worker = remote(&f);
    for _ in 0..3 {
        advance_setup(&mut f, &request.project, &mut worker).unwrap();
    }
    let extra = Session::new(
        Agent::Claude,
        setup::status(&f.owner, &request.project).unwrap().revision,
    );
    transact(
        &mut f,
        &Change::ReserveSession(request.project.clone(), extra),
        &mut |_, _, bytes| Ok(apply(&mut worker, bytes)),
    )
    .unwrap();
    let saved = f.owner.load().unwrap();
    assert!(
        advance_exchange(
            &mut f,
            &request.project,
            &Cancellation::default(),
            &mut |_, _, _| panic!("adopted foreign session")
        )
        .is_err()
    );
    assert_eq!(saved, f.owner.load().unwrap());
}
