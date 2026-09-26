mod native;
mod recovery;
use super::*;
use crate::cloud_runtime::project_setup::{self as setup, Request as SetupRequest, Step};
use std::collections::BTreeSet;

fn setup_request(f: &CoordinatorFixture, name: &str) -> SetupRequest {
    let repository = f.directory.path().join(name);
    repository_fixture(&repository, name);
    SetupRequest {
        project: reservation(f, name, 8000).project,
        repository,
        selection: "HEAD".into(),
        image_digest: f.record.spec.image_digest.clone(),
        capabilities: Capabilities {
            agents: [Agent::Claude].into(),
            browsers: BTreeSet::new(),
            desktop: false,
            browserstack: None,
        },
        agents: vec![Agent::Claude; 2],
    }
}
fn repository_fixture(path: &Path, name: &str) {
    init(path);
    fs::write(path.join("value"), format!("committed {name}")).unwrap();
    git(path, &["add", "value"]);
    git(path, &["commit", "-qm", "Committed source"]);
}
fn begin_setup(f: &mut CoordinatorFixture, request: &SetupRequest) -> setup::Status {
    setup::begin_with(
        &mut f.owner,
        &resolve(&f.record).unwrap(),
        request,
        &Cancellation::default(),
    )
    .unwrap()
}
fn advance_setup(
    f: &mut CoordinatorFixture,
    project: &ProjectIdentity,
    worker: &mut Manifest,
) -> setup::Result<setup::Status> {
    let cancellation = Cancellation::default();
    advance_exchange(f, project, &cancellation, &mut |_, command, bytes| {
        if command == "horizon-cloud-worker import-project-source" {
            let request: RecoveryRequest = serde_json::from_slice(bytes).unwrap();
            return Ok(serde_json::to_vec(&worker.next(&request.message, &request.payload).unwrap().1).unwrap());
        }
        Ok(apply(worker, bytes))
    })
}
fn advance_exchange(
    f: &mut CoordinatorFixture,
    project: &ProjectIdentity,
    cancellation: &Cancellation,
    exchange: &mut impl FnMut(&Connection, &str, &[u8]) -> reservations::Result<Vec<u8>>,
) -> setup::Result<setup::Status> {
    let target = resolve(&f.record).unwrap();
    let image = f.record.spec.image_digest.clone();
    setup::advance_with(
        &mut f.owner,
        project,
        cancellation,
        Instant::now() + Duration::from_secs(60),
        &mut |owner, project, repository, revision| source::prepare(owner, project, repository, revision, cancellation),
        &mut |owner, change, _| {
            reservations::coordinate(owner, &target, &image, change, exchange)?;
            Ok(())
        },
    )
}

#[test]
fn project_setup_pins_source_and_all_session_ids_before_effects() {
    let mut f = ready();
    let request = setup_request(&f, "one");
    let first = begin_setup(&mut f, &request);
    assert_eq!(first.next, Step::Reserve);
    assert!(!reservations::started(&f.owner).unwrap());
    fs::write(request.repository.join("value"), "later committed source").unwrap();
    git(&request.repository, &["add", "value"]);
    git(&request.repository, &["commit", "-qm", "Moved selection"]);
    fs::write(request.repository.join("value"), "dirty sentinel").unwrap();
    f = f.reopen();
    assert_eq!(begin_setup(&mut f, &request), first);
    let mut worker = remote(&f);
    let mut status = first.clone();
    for _ in 0..9 {
        status = advance_setup(&mut f, &request.project, &mut worker).unwrap();
        f = f.reopen();
        assert_eq!(setup::status(&f.owner, &request.project).unwrap(), status);
    }
    assert_eq!(status.next, Step::Complete);
    assert_eq!(status.sessions, first.sessions);
    assert_eq!(status.revision, first.revision);
    assert_eq!(worker.operations.len(), 9);
    let operations = worker.operations.clone();
    assert_eq!(advance_setup(&mut f, &request.project, &mut worker).unwrap(), status);
    assert_eq!(worker.operations, operations);
    assert_eq!(fs::read(request.repository.join("value")).unwrap(), b"dirty sentinel");
}

#[test]
fn project_setup_rejects_changed_requests_and_existing_membership() {
    let mut f = ready();
    let request = setup_request(&f, "one");
    let first = begin_setup(&mut f, &request);
    let payload = f.owner.load().unwrap();
    for variant in 0..7 {
        let mut changed = request.clone();
        match variant {
            0 => changed.selection = first.revision.clone(),
            1 => changed.agents.pop().map(|_| ()).unwrap(),
            2 => changed.capabilities.desktop = true,
            3 => changed.image_digest = format!("test/worker@sha256:{}", "b".repeat(64)),
            4 => changed.repository = f.directory.path().into(),
            5 => changed.agents[0] = Agent::Codex,
            _ => changed.image_digest = "test/worker:latest".into(),
        }
        assert!(
            setup::begin_with(
                &mut f.owner,
                &resolve(&f.record).unwrap(),
                &changed,
                &Cancellation::default()
            )
            .is_err()
        );
        assert_eq!(f.owner.load().unwrap(), payload);
    }
    let mut worker = remote(&f);
    advance_setup(&mut f, &request.project, &mut worker).unwrap();
    let mut payload = f.owner.load().unwrap();
    payload.as_object_mut().unwrap().remove("project_setups");
    f.owner.save(payload.clone()).unwrap();
    assert!(
        setup::begin_with(
            &mut f.owner,
            &resolve(&f.record).unwrap(),
            &request,
            &Cancellation::default()
        )
        .is_err()
    );
    assert_eq!(f.owner.load().unwrap(), payload);
}

#[test]
fn project_setup_invalid_intents_never_advance() {
    let mut f = ready();
    let request = setup_request(&f, "one");
    begin_setup(&mut f, &request);
    let saved = f.owner.load().unwrap();
    for variant in 0..8 {
        let mut payload = saved.clone();
        let registry = &mut payload["project_setups"];
        match variant {
            0 => registry["version"] = 2.into(),
            1 => registry["intents"][0]["revision"] = "HEAD".into(),
            2 => registry["intents"][0]["sessions"][0]["id"] = uuid::Uuid::nil().to_string().into(),
            3 => registry["intents"][0]["sessions"][1] = registry["intents"][0]["sessions"][0].clone(),
            4 => registry["intents"][0]["request"]["repository"] = "relative".into(),
            5 => registry["intents"][0]["unknown"] = true.into(),
            6 => {
                let duplicate = registry["intents"][0].clone();
                registry["intents"].as_array_mut().unwrap().push(duplicate);
            }
            _ => *registry = serde_json::Value::Null,
        }
        f.owner.save(payload).unwrap();
        assert!(setup::status(&f.owner, &request.project).is_err());
        assert!(
            advance_exchange(
                &mut f,
                &request.project,
                &Cancellation::default(),
                &mut |_, _, _| panic!("invalid intent send")
            )
            .is_err()
        );
        f.owner.save(saved.clone()).unwrap();
    }
}
