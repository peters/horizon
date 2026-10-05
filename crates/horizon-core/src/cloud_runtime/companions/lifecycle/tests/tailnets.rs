use super::*;

#[test]
fn tailnet_selection_is_nonsecret_authorized_and_bound_to_the_operation() {
    let f = Fixture::new();
    std::fs::write(
        f.root.path().join("tailnets.json"),
        br#"{"tailnets":[{"id":"work","name":"Work"}]}"#,
    )
    .unwrap();
    let mut before = f.ready();
    before.spec = None;
    before.worker = None;
    before.operation = CreateState::Prepared;
    f.save(&before);
    let id = OperationId::generate();
    assert!(submit_with_tailnet(&f.request(), Action::EnsureReady, id, Some("unknown")).is_err());
    assert!(submit_with_tailnet(&f.request(), Action::Stop, id, Some("work")).is_err());
    let op = submit_with_tailnet(&f.request(), Action::EnsureReady, id, Some("work")).unwrap();
    assert_eq!(op.phase, Phase::Submitted);
    let target = f.root.path().join("target");
    assert_eq!(
        horizon_cloud::tailnet::Selection::load(&target)
            .unwrap()
            .tailnet
            .as_deref(),
        Some("work")
    );
    let original = serde_json::to_value(f.request().load().unwrap().1.intents).unwrap();
    assert!(submit_with_tailnet(&f.request(), Action::EnsureReady, id, Some("none")).is_err());
    assert!(submit_with_tailnet(&f.request(), Action::EnsureReady, OperationId::generate(), Some("none")).is_err());
    assert_eq!(
        serde_json::to_value(f.request().load().unwrap().1.intents).unwrap(),
        original
    );
    assert_eq!(
        submit_with_tailnet(&f.request(), Action::EnsureReady, id, Some("work"))
            .unwrap()
            .intent
            .operation_id,
        id
    );
    let catalog = crate::cloud_runtime::tailnet::store(f.root.path()).load().unwrap();
    horizon_cloud::tailnet::Selection::save(&target, None, &catalog).unwrap();
    let store = Store::lock(&target).unwrap();
    receipt::save(&store, &f.owner, id, Phase::Ready).unwrap();
    drop(store);
    submit_with_tailnet(&f.request(), Action::EnsureReady, id, Some("work")).unwrap();
    assert!(
        horizon_cloud::tailnet::Selection::load(&target)
            .unwrap()
            .tailnet
            .is_none(),
        "a stale retry cannot restore its old choice"
    );
}

#[test]
fn pending_tailnet_requests_fence_edits_and_fail_before_execution_on_drift() {
    for requested in [Some("work"), Some("none"), None] {
        let f = Fixture::new();
        std::fs::write(
            f.root.path().join("tailnets.json"),
            br#"{"tailnets":[{"id":"work","name":"Work"}]}"#,
        )
        .unwrap();
        let mut before = f.ready();
        before.spec = None;
        before.worker = None;
        before.operation = CreateState::Prepared;
        f.save(&before);
        let id = OperationId::generate();
        submit_with_tailnet(&f.request(), Action::EnsureReady, id, requested).unwrap();
        let selected = requested.filter(|id| *id != "none");
        let changed = if selected.is_some() { None } else { Some("work") };
        let root = f.root.path();
        assert!(crate::cloud_runtime::tailnet::change(root, "target", selected).is_ok());
        assert!(crate::cloud_runtime::tailnet::change(root, "target", changed).is_err());
        assert!(crate::cloud_runtime::tailnet::select(root, "target", changed).is_err());
        let catalog = crate::cloud_runtime::tailnet::store(root).load().unwrap();
        let target = root.join("target");
        horizon_cloud::tailnet::Selection::save(&target, changed, &catalog).unwrap();
        assert!(crate::cloud_runtime::tailnet::validate_pending(&target).is_err());
        let mut backend = Fake::new(&f);
        assert!(execute_with(&f.request(), id, &mut backend).is_err());
        assert!(backend.decisions.is_empty());
        cancel_submission(&f.request(), id).unwrap();
        assert!(crate::cloud_runtime::tailnet::change(root, "target", changed).is_ok());
        assert!(crate::cloud_runtime::tailnet::validate_pending(&target).is_ok());
    }
}

