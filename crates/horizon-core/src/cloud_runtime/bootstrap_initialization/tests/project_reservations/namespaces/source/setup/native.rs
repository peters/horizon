use super::*;
use crate::cloud_runtime::Error as TransportError;
use horizon_cloud_protocol::session_runtime::Status as RuntimeStatus;

struct Native {
    directory: PathBuf,
    target: bootstrap_recovery::Target,
    cancellation: Cancellation,
}
impl Native {
    fn advance(
        &self,
        f: &mut CoordinatorFixture,
        project: &ProjectIdentity,
        fault: usize,
    ) -> setup::Result<setup::Status> {
        let runner = runner(&self.cancellation);
        let image = f.record.spec.image_digest.clone();
        let vault = f.vault.clone();
        setup::advance_with(
            &mut f.owner,
            project,
            &self.cancellation,
            Instant::now() + Source::CONTROLLER_TIMEOUT,
            &mut |owner, project, repository, revision| {
                source::prepare(owner, project, repository, revision, &self.cancellation)
            },
            &mut |owner, change, budget| {
                let saved = Journal::load(owner)?;
                let root = owner.artifact_root()?.to_owned();
                reservations::coordinate(
                    owner,
                    &self.target,
                    &image,
                    change,
                    &mut |connection, command, bytes| {
                        let mut line = connection.pinned_command(command);
                        let reply = if command == "horizon-cloud-worker import-project-source" {
                            let input = source::for_request(saved.as_ref().ok_or(ReservationError::Missing)?, bytes)?
                                .frame(&root, bytes, &self.cancellation, startup_deadline())?;
                            runner.private_file_exchange(&mut line, input, budget)?
                        } else {
                            runner.private_exchange(&mut line, bytes, budget)?
                        };
                        if fault == 1 {
                            return Err(ReservationError::Invalid);
                        }
                        if fault == 2 {
                            vault.fail_after(Some(0));
                        }
                        Ok(reply)
                    },
                )?;
                Ok(())
            },
        )
    }
    fn restart(&self) {
        let request = self.directory.join("restart");
        fs::write(&request, "restart after interrupted project setup").unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        while request.try_exists().unwrap() {
            assert!(
                Instant::now() < deadline,
                "fixture did not acknowledge restart within 60s"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    fn change(&self, f: &mut CoordinatorFixture, change: &Change) {
        let runner = runner(&self.cancellation);
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            // Supervisors briefly hold the same allocation lock. Repeating the
            // original change validates and reuses its anchored signed request.
            let result = reservations::coordinate(
                &mut f.owner,
                &self.target,
                &f.record.spec.image_digest,
                change,
                &mut |connection, command, bytes| {
                    let budget = deadline.saturating_duration_since(Instant::now());
                    if budget.is_zero() {
                        return Err(ReservationError::Deadline);
                    }
                    Ok(runner.private_exchange(&mut connection.pinned_command(command), bytes, budget)?)
                },
            );
            match result {
                Err(ReservationError::Transport(TransportError::PrivateTransport)) if Instant::now() < deadline => {
                    eprintln!("Retrying the retained terminal request after a transport refusal");
                    std::thread::sleep(Duration::from_millis(100));
                }
                result => {
                    result.unwrap();
                    return;
                }
            }
        }
    }

    fn wait(&self, f: &CoordinatorFixture, project: &ProjectIdentity, id: uuid::Uuid, expected: &RuntimeStatus) {
        let runner = runner(&self.cancellation);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let observed = reservations::session_runtime::inspect_with(
                &f.owner,
                &self.target,
                project,
                id,
                &mut |connection, bytes| {
                    Ok(runner.private_exchange(
                        &mut connection.pinned_command("horizon-cloud-worker inspect-project-session"),
                        bytes,
                        Duration::from_secs(30),
                    )?)
                },
            )
            .unwrap()
            .status;
            if &observed == expected {
                return;
            }
            assert!(Instant::now() < deadline, "expected {expected:?}, got {observed:?}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    fn session_root(&self, project: &ProjectIdentity, id: uuid::Uuid) -> PathBuf {
        self.directory
            .join("workspace/projects")
            .join(project.project_id().to_string())
            .join("worktrees")
            .join(id.to_string())
    }
}

#[test]
#[ignore = "requires scripts/cloud-initialization-smoke.py --scenario setup and an isolated mount namespace"]
fn native_ssh_project_setup() {
    let directory = PathBuf::from(std::env::var_os("HORIZON_INITIALIZATION_FIXTURE").unwrap());
    let value: serde_json::Value = serde_json::from_slice(&fs::read(directory.join("fixture.json")).unwrap()).unwrap();
    let port = u16::try_from(value["port"].as_u64().unwrap()).unwrap();
    let mut f = native_fixture(&directory);
    let cancellation = Cancellation::default();
    let runner = runner(&cancellation);
    let target = target(&f.record, ([127, 0, 0, 1], port).into()).unwrap();
    enroll(&f.record, &target, &runner, startup_deadline()).unwrap();
    initialize(&mut f.owner, &mut f.record, &target, &runner, startup_deadline()).unwrap();
    complete(&mut f.owner, &mut f.record, &target, &cancellation, startup_deadline()).unwrap();
    let native = Native {
        directory,
        target,
        cancellation,
    };
    let mut projects = Vec::new();
    for name in ["one", "two", "three"] {
        let request = setup_request(&f, name);
        // Include verified LFS and recursively pinned submodules in real setup.
        fs::remove_dir_all(&request.repository).unwrap();
        repository(&request.repository, name);
        sessions::extend_fixture(&request.repository);
        let initial = setup::begin_with(&mut f.owner, &native.target, &request, &native.cancellation).unwrap();
        fs::write(request.repository.join("value"), "later branch content").unwrap();
        git(&request.repository, &["add", "value"]);
        git(&request.repository, &["commit", "-qm", "Move source selection"]);
        fs::write(request.repository.join("value"), "uncommitted sentinel").unwrap();
        assert_eq!(
            setup::begin_with(&mut f.owner, &native.target, &request, &native.cancellation).unwrap(),
            initial
        );
        projects.push((request, initial));
    }
    // Interleave projects under the same canonical owner, reopening it after
    // every attempt. This exercises serialized callers without replacing IDs.
    for step in 0..9 {
        for (index, (request, initial)) in projects.iter().enumerate() {
            if index != 1 {
                assert!(native.advance(&mut f, &request.project, index / 2 + 1).is_err());
                native.restart();
                f = f.reopen();
                assert!(setup::status(&f.owner, &request.project).unwrap().pending);
                let sibling = &projects[(index + 1) % 3].0.project;
                assert!(native.advance(&mut f, sibling, 0).is_err());
            }
            let status = native.advance(&mut f, &request.project, 0).unwrap();
            assert_eq!(status.sessions, initial.sessions);
            assert_eq!(status.revision, initial.revision);
            if step == 8 {
                assert_eq!(status.next, Step::Complete);
            }
            f = f.reopen();
        }
    }
    verify_sessions(&native, &mut f, &projects);
    // A normally exited session stays exited; settled setup must not relaunch it.
    let (request, initial) = &projects[0];
    let first = &initial.sessions[0];
    fs::write(
        native.session_root(&request.project, first.id).join("home/exit-agent"),
        "exit",
    )
    .unwrap();
    native.wait(
        &f,
        &request.project,
        first.id,
        &RuntimeStatus::Exited { code: Some(17) },
    );
    assert_eq!(
        native.advance(&mut f, &request.project, 0).unwrap().next,
        Step::Complete
    );
    stop_and_preserve(&native, &mut f, &projects);
}

fn verify_sessions(native: &Native, f: &mut CoordinatorFixture, projects: &[(SetupRequest, setup::Status)]) {
    for (request, initial) in projects {
        for session in &initial.sessions {
            native.wait(f, &request.project, session.id, &RuntimeStatus::Running);
            let root = native.session_root(&request.project, session.id);
            let deadline = Instant::now() + Duration::from_secs(5);
            while !root.join("home/launch-count").exists() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(50));
            }
            assert_eq!(fs::read_to_string(root.join("home/launch-count")).unwrap(), "launch\n");
            assert_eq!(git(&root.join("checkout"), &["rev-parse", "HEAD"]), initial.revision);
            assert_eq!(
                fs::read_to_string(root.join("checkout/value")).unwrap(),
                format!("committed {}", request.project.cloud_id())
            );
            assert_eq!(
                fs::read_to_string(root.join("checkout/asset")).unwrap(),
                format!("large asset {}", request.project.cloud_id())
            );
            assert_eq!(
                fs::read_to_string(root.join("checkout/module/value")).unwrap(),
                format!("module {}", request.project.cloud_id())
            );
            assert_eq!(
                fs::read(root.join("checkout/module/nested/value")).unwrap(),
                b"nested committed source"
            );
            fs::write(root.join("checkout/value"), "retained dirty checkout").unwrap();
            fs::write(root.join("home/retained"), "private home").unwrap();
        }
        assert_eq!(native.advance(f, &request.project, 0).unwrap().next, Step::Complete);
    }
}

fn stop_and_preserve(native: &Native, f: &mut CoordinatorFixture, projects: &[(SetupRequest, setup::Status)]) {
    let sibling = native
        .session_root(&projects[2].0.project, projects[2].1.sessions[0].id)
        .join("checkout/runtime-progress");
    let before = fs::metadata(&sibling).unwrap().len();
    for (request, initial) in projects {
        for session in &initial.sessions {
            native.change(f, &Change::StopSession(request.project.clone(), session.id));
            native.wait(f, &request.project, session.id, &RuntimeStatus::Stopped);
            assert_eq!(native.advance(f, &request.project, 0).unwrap().next, Step::Terminal);
            let root = native.session_root(&request.project, session.id);
            assert_eq!(fs::read_to_string(root.join("home/launch-count")).unwrap(), "launch\n");
            assert_eq!(
                fs::read(root.join("checkout/value")).unwrap(),
                b"retained dirty checkout"
            );
            assert_eq!(fs::read(root.join("home/retained")).unwrap(), b"private home");
        }
        native.change(f, &Change::Cancel(request.project.clone()));
        assert_eq!(native.advance(f, &request.project, 0).unwrap().next, Step::Terminal);
        if request.project == projects[0].0.project {
            assert!(fs::metadata(&sibling).unwrap().len() > before);
        }
    }
    let worker: Manifest = serde_json::from_slice(
        &fs::read(native.directory.join("workspace/.horizon-allocation/membership.json")).unwrap(),
    )
    .unwrap();
    worker.validate().unwrap();
    assert_eq!(worker.members.len(), 3);
    assert!(worker.members.iter().all(|m| m.state == State::Removed));
    assert_eq!(worker, Journal::load(&f.owner).unwrap().unwrap().manifest);
}
