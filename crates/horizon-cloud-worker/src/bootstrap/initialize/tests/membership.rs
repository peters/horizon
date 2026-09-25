mod namespaces;
use super::*;
use crate::bootstrap::{
    membership::{mutate, startup},
    store::Publication,
};
use horizon_cloud::Capabilities;
use horizon_cloud_protocol::{
    ProjectId, ProjectIdentity,
    membership::{Manifest, Receipt, Request, State},
};

fn identity(name: &str) -> ProjectIdentity {
    ProjectIdentity::new(ProjectId::generate(), "session".into(), "workspace".into(), name.into()).unwrap()
}
fn reserve(port: u16, desktop: bool) -> Request {
    let mut capabilities: Capabilities = serde_json::from_str("{}").unwrap();
    capabilities.desktop = desktop;
    Request::Reserve {
        capabilities,
        ports: [port].into(),
    }
}
impl Fixture {
    fn ready() -> Self {
        let f = Self::new();
        f.init(&f.init_request()).unwrap();
        f.recover(&f.recover_request()).unwrap();
        f
    }
    fn membership(
        &self,
        identity: &ProjectIdentity,
        revision: u64,
        operation: OperationId,
        payload: &Request,
    ) -> RecoveryRequest {
        let payload_bytes = serde_json::to_string(payload).unwrap();
        let binding = &self.runtime.startup.controller;
        let intent = Intent::new(
            binding,
            operation,
            revision,
            Target::Project {
                identity: identity.clone(),
            },
            payload.action(),
            payload_bytes.as_bytes(),
        )
        .unwrap();
        RecoveryRequest {
            message: serde_json::to_string(&SignedIntent::sign(intent, binding, &self.controller).unwrap()).unwrap(),
            payload: payload_bytes,
        }
    }
    fn change(&self, request: &RecoveryRequest) -> io::Result<Receipt> {
        let payload: Request = decode(request.payload.as_bytes())?;
        mutate(
            &Store::open(&self.root())?,
            &self.runtime,
            request,
            payload.action(),
            |_| Ok(()),
            &mut |_| Ok(()),
        )
    }
    fn manifest(&self) -> Manifest {
        decode(&fs::read(self.root().join(MANIFEST)).unwrap()).unwrap()
    }
}

#[test]
fn three_reservations_restart_and_cancel_preserve_siblings_and_tombstones() {
    let f = Fixture::ready();
    let mut requests = Vec::new();
    let projects = [identity("one"), identity("two"), identity("three")];
    let sentinel = f.directory.path().join("workspace/user-data");
    fs::write(&sentinel, b"preserve").unwrap();
    for (index, project) in projects.iter().enumerate() {
        let request = f.membership(
            project,
            index as u64,
            OperationId::generate(),
            &reserve(8000 + u16::try_from(index).unwrap(), index == 0),
        );
        let receipt = f.change(&request).unwrap();
        assert_eq!(receipt.state, State::Attaching);
        assert_eq!(receipt, f.change(&request).unwrap());
        requests.push(request);
    }
    let before = f.manifest();
    assert_eq!(
        before
            .members
            .iter()
            .map(|member| &member.namespace)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3
    );
    let store = Store::open(&f.root()).unwrap();
    let bootstrap: Bootstrap = decode(&store.read(BOOTSTRAP).unwrap().unwrap()).unwrap();
    startup(&store, &bootstrap).unwrap();
    assert_eq!(&*store.host_key().unwrap(), &*f.key);
    assert!(bootstrap.empty(&store).is_err());
    drop(store);
    let cancel = f.membership(&projects[0], 3, OperationId::generate(), &Request::Cancel {});
    let removed = f.change(&cancel).unwrap();
    assert_eq!(removed.state, State::Removed);
    assert_eq!(f.change(&cancel).unwrap(), removed);
    assert!(f.change(&requests[0]).is_err());
    let after = f.manifest();
    assert_eq!(&after.members[1..], &before.members[1..]);
    assert_eq!(fs::read(sentinel).unwrap(), b"preserve");
    assert!(
        f.change(&f.membership(&projects[0], 4, OperationId::generate(), &reserve(9000, false)))
            .is_err()
    );
    // Logical resources can move to a new identity, but old identities stay dead.
    f.change(&f.membership(&identity("four"), 4, OperationId::generate(), &reserve(8000, true)))
        .unwrap();
    assert!(f.recover(&f.recover_request()).is_err());
    assert!(f.abandon(&f.abandon_request()).is_err());
    assert!(f.init(&f.init_request()).is_err());
}

