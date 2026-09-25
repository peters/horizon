use super::*;
use horizon_cloud_protocol::{
    ProjectId, ProjectIdentity,
    bootstrap::RecoveryRequest,
    membership::{Manifest, Receipt, Request as MembershipRequest, State},
    signed::{Intent, Target as IntentTarget},
};

fn signed(owner: &Owner, identity: &ProjectIdentity, revision: u64, request: &MembershipRequest) -> Vec<u8> {
    let payload = serde_json::to_string(request).unwrap();
    let intent = Intent::new(
        &owner.binding().unwrap(),
        OperationId::generate(),
        revision,
        IntentTarget::Project {
            identity: identity.clone(),
        },
        request.action(),
        payload.as_bytes(),
    )
    .unwrap();
    serde_json::to_vec(&RecoveryRequest {
        message: serde_json::to_string(&owner.sign(intent).unwrap()).unwrap(),
        payload,
    })
    .unwrap()
}
fn project(name: &str) -> ProjectIdentity {
    ProjectIdentity::new(ProjectId::generate(), "session".into(), "workspace".into(), name.into()).unwrap()
}
fn reservation(port: u16) -> MembershipRequest {
    MembershipRequest::Reserve {
        capabilities: serde_json::from_str("{}").unwrap(),
        ports: [port].into(),
    }
}
fn exchange(
    runner: &Runner<'_>,
    target: &bootstrap_recovery::Target,
    command: &str,
    request: &[u8],
) -> crate::cloud_runtime::Result<Vec<u8>> {
    runner.private_exchange(
        &mut target.connection.pinned_command(command),
        request,
        Duration::from_secs(30),
    )
}
fn confirm(owner: &Owner, bytes: &[u8], request: &[u8], identity: &ProjectIdentity, revision: u64, state: State) {
    let receipt: Receipt = serde_json::from_slice(bytes).unwrap();
    let request: RecoveryRequest = serde_json::from_slice(request).unwrap();
    let message = horizon_cloud_protocol::signed::SignedIntent::parse(request.message.as_bytes()).unwrap();
    let intent = message
        .verify(&owner.binding().unwrap(), request.payload.as_bytes())
        .unwrap();
    assert_eq!(receipt.version, 1);
    assert_eq!(receipt.operation, intent.operation());
    assert_eq!(receipt.fingerprint, intent.fingerprint().unwrap());
    assert_eq!(receipt.identity, *identity);
    assert_eq!(receipt.revision, revision);
    assert_eq!(receipt.state, state);
}

