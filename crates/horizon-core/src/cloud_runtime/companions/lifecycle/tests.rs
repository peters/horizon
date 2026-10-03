use super::super::{Declaration, Scope, Selection, Target};
use super::*;
mod recovery;
use crate::cloud_runtime::{CreateState, Stage, state::Deployment};
use intent::{Decision, Origin};

struct Fixture {
    root: tempfile::TempDir,
    owner: Owner,
    context: Context,
    binding: Binding,
}
impl Fixture {
    fn new() -> Self {
        let this = Self::unbound();
        bind(&this.request(), this.binding.clone()).unwrap();
        this
    }
    /// A checked companion with a ready deployment and no lifecycle binding yet.
    fn unbound() -> Self {
        let root = tempfile::tempdir().unwrap();
        let owner = Owner {
            scope: Scope {
                session_id: "session".into(),
                workspace_id: "workspace".into(),
            },
            cloud_id: "source".into(),
        };
        let target = Target {
            scope: owner.scope.clone(),
            cloud_id: "target".into(),
            declaration: Declaration::new("example/consumer", "cpu"),
        };
        let source = Target {
            scope: owner.scope.clone(),
            cloud_id: owner.cloud_id.clone(),
            declaration: Declaration::new("example/source", "cpu"),
        };
        let binding = Binding::new(
            &owner,
            "consumer",
            target.clone(),
            root.path().join("checkout"),
            Origin::Existing,
        )
        .unwrap();
        let context = Context {
            source,
            declarations: [("consumer".into(), target.declaration.clone())].into(),
            inventory: vec![target],
        };
        let this = Self {
            root,
            owner,
            context,
            binding,
        };
        this.select(&this.owner);
        this.save(&this.ready());
        this
    }
    fn request(&self) -> Request<'_> {
        Request {
            root: self.root.path(),
            owner: &self.owner,
            context: &self.context,
            alias: "consumer",
        }
    }
    fn select(&self, owner: &Owner) {
        let store = journal::Store::open(self.root.path(), owner).unwrap();
        let mut state = store.load().unwrap();
        let mut source = self.context.source.clone();
        source.cloud_id.clone_from(&owner.cloud_id);
        state.grants.insert(
            "consumer".into(),
            journal::Grant {
                selection: Selection::new(&source, "consumer", self.binding.target()).unwrap(),
                target: self.binding.target().clone(),
                id: "grant".into(),
                selected: true,
                source_worker: None,
                target_worker: None,
                revision: None,
                source_disconnected: true,
                target_revoked: true,
                source_forgotten: true,
                access: None,
            },
        );
        store.save(&state).unwrap();
    }
    fn ready(&self) -> Deployment {
        serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"target","repository":self.binding.checkout(),"revision":"a".repeat(40),
            "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
            "stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},"spec":null,"sessions":[],
            "source_ready":true,"worker":{"id":"worker1","name":"w","imageName":"i","desiredStatus":"RUNNING",
            "publicIp":"203.0.113.7","portMappings":{"22":22022}}
        }))
        .unwrap()
    }
    fn save(&self, state: &Deployment) {
        Store::lock(&self.root.path().join("target"))
            .unwrap()
            .save(state)
            .unwrap();
    }
    fn submit(&self, action: Action) -> Operation {
        submit(&self.request(), action, OperationId::generate()).unwrap()
    }
}

