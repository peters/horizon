use super::*;

fn digest(image: char) -> String {
    format!("registry.example/worker@sha256:{}", image.to_string().repeat(64))
}

fn image(image: char) -> ReplacementImage {
    ReplacementImage {
        digest: digest(image),
        registry_auth_id: Some(format!("pull-{image}")),
        registry_generation: Some(format!("generation-{image}")),
    }
}

fn ready() -> Deployment {
    let profile = serde_json::json!({"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8});
    serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"replaced","repository":"/synthetic","revision":"b".repeat(40),
        "profile":profile,"stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},
        "spec":{
            "operation_id":"replaced","image_digest":digest('a'),"profile":profile,"public_key":"unused",
            "registry_auth_id":"pull-a","gpu_types":[],"cpu_flavors":["cpu3c"],"data_centers":[]
        },
        "registry_generation":"generation-a","worker":null,"sessions":[]
    }))
    .unwrap()
}

fn begun() -> (Deployment, OperationId) {
    let mut state = ready();
    let operation = OperationId::generate();
    state.begin_replacement(operation, "c".repeat(40)).unwrap();
    (state, operation)
}

fn requested() -> (Deployment, OperationId) {
    let (mut state, operation) = begun();
    state.replacement_built(image('b')).unwrap();
    assert_eq!(state.request_replacement().unwrap(), Stage::Ready);
    (state, operation)
}

#[test]
fn replacement_journal_and_barrier_stage_round_trip() {
    let (prepared, _) = begun();
    let mut built = prepared.clone();
    built.replacement_built(image('b')).unwrap();
    let (requested, operation) = requested();
    let mut committed = requested.clone();
    committed.commit_replacement().unwrap();
    for state in [prepared, built, requested.clone(), committed] {
        let value = serde_json::to_value(&state).unwrap();
        let restored: Deployment = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(restored.image_replacement, state.image_replacement);
        assert_eq!(restored.session_restart, state.session_restart);
        assert_eq!(restored.stage, state.stage);
        assert_eq!(serde_json::to_value(&restored).unwrap(), value);
    }
    let value = serde_json::to_value(&requested).unwrap();
    assert_eq!(value["stage"], "Replace");
    assert_eq!(value["image_replacement"]["operation"], operation.to_string());
    assert_eq!(
        value["image_replacement"]["phase"],
        serde_json::json!({
            "state":"requested","digest":digest('b'),"registry_auth_id":"pull-b",
            "registry_generation":"generation-b"
        })
    );
    assert_eq!(
        serde_json::from_value::<Stage>(serde_json::json!("Replace")).unwrap(),
        Stage::Replace
    );
    assert!(!Stage::ALL.contains(&Stage::Replace));
}

#[test]
fn replacement_journal_rejects_unknown_fields() {
    let (prepared, _) = begun();
    let (requested, _) = requested();
    for (state, pointer) in [
        (&prepared, "/image_replacement"),
        (&prepared, "/image_replacement/phase"),
        (&requested, "/image_replacement/phase"),
    ] {
        let mut value = serde_json::to_value(state).unwrap();
        value
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unknown_fence".into(), serde_json::json!(true));
        assert!(
            serde_json::from_value::<Deployment>(value).is_err(),
            "accepted {pointer}"
        );
    }
    let mut value = serde_json::to_value(&requested).unwrap();
    value["image_replacement"]["phase"]["state"] = serde_json::json!("applied");
    assert!(serde_json::from_value::<Deployment>(value).is_err());
}

#[test]
fn replacement_commits_only_the_image_binding_and_asks_sessions_to_relaunch() {
    let (mut state, operation) = requested();
    let before = state.spec.clone().unwrap();
    let next = state.replacement_worker().unwrap().unwrap();
    assert!(before.verify_replacement(&next).is_ok());
    state.refuse_replacement(Stage::Push).unwrap();
    assert_eq!(state.stage, Stage::Push);
    assert!(matches!(
        state.image_replacement.as_ref().unwrap().phase,
        ReplacementPhase::Built(_)
    ));
    assert_eq!(state.request_replacement().unwrap(), Stage::Push);
    assert_eq!(state.stage, Stage::Replace);
    assert_eq!(state.commit_replacement().unwrap(), operation);
    let spec = state.spec.as_ref().unwrap();
    assert_eq!(*spec, next);
    assert_eq!(
        (spec.image_digest.as_str(), spec.registry_auth_id.as_deref()),
        (digest('b').as_str(), Some("pull-b"))
    );
    assert_eq!(state.registry_generation.as_deref(), Some("generation-b"));
    assert!(state.image_replacement.is_none());
    assert_eq!(state.session_restart, Some(operation));
    assert_eq!(state.stage, Stage::Readiness);
    assert_eq!(
        state.operation,
        CreateState::Bound {
            worker_id: "worker1".into()
        }
    );
    assert!(state.commit_replacement().is_err());
}