#[test]
#[ignore = "requires scripts/cloud-initialization-smoke.py --scenario reservations and an isolated mount namespace"]
fn native_ssh_project_reservations() {
    let directory = PathBuf::from(std::env::var_os("HORIZON_INITIALIZATION_FIXTURE").unwrap());
    let port = u16::try_from(serde_json::from_slice::<serde_json::Value>(&fs::read(directory.join("fixture.json")).unwrap()).unwrap()["port"].as_u64().unwrap()).unwrap();
    let CoordinatorFixture {
        directory: _temporary,
        root: _root,
        vault: _vault,
        mut owner,
        mut record,
    } = native_fixture(&directory);
    let cancellation = Cancellation::default();
    let runner = runner(&cancellation);
    let target = target(&record, ([127, 0, 0, 1], port).into()).unwrap();
    enroll(&record, &target, &runner, startup_deadline()).unwrap();
    initialize(&mut owner, &mut record, &target, &runner, startup_deadline()).unwrap();
    complete(&mut owner, &mut record, &target, &cancellation, startup_deadline()).unwrap();
    let root = directory.join("workspace/.horizon-allocation");
    let bootstrap = fs::read(root.join("bootstrap.json")).unwrap();
    let key = fs::read(root.join("ssh-host-key")).unwrap();
    let pin = fs::read(&target.connection.known_hosts).unwrap();
    let projects = [project("one"), project("two"), project("three")];
    let mut requests = Vec::new();
    let mut responses = Vec::new();
    for (index, project) in projects.iter().enumerate() {
        let request = signed(
            &owner,
            project,
            index as u64,
            &reservation(8000 + u16::try_from(index).unwrap()),
        );
        let response = exchange(&runner, &target, "horizon-cloud-worker reserve-project", &request).unwrap();
        confirm(&owner, &response, &request, project, index as u64 + 1, State::Attaching);
        requests.push(request);
        responses.push(response);
    }
    let before: Manifest = serde_json::from_slice(&fs::read(root.join("membership.json")).unwrap()).unwrap();
    before.validate().unwrap();
    assert_eq!(
        before
            .members
            .iter()
            .map(|member| &member.namespace)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3
    );
    // Drop a response, restart only the fixture runtime, and recover the same receipt.
    fs::write(directory.join("restart"), b"restart populated worker").unwrap();
    assert_eq!(
        exchange(&runner, &target, "horizon-cloud-worker reserve-project", &requests[0]).unwrap(),
        responses[0]
    );
    assert_eq!(fs::read(&target.connection.known_hosts).unwrap(), pin);
    assert_eq!(fs::read(root.join("ssh-host-key")).unwrap(), key);
    reject_conflicts(&owner, &target, &runner);
    let cancel = signed(&owner, &projects[0], 3, &MembershipRequest::Cancel {});
    let response = exchange(
        &runner,
        &target,
        "horizon-cloud-worker cancel-project-reservation",
        &cancel,
    )
    .unwrap();
    confirm(&owner, &response, &cancel, &projects[0], 4, State::Removed);
    fs::write(directory.join("restart"), b"restart cancelled worker").unwrap();
    assert_eq!(
        exchange(
            &runner,
            &target,
            "horizon-cloud-worker cancel-project-reservation",
            &cancel
        )
        .unwrap(),
        response
    );
    assert!(exchange(&runner, &target, "horizon-cloud-worker reserve-project", &requests[0]).is_err());
    let after: Manifest = serde_json::from_slice(&fs::read(root.join("membership.json")).unwrap()).unwrap();
    assert_eq!(&after.members[1..], &before.members[1..]);
    assert_eq!(after.members[0].state, State::Removed);
    assert_eq!(after.revision, 4);
    assert_eq!(fs::read(root.join("bootstrap.json")).unwrap(), bootstrap);
    let fourth = signed(&owner, &project("four"), 4, &reservation(8000));
    exchange(&runner, &target, "horizon-cloud-worker reserve-project", &fourth).unwrap();
    reject_bootstrap(&mut owner, &target, &runner, &record, &cancellation);
    // Only the allocation journal exists; no project source, tools, homes or sessions.
    assert_eq!(fs::read_dir(directory.join("workspace")).unwrap().count(), 1);
    let final_manifest: Manifest = serde_json::from_slice(&fs::read(root.join("membership.json")).unwrap()).unwrap();
    final_manifest.validate().unwrap();
    assert_eq!(final_manifest.revision, 5);
}

fn reject_conflicts(owner: &Owner, target: &bootstrap_recovery::Target, runner: &Runner<'_>) {
    for request in [
        signed(owner, &project("four"), 3, &reservation(8001)),
        signed(owner, &project("four"), 2, &reservation(8004)),
        signed(owner, &project("one"), 3, &reservation(8004)),
    ] {
        assert!(exchange(runner, target, "horizon-cloud-worker reserve-project", &request).is_err());
    }
    let mut unavailable = reservation(8004);
    if let MembershipRequest::Reserve { capabilities, .. } = &mut unavailable {
        capabilities.desktop = true;
    }
    assert!(
        exchange(
            runner,
            target,
            "horizon-cloud-worker reserve-project",
            &signed(owner, &project("four"), 3, &unavailable)
        )
        .is_err()
    );
}

fn reject_bootstrap(
    owner: &mut Owner,
    target: &bootstrap_recovery::Target,
    runner: &Runner<'_>,
    record: &Record,
    cancellation: &Cancellation,
) {
    assert!(bootstrap_recovery::recover(owner, target, cancellation, Duration::from_secs(30)).is_err());
    let abandon = BootstrapPayload::Abandon {
        startup: target.startup.clone(),
        worker_id: target.worker_id.clone(),
    };
    let signed = Signed::new(
        owner,
        &target.startup,
        &target.worker_id,
        &abandon,
        BootstrapOutcome::Abandoned,
    )
    .unwrap();
    assert!(
        exchange(
            runner,
            target,
            "horizon-cloud-worker abandon-bootstrap",
            signed.request(target, &abandon, BootstrapOutcome::Abandoned).unwrap()
        )
        .is_err()
    );
    let initial = serde_json::to_value(record.initialize.as_ref().unwrap()).unwrap();
    assert!(
        exchange(
            runner,
            target,
            "horizon-cloud-worker initialize-allocation",
            initial["request"].as_str().unwrap().as_bytes()
        )
        .is_err()
    );
}