struct Fake<'a> {
    fixture: &'a Fixture,
    decisions: Vec<Decision>,
    fail: bool,
    /// Whether a failed run may have left a provider mutation unconfirmed.
    uncertain: bool,
    access: Phase,
    /// Takes the source journal's lock during the run and keeps it, as a
    /// concurrent refresh can.
    hold_journal: bool,
    held: Option<journal::Store>,
    /// Whether each run was told it only inspects.
    inspected: Vec<bool>,
    /// Whether a run proves the earlier uncertain operation settled.
    settled: bool,
}
impl<'a> Fake<'a> {
    fn new(fixture: &'a Fixture) -> Self {
        Self {
            fixture,
            decisions: Vec::new(),
            fail: false,
            uncertain: true,
            access: Phase::Ready,
            hold_journal: false,
            held: None,
            inspected: Vec::new(),
            settled: false,
        }
    }
}
impl execution::Backend for Fake<'_> {
    fn inspecting(&mut self, inspecting: bool) {
        self.inspected.push(inspecting);
    }
    fn uncertainty_settled(&self) -> bool {
        self.settled
    }
    fn run(&mut self, store: &Store, decision: Decision, action: Action) -> Result<Phase> {
        assert!(
            matches!(Store::lock(store.root()), Err(Error::Busy)),
            "provider and guard share the target lock"
        );
        let claim = receipt::load(store.root())?.unwrap();
        match decision {
            Decision::Reuse | Decision::VerifyAccess => assert_eq!(claim.phase, Phase::VerifyingAccess),
            Decision::AlreadyStopped | Decision::Refuse(_) => assert_eq!(claim.phase, Phase::Settling),
            _ => {}
        }
        let operation = status(&self.fixture.request(), claim.id)?;
        assert!(matches!(operation.intent.state, State::Executing | State::Uncertain));
        let duplicate = submit(&self.fixture.request(), action, OperationId::generate())?;
        assert_eq!(
            duplicate.intent.operation_id, claim.id,
            "a live operation still deduplicates"
        );
        self.decisions.push(decision);
        if self.hold_journal {
            self.held = Some(journal::Store::open(self.fixture.root.path(), &self.fixture.owner)?);
        }
        if self.fail {
            return Err(Error::Invalid("Synthetic lost response"));
        }
        match decision {
            Decision::Refuse(_) => Ok(Phase::Refused),
            Decision::ReconcileOnly => Ok(Phase::ReconcileRequired),
            _ if action == Action::Stop => Ok(Phase::Stopped),
            _ => Ok(Phase::Ready),
        }
    }
    fn mutation_uncertain(&self) -> bool {
        self.uncertain
    }
    fn verify_access(&mut self, _: &Request<'_>) -> Result<Phase> {
        Ok(self.access)
    }
}

#[test]
fn submission_and_polling_are_passive_and_duplicates_preserve_their_id() {
    let fixture = Fixture::new();
    let original = std::fs::read(fixture.root.path().join("target/deployment.json")).unwrap();
    let first = fixture.submit(Action::EnsureReady);
    let retry_id = OperationId::generate();
    let second = submit(&fixture.request(), Action::EnsureReady, retry_id).unwrap();
    assert_eq!(first.intent.operation_id, second.intent.operation_id);
    assert_eq!(status(&fixture.request(), retry_id).unwrap().phase, Phase::Submitted);
    assert_eq!(
        std::fs::read(fixture.root.path().join("target/deployment.json")).unwrap(),
        original
    );
    let mut backend = Fake::new(&fixture);
    execute_with(&fixture.request(), first.intent.operation_id, &mut backend).unwrap();
    let stop = fixture.submit(Action::Stop);
    execute_with(&fixture.request(), stop.intent.operation_id, &mut backend).unwrap();
    let delayed = submit(&fixture.request(), Action::EnsureReady, retry_id).unwrap();
    assert_eq!(delayed.intent.operation_id, first.intent.operation_id);
    execute_with(&fixture.request(), retry_id, &mut backend).unwrap();
    assert_eq!(
        backend.decisions.len(),
        2,
        "terminal retries cannot undo the newer Stop"
    );
}

#[test]
fn uncertain_execution_never_replays_and_blocks_other_sources() {
    let fixture = Fixture::new();
    let mut state = fixture.ready();
    state.stage = Stage::Readiness;
    fixture.save(&state);
    let op = fixture.submit(Action::EnsureReady);
    let mut backend = Fake::new(&fixture);
    backend.fail = true;
    assert!(execute_with(&fixture.request(), op.intent.operation_id, &mut backend).is_err());
    assert_eq!(
        status(&fixture.request(), op.intent.operation_id).unwrap().intent.state,
        State::Uncertain
    );
    backend.fail = false;
    execute_with(&fixture.request(), op.intent.operation_id, &mut backend).unwrap();
    assert_eq!(backend.decisions, [Decision::Reconnect, Decision::ReconcileOnly]);
    assert!(matches!(
        submit(&fixture.request(), Action::Stop, OperationId::generate()),
        Err(Error::Busy)
    ));
    let mut owner = fixture.owner.clone();
    owner.cloud_id = "another-source".into();
    fixture.select(&owner);
    let mut context = fixture.context.clone();
    context.source.cloud_id.clone_from(&owner.cloud_id);
    let request = Request {
        owner: &owner,
        context: &context,
        ..fixture.request()
    };
    let binding = Binding::new(
        &owner,
        "consumer",
        fixture.binding.target().clone(),
        fixture.binding.checkout().into(),
        Origin::Existing,
    )
    .unwrap();
    bind(&request, binding).unwrap();
    assert!(matches!(
        submit(&request, Action::EnsureReady, OperationId::generate()),
        Err(Error::Busy)
    ));
}

