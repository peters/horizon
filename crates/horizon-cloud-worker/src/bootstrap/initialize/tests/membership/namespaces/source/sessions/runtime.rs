use super::*;
use crate::bootstrap::session_runtime;
use horizon_cloud_protocol::session_runtime::{Pending, Request as Query, Status};
use std::collections::BTreeSet;

fn prepared_runtime() -> (Fixture, ProjectIdentity, SessionId) {
    let f = Fixture::ready();
    let project = identity("runtime-one");
    let reserve = Request::Reserve {
        capabilities: serde_json::from_str(r#"{"agents":["claude"]}"#).unwrap(),
        ports: BTreeSet::default(),
    };
    f.change(&f.membership(&project, 0, OperationId::generate(), &reserve))
        .unwrap();
    f.change(&f.membership(
        &project,
        f.manifest().revision,
        OperationId::generate(),
        &Request::PrepareNamespace {},
    ))
    .unwrap();
    let (descriptor, bytes) = bytes();
    import(&f, &request(&f, &project, descriptor.clone()), &bytes, &mut |_| Ok(())).unwrap();
    let session = session(&descriptor.revision, Agent::Claude);
    f.change(&session_request(&f, &project, session.clone())).unwrap();
    f.change(&prepare_request(&f, &project, session.id)).unwrap();
    (f, project, session.id)
}
fn query(
    f: &Fixture,
    project: &ProjectIdentity,
    id: SessionId,
    revision: u64,
    pending: Option<Pending>,
) -> RecoveryRequest {
    let payload = serde_json::to_string(&Query {
        session_id: id,
        pending,
    })
    .unwrap();
    let binding = &f.runtime.startup.controller;
    let intent = Intent::new(
        binding,
        OperationId::generate(),
        revision,
        Target::Project {
            identity: project.clone(),
        },
        horizon_cloud_protocol::signed::Action::InspectProjectSession,
        payload.as_bytes(),
    )
    .unwrap();
    RecoveryRequest {
        message: serde_json::to_string(&SignedIntent::sign(intent, binding, &f.controller).unwrap()).unwrap(),
        payload,
    }
}
fn inspect(f: &Fixture, request: &RecoveryRequest) -> io::Result<horizon_cloud_protocol::session_runtime::Observation> {
    session_runtime::inspect_with(&Store::open(&f.root())?, &f.runtime, request)
}

#[test]
fn historical_launch_missing_its_runtime_record_never_mints_replacement_authority() {
    let (f, project, id) = prepared_runtime();
    let before = f.manifest().revision;
    assert_eq!(
        inspect(&f, &query(&f, &project, id, before, None)).unwrap().status,
        Status::NotStarted
    );
    let start = f.membership(
        &project,
        before,
        OperationId::generate(),
        &Request::StartSession { session_id: id },
    );
    // Models both the intent-to-runtime-record crash gap and a missing runtime
    // record after a historical launch. Neither permits another process launch.
    let receipt = anchored_intent(&f, &start);
    assert_eq!(f.change(&start).unwrap(), receipt);
    assert!(!f.root().join(format!("runtime-{id}.json")).exists());
    let pending = Pending {
        operation: receipt.operation,
        revision: receipt.revision,
        fingerprint: receipt.fingerprint,
    };
    let observation = inspect(&f, &query(&f, &project, id, before, Some(pending.clone()))).unwrap();
    assert_eq!(observation.status, Status::Uncertain);
    assert_eq!(observation.launch, Some(receipt.operation));
    assert!(inspect(&f, &query(&f, &project, id, before, None)).is_err());
    let mut wrong = pending;
    wrong.fingerprint[0] ^= 1;
    assert!(inspect(&f, &query(&f, &project, id, before, Some(wrong))).is_err());
    let stop = f.membership(
        &project,
        f.manifest().revision,
        OperationId::generate(),
        &Request::StopSession { session_id: id },
    );
    f.change(&stop).unwrap();
    f.change(&start).unwrap();
    boot(&f).unwrap();
    assert!(!f.root().join(format!("runtime-{id}.json")).exists());
    assert_eq!(
        inspect(&f, &query(&f, &project, id, f.manifest().revision, None))
            .unwrap()
            .status,
        Status::Uncertain
    );
    assert!(f.change(&cancel(&f, &project)).is_err());
    assert!(root(&f, &project, id).join("checkout/committed").exists());
}

#[test]
fn signed_session_observation_rejects_wrong_scope_unknown_sessions_and_replaced_roots() {
    let (f, project, id) = prepared_runtime();
    assert!(inspect(&f, &query(&f, &identity("foreign"), id, f.manifest().revision, None)).is_err());
    assert!(
        inspect(
            &f,
            &query(&f, &project, SessionId::new_v4(), f.manifest().revision, None)
        )
        .is_err()
    );
    let start = f.membership(
        &project,
        f.manifest().revision,
        OperationId::generate(),
        &Request::StartSession { session_id: id },
    );
    anchored_intent(&f, &start);
    let home = root(&f, &project, id).join("home");
    fs::rename(&home, home.with_extension("retained")).unwrap();
    fs::create_dir(&home).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(f.change(&start).is_err());
    assert!(!f.root().join(format!("runtime-{id}.json")).exists());
}

#[test]
fn visible_terminal_record_requires_durable_retry_before_acknowledgement() {
    use crate::bootstrap::store::Publication;
    let (f, project, id) = prepared_runtime();
    let start = f.membership(
        &project,
        f.manifest().revision,
        OperationId::generate(),
        &Request::StartSession { session_id: id },
    );
    let launch = anchored_intent(&f, &start);
    let stop = f.membership(
        &project,
        f.manifest().revision,
        OperationId::generate(),
        &Request::StopSession { session_id: id },
    );
    f.change(&stop).unwrap();
    let request = query(&f, &project, id, f.manifest().revision, None);
    let name = format!("runtime-{id}.json");
    let bytes = serde_json::to_vec(&serde_json::json!({
        "version":1, "launch":launch, "session":id, "nonce":OperationId::generate(),
        "supervisor":null, "agent":null, "status":{"state":"stopped"}
    }))
    .unwrap();
    let manifest = f.manifest();
    let store = Store::open(&f.root()).unwrap();
    assert!(
        store
            .write_with(&name, None, &bytes, &mut |phase| {
                if phase == Publication::Renamed {
                    Err(io::Error::other("interrupted directory sync"))
                } else {
                    Ok(())
                }
            })
            .is_err()
    );
    assert_eq!(store.read(&name).unwrap().unwrap(), bytes);
    store.fail_sync_after(0);
    assert!(session_runtime::inspect_with(&store, &f.runtime, &request).is_err());
    assert!(session_runtime::validate(&store, &manifest, Some(&project)).is_err());
    drop(store);
    assert_eq!(inspect(&f, &request).unwrap().status, Status::Stopped);
    let mut malformed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    malformed["status"]["unexpected"] = true.into();
    let store = Store::open(&f.root()).unwrap();
    store
        .write(&name, Some(&bytes), &serde_json::to_vec(&malformed).unwrap())
        .unwrap();
    assert!(session_runtime::inspect_with(&store, &f.runtime, &request).is_err());
}