#[test]
fn premature_or_inconsistent_transitions_leave_the_record_unchanged() {
    let mut state = ready();
    for change in [
        (|state: &mut Deployment| state.stage = Stage::Readiness) as fn(&mut Deployment),
        |state| state.stop_requested = true,
        |state| state.operation = CreateState::Requested,
        |state| state.session_restart = Some(OperationId::generate()),
        // Too long for the replacement's image tag.
        |state| state.cloud_id = "c".repeat(90),
    ] {
        let mut refused = ready();
        change(&mut refused);
        let operation = OperationId::generate();
        assert!(refused.begin_replacement(operation, "c".repeat(40)).is_err());
        assert!(refused.image_replacement.is_none());
    }
    assert!(state.begin_replacement(OperationId::generate(), "HEAD".into()).is_err());
    let (mut state, _) = begun();
    assert!(state.request_replacement().is_err());
    assert!(state.commit_replacement().is_err());
    let unchanged = serde_json::to_value(&state).unwrap();
    let mismatched = [
        ReplacementImage {
            digest: digest('a'),
            ..image('b')
        },
        ReplacementImage {
            digest: "registry.example/worker:latest".into(),
            ..image('b')
        },
    ];
    for image in mismatched {
        assert!(state.replacement_built(image).is_err());
        assert_eq!(serde_json::to_value(&state).unwrap(), unchanged);
    }
    assert!(
        state
            .begin_replacement(OperationId::generate(), "c".repeat(40))
            .is_err()
    );
    state.replacement_built(image('b')).unwrap();
    assert!(state.replacement_built(image('c')).is_err());
    assert!(state.refuse_replacement(Stage::Ready).is_err());
    let mut discarded = state.clone();
    discarded.discard_replacement().unwrap();
    assert!(discarded.image_replacement.is_none());
    state.request_replacement().unwrap();
    assert!(state.request_replacement().is_err());
    assert!(state.discard_replacement().is_err());
    assert_eq!(state.stage, Stage::Replace);
}

#[test]
fn journal_must_describe_the_bound_worker_and_its_recorded_image() {
    let (valid, _) = requested();
    for change in [
        (|state: &mut Deployment| state.stage = Stage::Readiness) as fn(&mut Deployment),
        |state| {
            state.operation = CreateState::Bound {
                worker_id: "worker2".into(),
            }
        },
        |state| state.operation = CreateState::Requested,
        |state| state.registry_generation = None,
        |state| state.spec.as_mut().unwrap().image_digest = digest('c'),
        |state| state.spec.as_mut().unwrap().registry_auth_id = None,
        |state| state.image_replacement.as_mut().unwrap().version = 2,
        |state| state.image_replacement.as_mut().unwrap().recipe_revision = "HEAD".into(),
        |state| state.image_replacement.as_mut().unwrap().tag = String::new(),
        |state| state.image_replacement.as_mut().unwrap().tag = "horizon-replaced-other".into(),
        |state| state.image_replacement = None,
    ] {
        let mut state = valid.clone();
        change(&mut state);
        assert!(state.replacement_worker().is_err());
        assert!(state.refuse_unsettled_replacement().is_err());
    }
    let mut terminated = valid;
    terminated.operation = CreateState::Terminated {
        worker_id: "worker1".into(),
    };
    assert_eq!(
        terminated.replacement_worker().unwrap().unwrap().image_digest,
        digest('b')
    );
}

#[test]
fn a_terminated_worker_keeps_its_journal_only_for_deletion() {
    let terminate = |state: &mut Deployment| {
        state.operation = CreateState::Terminated {
            worker_id: "worker1".into(),
        };
    };
    let (mut prepared, _) = begun();
    terminate(&mut prepared);
    let mut built = begun().0;
    built.replacement_built(image('b')).unwrap();
    terminate(&mut built);
    let (mut requested, _) = requested();
    terminate(&mut requested);
    for state in [&prepared, &built, &requested] {
        assert!(state.replacement_worker().is_ok(), "deletion identifies either image");
    }
    let unchanged = |state: &Deployment| serde_json::to_value(state).unwrap();
    let before = unchanged(&prepared);
    assert!(prepared.replacement_built(image('b')).is_err());
    assert_eq!(unchanged(&prepared), before);
    let before = unchanged(&built);
    assert!(built.request_replacement().is_err());
    assert_eq!(unchanged(&built), before);
    let before = unchanged(&requested);
    assert!(requested.refuse_replacement(Stage::Ready).is_err());
    assert!(requested.commit_replacement().is_err());
    assert_eq!(unchanged(&requested), before);
}

#[test]
fn pending_replacements_refuse_actions_that_assume_the_recorded_image() {
    let (prepared, _) = begun();
    let mut built = prepared.clone();
    built.replacement_built(image('b')).unwrap();
    let (requested, _) = requested();
    assert!(ready().refuse_pending_replacement().is_ok());
    assert!(ready().refuse_unsettled_replacement().is_ok());
    for state in [&prepared, &built, &requested] {
        assert!(
            matches!(state.refuse_pending_replacement(), Err(Error::Invalid(message)) if message == REPLACEMENT_PENDING)
        );
    }
    assert!(prepared.refuse_unsettled_replacement().is_ok());
    assert!(built.refuse_unsettled_replacement().is_ok());
    assert!(
        matches!(requested.refuse_unsettled_replacement(), Err(Error::Invalid(message)) if message == REPLACEMENT_PENDING)
    );
}