#[test]
fn a_failure_before_any_provider_mutation_allows_a_fresh_request() {
    let fixture = Fixture::new();
    let mut state = fixture.ready();
    state.stage = Stage::Readiness;
    fixture.save(&state);
    let op = fixture.submit(Action::EnsureReady);
    let mut backend = Fake::new(&fixture);
    // As a reconnect refused by a local check, or a stop the provider definitely
    // refused and that was rolled back: nothing was left pending.
    backend.fail = true;
    backend.uncertain = false;
    assert!(execute_with(&fixture.request(), op.intent.operation_id, &mut backend).is_err());
    assert_eq!(
        status(&fixture.request(), op.intent.operation_id).unwrap().intent.state,
        State::RetryRequired
    );
    backend.fail = false;
    let retry = fixture.submit(Action::EnsureReady);
    execute_with(&fixture.request(), retry.intent.operation_id, &mut backend).unwrap();
    assert_eq!(
        backend.decisions,
        [Decision::Reconnect, Decision::Reconnect],
        "the repaired reconnect runs again rather than only reconciling"
    );
}

#[test]
fn a_definite_failure_survives_contention_on_the_source_journal() {
    let fixture = Fixture::new();
    let op = fixture.submit(Action::Stop);
    let mut backend = Fake::new(&fixture);
    backend.fail = true;
    backend.uncertain = false;
    backend.hold_journal = true;
    // A refresh holds the source journal when the run finishes.
    assert!(matches!(
        execute_with(&fixture.request(), op.intent.operation_id, &mut backend),
        Err(Error::Busy)
    ));
    let pending = status(&fixture.request(), op.intent.operation_id);
    assert!(matches!(pending, Err(Error::Busy)), "the journal is still held");
    backend.held = None;
    let shown = status(&fixture.request(), op.intent.operation_id).unwrap();
    assert_eq!(
        shown.phase,
        Phase::RetryRequired,
        "the target records the definite failure"
    );
    backend.hold_journal = false;
    let settled = execute_with(&fixture.request(), op.intent.operation_id, &mut backend).unwrap();
    assert_eq!(settled.intent.state, State::RetryRequired);
    backend.fail = false;
    let retry = fixture.submit(Action::Stop);
    execute_with(&fixture.request(), retry.intent.operation_id, &mut backend).unwrap();
    assert_eq!(
        backend.decisions,
        [Decision::Stop, Decision::Stop],
        "the stop runs again rather than only reconciling"
    );
}

#[test]
fn ready_requires_a_fresh_grant_and_a_stale_id_does_not_refresh_access() {
    let fixture = Fixture::new();
    let op = fixture.submit(Action::EnsureReady);
    let mut backend = Fake::new(&fixture);
    backend.access = Phase::ReconcileRequired;
    let result = execute_with(&fixture.request(), op.intent.operation_id, &mut backend).unwrap();
    assert_eq!(result.intent.state, State::RetryRequired);
    assert_eq!(result.phase, Phase::RetryRequired);
    assert_eq!(fixture.submit(Action::Stop).phase, Phase::Submitted);
    let (store, mut journal) = fixture.request().load().unwrap();
    journal.grants.get_mut("consumer").unwrap().selected = false;
    store.save(&journal).unwrap();
    drop(store);
    assert!(execute_with(&fixture.request(), op.intent.operation_id, &mut backend).is_err());
    assert_eq!(backend.decisions.len(), 1);
}