#[test]
fn authentication_conflicts_and_changed_retries_leave_manifest_unchanged() {
    let f = Fixture::ready();
    let project = identity("one");
    let operation = OperationId::generate();
    let original = f.membership(&project, 0, operation, &reserve(8000, true));
    f.change(&original).unwrap();
    let bytes = fs::read(f.root().join(MANIFEST)).unwrap();
    let foreign = Fixture::ready();
    let duplicate_project =
        ProjectIdentity::new(project.project_id(), "session".into(), "other".into(), "other".into()).unwrap();
    for request in [
        f.membership(&project, 0, operation, &reserve(8001, false)),
        f.membership(&identity("new"), 1, operation, &reserve(8001, false)),
        f.membership(&identity("one"), 1, OperationId::generate(), &reserve(8001, false)),
        f.membership(&duplicate_project, 1, OperationId::generate(), &reserve(8001, false)),
        f.membership(&identity("new"), 0, OperationId::generate(), &reserve(8001, false)),
        f.membership(&identity("new"), 1, OperationId::generate(), &reserve(8000, false)),
        f.membership(&identity("new"), 1, OperationId::generate(), &reserve(8001, true)),
        f.membership(&identity("new"), 1, OperationId::generate(), &Request::Cancel {}),
        foreign.membership(&identity("new"), 1, OperationId::generate(), &reserve(8001, false)),
    ] {
        assert!(f.change(&request).is_err());
        assert_eq!(fs::read(f.root().join(MANIFEST)).unwrap(), bytes);
    }
    let store = Store::open(&f.root()).unwrap();
    assert!(Store::open(&f.root()).is_err());
    assert!(
        mutate(
            &store,
            &f.runtime,
            &original,
            Action::RemoveProject,
            |_| panic!("wrong route"),
            &mut |_| Ok(())
        )
        .is_err()
    );
    // An exact retry does not masquerade as a fresh capability check.
    mutate(
        &store,
        &f.runtime,
        &original,
        Action::AttachProject,
        |_| panic!("historical receipt"),
        &mut |_| Ok(()),
    )
    .unwrap();
}

#[test]
fn ports_grants_and_missing_capabilities_fail_before_publication() {
    let f = Fixture::ready();
    for port in [0, 22, 1023, 5900, 5999, 6000, 6099, 47280] {
        let request = f.membership(&identity("one"), 0, OperationId::generate(), &reserve(port, false));
        assert!(f.change(&request).is_err());
    }
    let mut remote = reserve(8000, false);
    if let Request::Reserve { capabilities, .. } = &mut remote {
        capabilities.browserstack = Some(horizon_cloud::BrowserStack {
            local_ports: [8001].into(),
            ..Default::default()
        });
    }
    assert!(
        f.change(&f.membership(&identity("one"), 0, OperationId::generate(), &remote))
            .is_err()
    );
    let request = f.membership(&identity("one"), 0, OperationId::generate(), &reserve(8000, false));
    assert!(
        mutate(
            &Store::open(&f.root()).unwrap(),
            &f.runtime,
            &request,
            Action::AttachProject,
            |_| Err(invalid()),
            &mut |_| Ok(())
        )
        .is_err()
    );
    assert_eq!(f.manifest().revision, 0);
}