#[test]
fn uncommitted_tailnet_requests_never_change_the_default_selection() {
    for interrupt_before_source_write in [true, false] {
        let f = Fixture::new();
        std::fs::write(
            f.root.path().join("tailnets.json"),
            br#"{"tailnets":[{"id":"work","name":"Work"}]}"#,
        )
        .unwrap();
        let mut prepared = f.ready();
        prepared.worker = None;
        prepared.spec = None;
        prepared.operation = CreateState::Prepared;
        f.save(&prepared);
        let target = f.root.path().join("target");
        let id = OperationId::generate();
        if interrupt_before_source_write {
            let store = Store::lock(&target).unwrap();
            let _staged = tailnet_choice::prepare(f.root.path(), &store, id, Some("work")).unwrap();
            // Simulate exit after the request file is durable, before source persistence.
        } else {
            let store = journal::Store::open(f.root.path(), &f.owner).unwrap();
            let mut state = store.load().unwrap();
            let bytes = serde_json::to_vec(&state).unwrap().len();
            state.grants.get_mut("consumer").unwrap().revision = Some("x".repeat(256 * 1024 - bytes - 5));
            store.save(&state).unwrap();
            drop(store);
            // The existing source loads, but adding an intent exceeds its durable size limit.
            assert!(submit_with_tailnet(&f.request(), Action::EnsureReady, id, Some("work")).is_err());
            let store = journal::Store::open(f.root.path(), &f.owner).unwrap();
            let mut state = store.load().unwrap();
            assert!(state.intents.operation(id).is_none());
            state.grants.get_mut("consumer").unwrap().revision = None;
            store.save(&state).unwrap();
        }
        assert!(target.join(format!("tailnet-request-{id}.json")).is_file());
        assert!(receipt::load(&target).unwrap().is_none());
        assert!(
            horizon_cloud::tailnet::Selection::load(&target)
                .unwrap()
                .tailnet
                .is_none()
        );
        submit(&f.request(), Action::EnsureReady, OperationId::generate()).unwrap();
        assert!(
            horizon_cloud::tailnet::Selection::load(&target)
                .unwrap()
                .tailnet
                .is_none()
        );
        assert!(crate::cloud_runtime::tailnet::validate_pending(&target).is_ok());
    }
}