#[test]
fn deleted_terminated_and_lost_records_never_reach_reconnect() {
    for kind in ["deleted", "terminated", "lost", "missing", "identity"] {
        let fixture = Fixture::new();
        let mut state = fixture.ready();
        match kind {
            "deleted" => state.stage = Stage::Deleted,
            "terminated" => {
                state.operation = CreateState::Terminated {
                    worker_id: "worker1".into(),
                }
            }
            "lost" => state.worker.as_mut().unwrap().desired_status = "TERMINATED".into(),
            "identity" => state.cloud_id = "other".into(),
            _ => {}
        }
        fixture.save(&state);
        if kind == "missing" {
            std::fs::remove_file(fixture.root.path().join("target/deployment.json")).unwrap();
        }
        let op = fixture.submit(Action::EnsureReady);
        let mut backend = Fake::new(&fixture);
        assert_eq!(
            execute_with(&fixture.request(), op.intent.operation_id, &mut backend)
                .unwrap()
                .phase,
            Phase::Refused,
            "{kind}"
        );
        assert!(matches!(backend.decisions.as_slice(), [Decision::Refuse(_)]));
    }
}

#[test]
fn target_lock_busy_and_missing_claim_never_execute_a_provider_call() {
    let fixture = Fixture::new();
    let op = fixture.submit(Action::EnsureReady);
    let held = Store::lock(&fixture.root.path().join("target")).unwrap();
    let mut backend = Fake::new(&fixture);
    assert!(matches!(
        execute_with(&fixture.request(), op.intent.operation_id, &mut backend),
        Err(Error::Busy)
    ));
    assert_eq!(
        status(&fixture.request(), op.intent.operation_id).unwrap().phase,
        Phase::Submitted
    );
    drop(held);
    std::fs::remove_file(fixture.root.path().join("target/companion-operation.json")).unwrap();
    assert!(execute_with(&fixture.request(), op.intent.operation_id, &mut backend).is_err());
    assert!(backend.decisions.is_empty());
}

#[test]
fn creation_requires_confirmation_even_if_a_panel_already_prepared_its_record() {
    let fixture = Fixture::new();
    let mut state = fixture.ready();
    state.operation = CreateState::Prepared;
    state.worker = None;
    fixture.save(&state);
    let op = fixture.submit(Action::EnsureReady);
    let mut backend = Fake::new(&fixture);
    assert_eq!(
        execute_with(&fixture.request(), op.intent.operation_id, &mut backend)
            .unwrap()
            .phase,
        Phase::ConfirmationRequired
    );
    assert!(backend.decisions.is_empty());
}

#[test]
fn a_hetzner_resume_interrupted_after_its_fence_was_cleared_reconnects_without_confirmation() {
    let fixture = Fixture::new();
    let mut state = fixture.ready();
    state.profile.provider = horizon_cloud::hetzner::PROVIDER.into();
    state.operation = CreateState::Prepared;
    state.worker = None;
    fixture.save(&state);
    // A server held the volume before Resume cleared its fence.
    let journal = serde_json::json!({"location": "hel1", "volume": {"state": "bound", "worker_id": "9"}});
    std::fs::write(fixture.root.path().join("target/hetzner.json"), journal.to_string()).unwrap();
    let op = fixture.submit(Action::EnsureReady);
    let mut backend = Fake::new(&fixture);
    execute_with(&fixture.request(), op.intent.operation_id, &mut backend).unwrap();
    assert_eq!(backend.decisions, [Decision::Reconnect]);
    // The same journal beside a RunPod record proves nothing: creation needs confirming.
    let mut runpod = fixture.ready();
    runpod.operation = CreateState::Prepared;
    runpod.worker = None;
    fixture.save(&runpod);
    let op = fixture.submit(Action::EnsureReady);
    let mut backend = Fake::new(&fixture);
    assert_eq!(
        execute_with(&fixture.request(), op.intent.operation_id, &mut backend)
            .unwrap()
            .phase,
        Phase::ConfirmationRequired
    );
    fixture.save(&state);
    // A volume no server has held proves nothing was ever created and approved.
    let journal =
        serde_json::json!({"location": "hel1", "volume": {"state": "bound", "worker_id": "9"}, "unused": true});
    std::fs::write(fixture.root.path().join("target/hetzner.json"), journal.to_string()).unwrap();
    let op = fixture.submit(Action::EnsureReady);
    let mut backend = Fake::new(&fixture);
    assert_eq!(
        execute_with(&fixture.request(), op.intent.operation_id, &mut backend)
            .unwrap()
            .phase,
        Phase::ConfirmationRequired
    );
    assert!(backend.decisions.is_empty());
}

