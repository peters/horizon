use super::*;
use reservations::{SessionTransport, prepare_attachment};

#[test]
fn attachment_revalidates_the_owner_journal_and_retains_pinned_ssh_material() {
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
        Change::StartSession(request.project.clone(), session.id),
    ] {
        assert!(prepare_attachment(&f.owner, &request.project, session.id).is_err());
        transact(&mut f, &change, &mut |_, _, bytes| Ok(apply(&mut worker, bytes))).unwrap();
    }
    let selection = prepare_attachment(&f.owner, &request.project, session.id).unwrap();
    let target = resolve(&f.record).unwrap();
    let transport = selection.transport_with(&f.owner, &target).unwrap();
    let args = transport.arguments();
    assert_eq!(args[0], "-tt");
    assert!(args.iter().any(|s| s == "StrictHostKeyChecking=yes"));
    assert!(
        !args
            .iter()
            .any(|s| s.contains("accept-new") || s.contains("horizon-worker-session"))
    );
    let key = PathBuf::from(&args[args.iter().position(|s| s == "-i").unwrap() + 1]);
    assert!(key.exists());
    let encoded = args
        .last()
        .unwrap()
        .strip_prefix("horizon-cloud-worker attach-project-session ")
        .unwrap();
    let authorization = horizon_cloud_protocol::session_attachment::decode(encoded).unwrap();
    let signed = horizon_cloud_protocol::signed::SignedIntent::parse(authorization.message.as_bytes()).unwrap();
    let intent = signed
        .verify(&worker.startup.controller, authorization.payload.as_bytes())
        .unwrap();
    assert_eq!(
        intent.action(),
        horizon_cloud_protocol::signed::Action::AttachProjectSession
    );
    assert_eq!(intent.expected_revision(), worker.revision);
    let change = Change::StopSession(request.project.clone(), session.id);
    assert!(transact(&mut f, &change, &mut |_, _, _| Err(ReservationError::Invalid)).is_err());
    let before = Journal::load(&f.owner).unwrap();
    assert!(prepare_attachment(&f.owner, &request.project, session.id).is_err());
    assert!(selection.transport_with(&f.owner, &target).is_err());
    assert!(Journal::load(&f.owner).unwrap() == before);
    transact(&mut f, &Change::Resume, &mut |_, _, bytes| {
        Ok(apply(&mut worker, bytes))
    })
    .unwrap();
    assert!(prepare_attachment(&f.owner, &request.project, session.id).is_err());
    assert!(selection.transport_with(&f.owner, &target).is_err());
    drop(transport);
    assert!(!key.exists());
}

pub(super) struct Fixture {
    child: std::process::Child,
    _transports: Vec<SessionTransport>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_none() {
            self.child.kill().unwrap();
        }
        self.child.wait().unwrap();
    }
}
pub(super) fn begin(
    f: &CoordinatorFixture,
    target: &crate::cloud_runtime::bootstrap_recovery::Target,
    directory: &Path,
    sessions: &[(ProjectIdentity, uuid::Uuid, PathBuf)],
) -> Option<Fixture> {
    std::env::var_os("HORIZON_ATTACHMENT_SMOKE")?;
    let transports: Vec<_> = sessions
        .iter()
        .map(|(project, id, _)| {
            prepare_attachment(&f.owner, project, *id)
                .unwrap()
                .transport_with(&f.owner, target)
                .unwrap()
        })
        .collect();
    let arguments: Vec<_> = transports.iter().map(SessionTransport::arguments).collect();
    let path = directory.join("attachment-clients.json");
    fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({"directory":directory,"arguments":arguments,"race":std::env::var_os("HORIZON_ATTACHMENT_RACE").is_some()})).unwrap(),
    )
    .unwrap();
    let log = fs::File::create(directory.join("attachment-clients.log")).unwrap();
    let child = std::process::Command::new("python3")
        .arg(directory.join("control/cloud_attachment_smoke.py"))
        .arg(&path)
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let mut fixture = Fixture {
        child,
        _transports: transports,
    };
    fixture.wait(directory, "attachment-ready");
    let deadline = Instant::now() + Duration::from_secs(5);
    while fs::read_to_string(sessions[0].2.join("home/terminal-size")).unwrap() != "101 31" {
        assert!(Instant::now() < deadline, "terminal resize did not reach agent");
        std::thread::sleep(Duration::from_millis(50));
    }
    for (index, (_, _, root)) in sessions.iter().enumerate() {
        let input = fs::read_to_string(root.join("home/terminal-input")).unwrap();
        assert!(input.contains(&format!("session-{index}-first")));
        for sibling in 0..sessions.len() {
            if sibling != index {
                assert!(!input.contains(&format!("session-{sibling}-first")));
            }
        }
    }
    Some(fixture)
}
impl Fixture {
    fn wait(&mut self, directory: &Path, marker: &str) {
        let deadline = Instant::now() + Duration::from_secs(50);
        while !directory.join(marker).exists() {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "attachment client failed; inspect its private log"
            );
            assert!(Instant::now() < deadline, "attachment fixture deadline expired");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    pub(super) fn exited(&mut self, directory: &Path) {
        fs::write(directory.join("attachment-exited"), "exited").unwrap();
        self.wait(
            directory,
            if std::env::var_os("HORIZON_ATTACHMENT_RACE").is_some() {
                "control/attachment-paused"
            } else {
                "attachment-dead"
            },
        );
    }
    pub(super) fn finish(mut self, directory: &Path) {
        fs::write(directory.join("attachment-stopped"), "stopped").unwrap();
        self.wait(directory, "attachment-result.json");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(50));
        }
        fs::remove_file(directory.join("attachment-clients.json")).unwrap();
    }
}