#[test]
fn durable_tailnet_retries_survive_catalog_deletion_or_corruption() {
    for phase in [Phase::Submitted, Phase::Ready] {
        for catalog_bytes in [b"{\"tailnets\":[]}".as_slice(), b"invalid".as_slice()] {
            let f = Fixture::new();
            let catalog_path = f.root.path().join("tailnets.json");
            std::fs::write(&catalog_path, br#"{"tailnets":[{"id":"work","name":"Work"}]}"#).unwrap();
            let target = f.root.path().join("target");
            let catalog = crate::cloud_runtime::tailnet::store(f.root.path()).load().unwrap();
            horizon_cloud::tailnet::Selection::save(&target, Some("work"), &catalog).unwrap();
            let id = OperationId::generate();
            submit_with_tailnet(&f.request(), Action::EnsureReady, id, Some("work")).unwrap();
            if phase == Phase::Ready {
                let mut backend = Fake::new(&f);
                assert_eq!(execute_with(&f.request(), id, &mut backend).unwrap().phase, phase);
            }
            let request_path = target.join(format!("tailnet-request-{id}.json"));
            let saved = std::fs::read(&request_path).unwrap();
            std::fs::write(&catalog_path, catalog_bytes).unwrap();
            for requested in [Some("work"), None] {
                let retried = submit_with_tailnet(&f.request(), Action::EnsureReady, id, requested).unwrap();
                assert_eq!(retried.intent.operation_id, id);
                assert_eq!(retried.phase, phase);
            }
            for changed in ["none", "unknown"] {
                assert!(submit_with_tailnet(&f.request(), Action::EnsureReady, id, Some(changed)).is_err());
            }
            if phase == Phase::Ready {
                assert!(
                    submit_with_tailnet(&f.request(), Action::EnsureReady, OperationId::generate(), Some("work"))
                        .is_err()
                );
            } else {
                assert_eq!(
                    submit_with_tailnet(&f.request(), Action::EnsureReady, OperationId::generate(), Some("work"))
                        .unwrap()
                        .intent
                        .operation_id,
                    id
                );
            }
            assert_eq!(std::fs::read(request_path).unwrap(), saved);
            assert_eq!(
                horizon_cloud::tailnet::Selection::load(&target)
                    .unwrap()
                    .tailnet
                    .as_deref(),
                Some("work")
            );
        }
    }
}

#[test]
fn interrupted_selection_commit_recovers_from_the_durable_request_without_catalog() {
    for requested in [Some("work"), Some("none")] {
        for execute_directly in [false, true] {
            let f = Fixture::new();
            let catalog_path = f.root.path().join("tailnets.json");
            std::fs::write(&catalog_path, br#"{"tailnets":[{"id":"work","name":"Work"}]}"#).unwrap();
            let mut prepared = f.ready();
            prepared.worker = None;
            prepared.spec = None;
            prepared.operation = CreateState::Prepared;
            f.save(&prepared);
            let target = f.root.path().join("target");
            let catalog = crate::cloud_runtime::tailnet::store(f.root.path()).load().unwrap();
            let prior = if requested == Some("none") { Some("work") } else { None };
            horizon_cloud::tailnet::Selection::save(&target, prior, &catalog).unwrap();
            let id = OperationId::generate();
            {
                let store = Store::lock(&target).unwrap();
                let choice = tailnet_choice::prepare(f.root.path(), &store, id, requested).unwrap();
                let source = journal::Store::open(f.root.path(), &f.owner).unwrap();
                let mut state = source.load().unwrap();
                state.intents.submit("consumer", Action::EnsureReady, id).unwrap();
                source.save(&state).unwrap();
                receipt::save(&store, &f.owner, id, Phase::Submitted).unwrap();
                // Force the last write to fail after both intent and target claim are durable.
                std::fs::remove_file(target.join("tailnet.json")).unwrap();
                std::fs::create_dir(target.join("tailnet.json")).unwrap();
                assert!(choice.commit(&store).is_err());
            }
            assert!(target.join(format!("tailnet-commit-{id}.pending")).is_file());
            std::fs::remove_dir(target.join("tailnet.json")).unwrap();
            horizon_cloud::tailnet::Selection::save(&target, prior, &catalog).unwrap();
            f.save(&f.ready());
            assert!(submit_with_tailnet(&f.request(), Action::EnsureReady, id, requested).is_err());
            assert_eq!(
                horizon_cloud::tailnet::Selection::load(&target)
                    .unwrap()
                    .tailnet
                    .as_deref(),
                prior
            );
            f.save(&prepared);
            std::fs::remove_file(catalog_path).unwrap();
            let saved = std::fs::read(target.join(format!("tailnet-request-{id}.json"))).unwrap();
            if execute_directly {
                let mut backend = Fake::new(&f);
                assert_eq!(
                    execute_with(&f.request(), id, &mut backend).unwrap().phase,
                    Phase::ConfirmationRequired
                );
                assert!(backend.decisions.is_empty());
            } else {
                assert_eq!(
                    submit_with_tailnet(&f.request(), Action::EnsureReady, id, requested)
                        .unwrap()
                        .phase,
                    Phase::Submitted
                );
                submit_with_tailnet(&f.request(), Action::EnsureReady, id, requested).unwrap();
            }
            assert_eq!(
                horizon_cloud::tailnet::Selection::load(&target)
                    .unwrap()
                    .tailnet
                    .as_deref(),
                requested.filter(|id| *id != "none")
            );
            assert_eq!(
                std::fs::read(target.join(format!("tailnet-request-{id}.json"))).unwrap(),
                saved
            );
            assert!(!target.join(format!("tailnet-commit-{id}.pending")).exists());
            assert!(crate::cloud_runtime::tailnet::validate_pending(&target).is_ok());
        }
    }
}