#[test]
fn publication_failures_reopen_to_one_operation_and_durable_retry() {
    for boundary in [Publication::Staged, Publication::Renamed, Publication::Durable] {
        let f = Fixture::ready();
        let request = f.membership(&identity("one"), 0, OperationId::generate(), &reserve(8000, false));
        assert!(
            mutate(
                &Store::open(&f.root()).unwrap(),
                &f.runtime,
                &request,
                Action::AttachProject,
                |_| Ok(()),
                &mut |at| if at == boundary { Err(invalid()) } else { Ok(()) }
            )
            .is_err()
        );
        assert_eq!(f.manifest().revision, u64::from(boundary != Publication::Staged));
        let receipt = f.change(&request).unwrap();
        assert_eq!(f.change(&request).unwrap(), receipt);
        assert_eq!(f.manifest().operations.len(), 1);
    }
    for successes in [0, 1] {
        let f = Fixture::ready();
        let request = f.membership(&identity("one"), 0, OperationId::generate(), &reserve(8000, false));
        let store = Store::open(&f.root()).unwrap();
        store.fail_sync_after(successes);
        assert!(
            mutate(
                &store,
                &f.runtime,
                &request,
                Action::AttachProject,
                |_| Ok(()),
                &mut |_| Ok(())
            )
            .is_err()
        );
        assert!(
            mutate(
                &store,
                &f.runtime,
                &request,
                Action::AttachProject,
                |_| panic!("already published"),
                &mut |_| Ok(())
            )
            .is_err()
        );
        drop(store);
        assert_eq!(f.change(&request).unwrap().revision, 1);
    }
}

#[test]
fn changed_bootstrap_at_probe_or_publication_never_acknowledges() {
    for boundary in [
        None,
        Some(Publication::Staged),
        Some(Publication::Renamed),
        Some(Publication::Durable),
    ] {
        let f = Fixture::ready();
        let request = f.membership(&identity("one"), 0, OperationId::generate(), &reserve(8000, false));
        let store = Store::open(&f.root()).unwrap();
        let change = || fs::write(f.root().join(BOOTSTRAP), b"changed");
        assert!(
            mutate(
                &store,
                &f.runtime,
                &request,
                Action::AttachProject,
                |_| {
                    if boundary.is_none() {
                        change()?;
                    }
                    Ok(())
                },
                &mut |at| {
                    if boundary == Some(at) {
                        change()?;
                    }
                    Ok(())
                }
            )
            .is_err()
        );
        if boundary.is_none() || boundary == Some(Publication::Staged) {
            assert_eq!(f.manifest().revision, 0);
        }
    }
}

#[test]
fn descriptor_substitution_before_commit_preserves_foreign_directory() {
    let f = Fixture::ready();
    let request = f.membership(&identity("one"), 0, OperationId::generate(), &reserve(8000, false));
    let store = Store::open(&f.root()).unwrap();
    assert!(
        mutate(
            &store,
            &f.runtime,
            &request,
            Action::AttachProject,
            |_| {
                fs::rename(f.root(), f.root().with_extension("retained"))?;
                fs::create_dir(f.root())?;
                fs::write(f.root().join("sentinel"), b"preserve")
            },
            &mut |_| Ok(())
        )
        .is_err()
    );
    assert_eq!(fs::read(f.root().join("sentinel")).unwrap(), b"preserve");
    let retained: Manifest = decode(&fs::read(f.root().with_extension("retained").join(MANIFEST)).unwrap()).unwrap();
    assert_eq!(retained.revision, 0);
}

#[test]
fn corrupt_history_or_members_cannot_start_or_mutate() {
    let f = Fixture::ready();
    let request = f.membership(&identity("one"), 0, OperationId::generate(), &reserve(8000, false));
    f.change(&request).unwrap();
    let original = serde_json::to_value(f.manifest()).unwrap();
    for variant in 0..7 {
        let mut changed = original.clone();
        match variant {
            0 => changed["members"][0]["namespace"] = "other".into(),
            1 => changed["members"][0]["state"] = "removed".into(),
            2 => changed["operations"][0]["payload"] = "{}".into(),
            3 => changed["operations"][0]["receipt"]["revision"] = 99.into(),
            4 => changed["revision"] = 0.into(),
            5 => changed["worker_id"] = "foreign".into(),
            _ => changed["operations"] = serde_json::json!([]),
        }
        fs::write(f.root().join(MANIFEST), serde_json::to_vec(&changed).unwrap()).unwrap();
        let store = Store::open(&f.root()).unwrap();
        let bootstrap: Bootstrap = decode(&store.read(BOOTSTRAP).unwrap().unwrap()).unwrap();
        assert!(startup(&store, &bootstrap).is_err());
        assert!(
            mutate(
                &store,
                &f.runtime,
                &request,
                Action::AttachProject,
                |_| panic!("corrupt history"),
                &mut |_| Ok(())
            )
            .is_err()
        );
    }
}