#[test]
fn stopped_records_request_resume_only_after_an_explicit_ensure() {
    let fixture = Fixture::new();
    let mut state = fixture.ready();
    state.stop_requested = true;
    state.stage = Stage::Stopped;
    state.worker.as_mut().unwrap().desired_status = "EXITED".into();
    fixture.save(&state);
    let stop = fixture.submit(Action::Stop);
    let mut backend = Fake::new(&fixture);
    execute_with(&fixture.request(), stop.intent.operation_id, &mut backend).unwrap();
    let ensure = fixture.submit(Action::EnsureReady);
    execute_with(&fixture.request(), ensure.intent.operation_id, &mut backend).unwrap();
    assert_eq!(
        backend.decisions,
        [Decision::AlreadyStopped, Decision::ResumeThenReconnect]
    );
}

#[test]
fn hetzner_deletion_and_missing_storage_journals_fail_before_execution() {
    for journal in [
        Some(r#"{"deleting":true}"#),
        Some(r#"{"volume":{"state":"terminated","worker_id":"42"}}"#),
        None,
    ] {
        let fixture = Fixture::new();
        let mut state = fixture.ready();
        state.profile.provider = "hetzner".into();
        fixture.save(&state);
        if let Some(journal) = journal {
            std::fs::write(fixture.root.path().join("target/hetzner.json"), journal).unwrap();
        }
        let op = fixture.submit(Action::EnsureReady);
        let mut backend = Fake::new(&fixture);
        let result = execute_with(&fixture.request(), op.intent.operation_id, &mut backend);
        if journal.is_some() {
            assert_eq!(result.unwrap().phase, Phase::Refused);
        } else {
            assert!(result.is_err());
            assert!(backend.decisions.is_empty());
        }
    }
    let fixture = Fixture::new();
    std::fs::write(fixture.root.path().join("target/workspace-volume.required"), "").unwrap();
    let op = fixture.submit(Action::EnsureReady);
    let mut backend = Fake::new(&fixture);
    assert!(execute_with(&fixture.request(), op.intent.operation_id, &mut backend).is_err());
    assert!(backend.decisions.is_empty());
}

#[test]
fn a_checked_companion_binds_to_the_checkout_its_deployment_records() {
    let fixture = Fixture::unbound();
    // Without a deployment record, creation needs the owner's confirmation.
    std::fs::remove_file(fixture.root.path().join("target/deployment.json")).unwrap();
    assert!(bind_selected(&fixture.request()).is_err());
    fixture.save(&fixture.ready());
    bind_selected(&fixture.request()).unwrap();
    let state = journal::Store::open(fixture.root.path(), &fixture.owner)
        .unwrap()
        .load()
        .unwrap();
    assert_eq!(state.intents.binding("consumer"), Some(&fixture.binding));
    // Binding again leaves the saved binding as it is.
    bind_selected(&fixture.request()).unwrap();
    assert_eq!(fixture.submit(Action::EnsureReady).phase, Phase::Submitted);
}

#[test]
fn an_unchecked_companion_is_not_bound() {
    let fixture = Fixture::unbound();
    journal::Store::open(fixture.root.path(), &fixture.owner)
        .map(|store| {
            let mut state = store.load().unwrap();
            state.grants.get_mut("consumer").unwrap().selected = false;
            store.save(&state).unwrap();
        })
        .unwrap();
    assert!(bind_selected(&fixture.request()).is_err());
    let state = journal::Store::open(fixture.root.path(), &fixture.owner)
        .unwrap()
        .load()
        .unwrap();
    assert!(state.intents.binding("consumer").is_none());
}

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