#[test]
fn dedicated_tombstones_and_bootstrap_operation_ids_are_not_reusable() {
    let mut f = Fixture::new();
    f.runtime.startup.sharing = SharingMode::Dedicated;
    let init = f.init_request();
    f.init(&init).unwrap();
    f.recover(&f.recover_request()).unwrap();
    let signed = SignedIntent::parse(init.message.as_bytes()).unwrap();
    let op = signed
        .verify(&f.runtime.startup.controller, init.payload.as_bytes())
        .unwrap()
        .operation();
    let project = identity("one");
    for operation in [op, f.runtime.startup.token] {
        assert!(
            f.change(&f.membership(&project, 0, operation, &reserve(8000, false)))
                .is_err()
        );
    }
    f.change(&f.membership(&project, 0, OperationId::generate(), &reserve(8000, false)))
        .unwrap();
    f.change(&f.membership(&project, 1, OperationId::generate(), &Request::Cancel {}))
        .unwrap();
    assert!(
        f.change(&f.membership(&identity("two"), 2, OperationId::generate(), &reserve(8000, false)))
            .is_err()
    );
    assert!(f.abandon(&f.abandon_request()).is_err());
}

#[test]
fn capacity_rejection_preserves_room_to_cancel_every_retained_project() {
    let f = Fixture::ready();
    let mut projects = Vec::new();
    for index in 0..32 {
        let project = ProjectIdentity::new(
            ProjectId::generate(),
            "s".repeat(100),
            "w".repeat(100),
            format!("{index:0100}"),
        )
        .unwrap();
        let request = f.membership(
            &project,
            index,
            OperationId::generate(),
            &reserve(8000 + u16::try_from(index).unwrap(), false),
        );
        if f.change(&request).is_err() {
            break;
        }
        projects.push(project);
    }
    assert!(projects.len() >= 3 && projects.len() < 32);
    let count = projects.len() as u64;
    for (index, project) in projects.iter().enumerate() {
        let request = f.membership(
            project,
            count + index as u64,
            OperationId::generate(),
            &Request::Cancel {},
        );
        f.change(&request).unwrap();
    }
    let manifest = f.manifest();
    manifest.validate().unwrap();
    assert!(manifest.members.iter().all(|member| member.state == State::Removed));
    assert_eq!(manifest.revision, count * 2);
    assert!(serde_json::to_vec(&manifest).unwrap().len() <= horizon_cloud_protocol::membership::MAX_MANIFEST_BYTES);
}

#[test]
fn uncertain_cancellation_cannot_revive_a_project_after_publication() {
    for boundary in [Publication::Staged, Publication::Renamed, Publication::Durable] {
        let f = Fixture::ready();
        let project = identity("one");
        let attach = f.membership(&project, 0, OperationId::generate(), &reserve(8000, true));
        f.change(&attach).unwrap();
        let cancel = f.membership(&project, 1, OperationId::generate(), &Request::Cancel {});
        assert!(
            mutate(
                &Store::open(&f.root()).unwrap(),
                &f.runtime,
                &cancel,
                Action::RemoveProject,
                |_| panic!("cancellation must not probe"),
                &mut |at| if at == boundary { Err(invalid()) } else { Ok(()) }
            )
            .is_err()
        );
        if boundary == Publication::Staged {
            assert_eq!(f.change(&attach).unwrap().state, State::Attaching);
        } else {
            assert!(f.change(&attach).is_err());
        }
        let receipt = f.change(&cancel).unwrap();
        assert_eq!(receipt.state, State::Removed);
        assert_eq!(receipt, f.change(&cancel).unwrap());
        assert!(f.change(&attach).is_err());
        assert_eq!(f.manifest().operations.len(), 2);
        assert!(f.abandon(&f.abandon_request()).is_err());
    }
}
