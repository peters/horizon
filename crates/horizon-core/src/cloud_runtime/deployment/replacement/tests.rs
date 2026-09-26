use super::*;
use crate::cloud_runtime::state::REPLACEMENT_PENDING;
use horizon_cloud::Reason;
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    path::PathBuf,
};

const CRASH: &str = "simulated crash";

fn digest(image: char) -> String {
    format!("registry.example/worker@sha256:{}", image.to_string().repeat(64))
}

fn config(profile: &Profile) -> CloudConfig {
    CloudConfig {
        version: 1,
        default: "dev".into(),
        profiles: [("dev".into(), profile.clone())].into(),
        companions: std::collections::BTreeMap::new(),
    }
}

/// A ready cloud bound to `worker1` on image `a`, with a CPU storage journal.
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        Self::with(&json!({}))
    }

    fn with(capabilities: &serde_json::Value) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("cloud");
        let profile = json!({
            "provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,
            "build":{"context":".","dockerfile":"Dockerfile"},"capabilities":capabilities
        });
        let spec = json!({
            "operation_id":"rebuilt","image_digest":digest('a'),"profile":profile,"public_key":"unused",
            "registry_auth_id":"pull-a","gpu_types":[],"cpu_flavors":["cpu3c"],"data_centers":[]
        });
        let state: Deployment = serde_json::from_value(json!({
            "version":1,"cloud_id":"rebuilt","repository":"/synthetic","revision":"a".repeat(40),
            "profile":profile,"stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},
            "spec":spec,"registry_generation":"generation-a","worker":null,"source_ready":true,
            "ready_history":"Observed","sessions":[{
                "panel_id":"agent1","agent":"codex","tmux":"agent1","branch":"agent/agent1",
                "worktree":"/workspace/agents/agent1"
            }]
        }))
        .unwrap();
        Store::lock(&root).unwrap().save(&state).unwrap();
        let journal = json!({
            "version":1,"worker":spec,"state":{"state":"prepared"},
            "spec":{"operation_id":"rebuilt","size":20,"data_center_id":"EU-TEST-1"}
        });
        std::fs::write(root.join("workspace-volume.json"), journal.to_string()).unwrap();
        Self { _temp: temp, root }
    }

    fn store(&self) -> Store {
        Store::lock(&self.root).unwrap()
    }

    fn state(&self) -> Deployment {
        self.store().load().unwrap().unwrap()
    }

    fn edit(&self, change: impl FnOnce(&mut Deployment)) {
        let store = self.store();
        let mut state = store.load().unwrap().unwrap();
        change(&mut state);
        store.save(&state).unwrap();
    }

    /// The image the storage journal records its worker with.
    fn storage_image(&self) -> String {
        let journal: serde_json::Value =
            serde_json::from_slice(&std::fs::read(self.root.join("workspace-volume.json")).unwrap()).unwrap();
        journal["worker"]["image_digest"].as_str().unwrap().into()
    }
}

/// A scripted provider and image pipeline around a simulated worker image.
struct Script {
    world: Cell<Observed>,
    /// Reported before the world's image, such as provider APIs that still disagree.
    observations: RefCell<VecDeque<Result<Observed>>>,
    /// Outcome of each update: whether it applies, and the error it returns.
    updates: RefCell<VecDeque<(bool, Option<CloudError>)>>,
    head: Head,
    built: String,
    fail_at: Cell<Option<Boundary>>,
    calls: RefCell<Vec<String>>,
    pauses: Cell<u32>,
}

impl Script {
    fn new(fixture: &Fixture) -> Self {
        Self {
            world: Cell::new(Observed::Previous),
            observations: RefCell::default(),
            updates: RefCell::default(),
            head: Head {
                revision: "c".repeat(40),
                config: Some(config(&fixture.state().profile)),
            },
            built: digest('b'),
            fail_at: Cell::new(None),
            calls: RefCell::default(),
            pauses: Cell::new(0),
        }
    }

    fn log(&self, call: impl Into<String>) {
        self.calls.borrow_mut().push(call.into());
    }

    fn count(&self, call: &str) -> usize {
        self.calls.borrow().iter().filter(|logged| *logged == call).count()
    }
}

impl Provider for Script {
    fn replace(&self, worker_id: &str, from: &WorkerSpec, to: &WorkerSpec) -> Result<()> {
        assert_eq!(worker_id, "worker1");
        from.verify_replacement(to).unwrap();
        let target = if to.image_digest == self.built {
            Observed::Next
        } else {
            Observed::Previous
        };
        self.log(format!("replace {target:?}"));
        let (applies, error) = self.updates.borrow_mut().pop_front().unwrap_or((true, None));
        if applies {
            self.world.set(target);
        }
        error.map_or(Ok(()), |error| Err(error.into()))
    }

    fn observe(&self, state: &Deployment, _deadline: Option<Instant>) -> Result<Observed> {
        pair(state)?;
        self.log("observe");
        let scripted = self.observations.borrow_mut().pop_front();
        scripted.unwrap_or_else(|| Ok(self.world.get()))
    }

    fn pause(&self, _deadline: Instant) -> Result<bool> {
        self.pauses.set(self.pauses.get() + 1);
        Ok(self.pauses.get() < 4)
    }

    fn checkpoint(&self, boundary: Boundary) -> Result<()> {
        if self.fail_at.get() == Some(boundary) {
            return Err(Error::Invalid(CRASH));
        }
        Ok(())
    }
}

impl Recipe for Script {
    fn head(&self, repository: &Path) -> Result<Head> {
        assert_eq!(repository, Path::new("/synthetic"));
        self.log("head");
        Ok(self.head.clone())
    }
}

impl Steps for Script {
    fn build(&self, _state: &Deployment, revision: &str, tag: &str) -> Result<ReplacementImage> {
        assert_eq!(revision, self.head.revision);
        assert!(tag.starts_with("horizon-rebuilt-"));
        self.log("build");
        Ok(ReplacementImage {
            digest: self.built.clone(),
            registry_auth_id: Some("pull-b".into()),
            registry_generation: Some("generation-b".into()),
        })
    }

    fn verify(&self, _state: &Deployment, image: &ReplacementImage) -> Result<()> {
        assert_eq!(image.digest, self.built);
        self.log("verify");
        Ok(())
    }

    fn release_devices(&self, state: &Deployment) -> Result<()> {
        assert!(!state.image_replacement.as_ref().unwrap().requested());
        self.log("release");
        Ok(())
    }
}

/// The locked part of `rebuild`.
fn rebuild_with(fixture: &Fixture, script: &Script) -> Result<Driven> {
    let store = fixture.store();
    let mut state = store.load().unwrap().unwrap();
    ready(&state)?;
    let revision = committed_revision(script, &state, "dev", &|_| {})?;
    begin(script, &store, &mut state, revision)?;
    drive(script, &store, &mut state, &|_| {})
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Recovery {
    Continue,
    Cancel,
    Settle,
}

/// The locked part of each recovery; `Settle` is what a reconnect or provider check runs.
fn recover(fixture: &Fixture, script: &Script, recovery: Recovery) -> Result<()> {
    let store = fixture.store();
    let mut state = store.load().unwrap().unwrap();
    let requested = state
        .image_replacement
        .as_ref()
        .is_some_and(ImageReplacement::requested);
    match recovery {
        Recovery::Continue => drive(script, &store, &mut state, &|_| {}).map(|_| ()),
        Recovery::Cancel if requested => revert(script, &store, &mut state, &|_| {}),
        Recovery::Cancel => discard(&store, &mut state, &|_| {}),
        Recovery::Settle => settle_with(script, &store, &mut state),
    }
}

fn stages(events: &RefCell<Vec<Stage>>) -> impl Fn(Event) + '_ {
    move |event| {
        if let Event::Stage(stage, _) = event {
            events.borrow_mut().push(stage);
        }
    }
}

#[test]
fn rebuild_switches_the_worker_once_and_asks_its_sessions_to_relaunch() {
    let fixture = Fixture::new();
    let script = Script::new(&fixture);
    script
        .observations
        .borrow_mut()
        .extend([Ok(Observed::Unsettled), Err(CloudError::Transport.into())]);
    let store = fixture.store();
    let mut state = store.load().unwrap().unwrap();
    let events = RefCell::new(Vec::new());
    let revision = committed_revision(&script, &state, "dev", &stages(&events)).unwrap();
    begin(&script, &store, &mut state, revision).unwrap();
    let journal = state.image_replacement.clone().unwrap();
    assert_eq!(journal.recipe_revision, "c".repeat(40));
    assert_eq!(
        journal.tag,
        format!("horizon-rebuilt-{}", uuid::Uuid::from(journal.operation).simple())
    );
    assert_eq!(
        state.stage,
        Stage::Ready,
        "the stage changes only once the update may be sent"
    );
    let driven = drive(&script, &store, &mut state, &stages(&events)).unwrap();
    assert_eq!(driven, Driven::Committed);
    assert_eq!(
        *script.calls.borrow(),
        ["head", "build", "replace Next", "observe", "observe", "observe"]
    );
    assert_eq!(*events.borrow(), [Stage::Replace]);
    let saved = store.load().unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&saved).unwrap(),
        serde_json::to_value(&state).unwrap()
    );
    let spec = saved.spec.unwrap();
    assert_eq!(
        (spec.image_digest.as_str(), spec.registry_auth_id.as_deref()),
        (digest('b').as_str(), Some("pull-b"))
    );
    assert_eq!(saved.registry_generation.as_deref(), Some("generation-b"));
    assert!(saved.image_replacement.is_none());
    assert_eq!(saved.session_restart, Some(journal.operation));
    assert_eq!(saved.stage, Stage::Readiness);
    assert_eq!(
        saved.operation,
        CreateState::Bound {
            worker_id: "worker1".into()
        }
    );
    assert_eq!(fixture.storage_image(), digest('b'));
}

#[test]
fn every_crash_boundary_recovers_consistently_without_switching_twice() {
    use Boundary::{Begun, Built, Observed as Seen, Patched, Rebound, Requested};
    for boundary in [Begun, Built, Requested, Patched, Seen, Rebound] {
        for recovery in [Recovery::Continue, Recovery::Cancel, Recovery::Settle] {
            let context = format!("{boundary:?} then {recovery:?}");
            let fixture = Fixture::new();
            let script = Script::new(&fixture);
            script.fail_at.set(Some(boundary));
            let crashed = rebuild_with(&fixture, &script);
            assert!(matches!(crashed, Err(Error::Invalid(CRASH))), "{context}");
            script.fail_at.set(None);
            let sent = matches!(boundary, Patched | Seen | Rebound);
            let result = recover(&fixture, &script, recovery);
            let state = fixture.state();
            state.replacement_worker().expect(&context);
            let committed = state.spec.as_ref().unwrap().image_digest == digest('b');
            match (recovery, sent) {
                (Recovery::Continue, _) | (Recovery::Settle, true) => {
                    result.expect(&context);
                    assert!(committed && state.image_replacement.is_none(), "{context}");
                }
                (Recovery::Cancel, _) => {
                    result.expect(&context);
                    assert!(!committed && state.image_replacement.is_none(), "{context}");
                    let requested = !matches!(boundary, Begun | Built);
                    assert_eq!(state.session_restart.is_some(), requested, "{context}");
                    assert_eq!(state.stage == Stage::Readiness, requested, "{context}");
                }
                (Recovery::Settle, false) => {
                    let pending = matches!(&result, Err(Error::Invalid(message)) if *message == REPLACEMENT_PENDING);
                    assert_eq!(pending, boundary == Requested, "{context}");
                    assert!(!committed && state.image_replacement.is_some(), "{context}");
                }
            }
            let recorded = &state.spec.as_ref().unwrap().image_digest;
            assert_eq!(fixture.storage_image(), *recorded, "{context}");
            assert_eq!(
                script.count("replace Next"),
                usize::from(committed || sent),
                "{context}"
            );
            // Once the update may have been sent, cancelling always sends the reverse one.
            let reverted = usize::from(recovery == Recovery::Cancel && !matches!(boundary, Begun | Built));
            assert_eq!(script.count("replace Previous"), reverted, "{context}");
            if state.image_replacement.is_none() {
                // A committed or cancelled replacement has nothing left to continue or cancel.
                let calls = script.calls.borrow().len();
                for again in [Recovery::Continue, Recovery::Cancel] {
                    let error = recover(&fixture, &script, again).unwrap_err();
                    assert!(matches!(error, Error::Invalid(NOTHING_PENDING)), "{context}");
                }
                assert_eq!(script.calls.borrow().len(), calls, "{context}");
            }
        }
    }
}

#[test]
fn an_interrupted_cancel_switches_back_again_and_finishes() {
    let fixture = Fixture::new();
    let script = Script::new(&fixture);
    script.fail_at.set(Some(Boundary::Patched));
    assert!(rebuild_with(&fixture, &script).is_err());
    script.fail_at.set(Some(Boundary::Reverted));
    let interrupted = recover(&fixture, &script, Recovery::Cancel);
    assert!(matches!(interrupted, Err(Error::Invalid(CRASH))));
    assert!(fixture.state().image_replacement.unwrap().requested());
    assert!(recover(&fixture, &script, Recovery::Settle).is_err());
    script.fail_at.set(None);
    recover(&fixture, &script, Recovery::Cancel).unwrap();
    let state = fixture.state();
    assert!(state.image_replacement.is_none());
    assert_eq!(state.spec.unwrap().image_digest, digest('a'));
    // The reverse update targets the recorded image, so sending it again is safe.
    let switches = (script.count("replace Next"), script.count("replace Previous"));
    assert_eq!(switches, (1, 2));
}

#[test]
fn a_definite_refusal_returns_to_built_and_an_uncertain_update_stays_requested() {
    let cases: [(fn() -> CloudError, bool); 6] = [
        (|| CloudError::Rejected(Reason::default()), false),
        (|| CloudError::Unauthorized, false),
        (|| CloudError::Cancelled, false),
        (|| CloudError::Http(409, Reason::default()), false),
        (|| CloudError::Transport, true),
        (|| CloudError::Http(503, Reason::default()), true),
    ];
    for (error, uncertain) in cases {
        let fixture = Fixture::new();
        let script = Script::new(&fixture);
        // The update does not apply, so the worker keeps reporting its previous image.
        script.updates.borrow_mut().push_back((false, Some(error())));
        let result = rebuild_with(&fixture, &script);
        let state = fixture.state();
        let journal = state.image_replacement.clone().unwrap();
        let context = error().to_string();
        if uncertain {
            assert!(matches!(result, Err(Error::Invalid(UNSETTLED))), "{context}");
            assert!(journal.requested());
            assert_eq!(state.stage, Stage::Replace);
        } else {
            assert!(matches!(result, Err(Error::Provider(_))), "{context}");
            assert!(matches!(journal.phase, ReplacementPhase::Built(_)));
            assert_eq!(state.stage, Stage::Ready);
            assert_eq!(script.count("observe"), 0);
        }
        script.pauses.set(0);
        recover(&fixture, &script, Recovery::Continue).unwrap();
        assert_eq!(fixture.state().spec.unwrap().image_digest, digest('b'));
        assert_eq!(script.count("replace Next"), 2, "{context}");
        assert_eq!(script.count("verify"), usize::from(!uncertain), "{context}");
    }
}

#[test]
fn a_third_image_or_lost_worker_fails_closed_and_keeps_the_update_requested() {
    // A permanent client error ends the wait at once instead of retrying it.
    let failures: [fn() -> CloudError; 3] = [
        || CloudError::IdentityMismatch,
        || CloudError::WorkerLost,
        || CloudError::Http(405, Reason::default()),
    ];
    for failure in failures {
        let fixture = Fixture::new();
        let script = Script::new(&fixture);
        script.observations.borrow_mut().push_back(Err(failure().into()));
        let context = failure().to_string();
        let result = rebuild_with(&fixture, &script);
        assert!(matches!(result, Err(Error::Provider(_))), "{context}");
        let state = fixture.state();
        assert!(state.image_replacement.as_ref().unwrap().requested());
        assert_eq!(state.spec.as_ref().unwrap().image_digest, digest('a'));
        // A reconnect settles only on the new image; it never assumes either image.
        for observed in [Err(failure().into()), Ok(Observed::Unsettled), Ok(Observed::Previous)] {
            script.observations.borrow_mut().push_back(observed);
            assert!(recover(&fixture, &script, Recovery::Settle).is_err(), "{context}");
            assert_eq!(fixture.state().image_replacement, state.image_replacement);
        }
        recover(&fixture, &script, Recovery::Settle).unwrap();
        assert_eq!(fixture.state().spec.unwrap().image_digest, digest('b'));
        assert_eq!(script.count("replace Next"), 1);
    }
}

#[test]
fn a_changed_or_missing_committed_profile_refuses_before_anything_is_journaled() {
    let fixture = Fixture::new();
    let bound = fixture.state().profile;
    let mut renamed = config(&bound);
    renamed.profiles = [("other".into(), bound.clone())].into();
    let changed = |change: fn(&mut Profile)| {
        let mut head = bound.clone();
        change(&mut head);
        Some(config(&head))
    };
    let heads = [
        (None, NO_CONFIG),
        (Some(renamed), NO_PROFILE),
        (changed(|profile| profile.gpu = true), "size"),
        (changed(|profile| profile.storage.volume_gb = 40), "size"),
        (changed(|profile| profile.capabilities.desktop = true), "capabilities"),
        (
            changed(|profile| profile.image = "registry.example/other".into()),
            "image repository",
        ),
        (
            changed(|profile| profile.build.as_mut().unwrap().dockerfile = "Other".into()),
            "build section",
        ),
        (
            changed(|profile| profile.bootstrap.readiness_seconds = 900),
            "bootstrap",
        ),
    ];
    for (head, reason) in heads {
        let mut script = Script::new(&fixture);
        script.head.config = head;
        let error = rebuild_with(&fixture, &script).unwrap_err().to_string();
        assert!(error.contains(reason), "{error}");
        assert!(fixture.state().image_replacement.is_none());
        assert_eq!(*script.calls.borrow(), ["head"]);
    }
}

#[test]
fn only_a_ready_bound_cloud_with_a_recipe_and_nothing_pending_can_rebuild() {
    for change in [
        (|state: &mut Deployment| state.stage = Stage::Readiness) as fn(&mut Deployment),
        |state| state.stop_requested = true,
        |state| state.session_restart = Some(OperationId::generate()),
        |state| state.operation = CreateState::Requested,
        |state| state.profile.build = None,
        |state| {
            state
                .begin_replacement(OperationId::generate(), "c".repeat(40))
                .unwrap();
        },
    ] {
        let fixture = Fixture::new();
        fixture.edit(change);
        let before = fixture.state();
        let script = Script::new(&fixture);
        assert!(rebuild_with(&fixture, &script).is_err());
        assert!(script.calls.borrow().is_empty());
        assert_eq!(fixture.state().image_replacement, before.image_replacement);
    }
    // Continuing checks the whole journal before building anything.
    let fixture = Fixture::new();
    fixture.edit(|state| {
        state
            .begin_replacement(OperationId::generate(), "c".repeat(40))
            .unwrap();
        state.image_replacement.as_mut().unwrap().recipe_revision = "HEAD".into();
    });
    let script = Script::new(&fixture);
    assert!(recover(&fixture, &script, Recovery::Continue).is_err());
    assert!(script.calls.borrow().is_empty());
}

#[test]
fn an_unchanged_image_is_reported_without_switching_the_worker() {
    let fixture = Fixture::new();
    let mut script = Script::new(&fixture);
    script.built = digest('a');
    assert_eq!(rebuild_with(&fixture, &script).unwrap(), Driven::Unchanged);
    let state = fixture.state();
    assert!(state.image_replacement.is_none() && state.session_restart.is_none());
    assert_eq!(state.stage, Stage::Ready);
    assert_eq!(state.spec.unwrap().image_digest, digest('a'));
    assert_eq!(*script.calls.borrow(), ["head", "build"]);
}

#[test]
fn hosted_devices_are_released_before_the_update_may_be_sent() {
    let fixture = Fixture::with(&json!({"browserstack":{"targets":["phone"]}}));
    fixture.edit(|state| state.browserstack_released = false);
    let script = Script::new(&fixture);
    assert_eq!(rebuild_with(&fixture, &script).unwrap(), Driven::Committed);
    assert_eq!(script.calls.borrow()[..4], ["head", "build", "release", "replace Next"]);
    assert!(fixture.state().browserstack_released);
}

#[test]
fn cancelling_a_refused_switch_after_the_device_release_reconnects() {
    let fixture = Fixture::with(&json!({"browserstack":{"targets":["phone"]}}));
    fixture.edit(|state| state.browserstack_released = false);
    let script = Script::new(&fixture);
    script
        .updates
        .borrow_mut()
        .push_back((false, Some(CloudError::Unauthorized)));
    assert!(rebuild_with(&fixture, &script).is_err());
    // Also when a crash lost the record of the release.
    fixture.edit(|state| state.browserstack_released = false);
    assert!(may_have_released_devices(&fixture.state()));
    recover(&fixture, &script, Recovery::Cancel).unwrap();
    assert!(fixture.state().image_replacement.is_none());
    let unbuilt = Fixture::with(&json!({"browserstack":{"targets":["phone"]}}));
    let script = Script::new(&unbuilt);
    script.fail_at.set(Some(Boundary::Begun));
    assert!(rebuild_with(&unbuilt, &script).is_err());
    assert!(!may_have_released_devices(&unbuilt.state()));
    assert!(!may_have_released_devices(&Fixture::new().state()));
}

#[test]
fn a_foreign_worker_spec_is_refused_before_the_provider_is_read() {
    let fixture = Fixture::new();
    let script = Script::new(&fixture);
    script.fail_at.set(Some(Boundary::Requested));
    assert!(rebuild_with(&fixture, &script).is_err());
    fixture.edit(|state| state.spec.as_mut().unwrap().operation_id = "foreign".into());
    let calls = script.calls.borrow().len();
    let error = recover(&fixture, &script, Recovery::Settle).unwrap_err().to_string();
    assert!(error.contains("identities differ"), "{error}");
    fixture.edit(|state| state.spec = None);
    let error = recover(&fixture, &script, Recovery::Settle).unwrap_err().to_string();
    assert!(error.contains("Missing worker specification"), "{error}");
    assert_eq!(script.calls.borrow().len(), calls, "no provider read");
}

#[test]
fn a_cpu_cloud_sized_at_creation_rebuilds_from_its_committed_default_size() {
    let fixture = Fixture::new();
    let mut head = fixture.state().profile;
    (head.cpu, head.memory_gb) = (head.cpu * 2, head.memory_gb * 2);
    let mut script = Script::new(&fixture);
    script.head.config = Some(config(&head));
    assert_eq!(rebuild_with(&fixture, &script).unwrap(), Driven::Committed);
}

#[test]
fn a_switch_that_adds_a_registry_credential_is_refused_before_it_is_sent() {
    let fixture = Fixture::new();
    fixture.edit(|state| state.spec.as_mut().unwrap().registry_auth_id = None);
    let script = Script::new(&fixture);
    let error = rebuild_with(&fixture, &script).unwrap_err().to_string();
    assert!(error.contains("registry credential"), "{error}");
    assert!(fixture.state().image_replacement.is_none());
    assert_eq!(*script.calls.borrow(), ["head", "build"]);
}

#[test]
fn sessions_relaunch_in_place_and_lost_ones_are_reported() {
    let fixture = Fixture::new();
    fixture.edit(|state| {
        for (id, agent) in [("agent2", "shell"), ("agent3", "claude")] {
            state.sessions.push(Session {
                panel_id: id.into(),
                agent: agent.into(),
                tmux: id.into(),
                branch: format!("agent/{id}"),
                worktree: format!("/workspace/agents/{id}"),
            });
        }
    });
    let operation = OperationId::generate();
    let restart = WorkerContract {
        session_restart: true,
        ..WorkerContract::default()
    };
    for (contract, statuses, lost) in [
        (restart.clone(), vec!["0", "5", "3"], Some(vec!["agent3"])),
        (restart.clone(), vec!["4", "0", "0"], Some(vec!["agent1"])),
        (restart.clone(), vec!["0", "1"], None),
        (restart.clone(), vec!["0", ""], None),
        (
            WorkerContract::default(),
            vec![],
            Some(vec!["agent1", "agent2", "agent3"]),
        ),
    ] {
        fixture.edit(|state| state.session_restart = Some(operation));
        let store = fixture.store();
        let mut state = store.load().unwrap().unwrap();
        let reported = RefCell::new(Vec::new());
        let mut commands = Vec::new();
        let result = relaunch_sessions(
            &store,
            &mut state,
            &contract,
            &|event| {
                if let Event::Output(line) = event {
                    reported.borrow_mut().push(line.split(' ').nth(1).unwrap().to_owned());
                }
            },
            |command| {
                let status = statuses[commands.len()];
                commands.push(command.to_owned());
                let marker = if status.is_empty() {
                    String::new()
                } else {
                    format!("{RELAUNCH_STATUS}{status}")
                };
                Ok(format!("Session output\n{marker}\n"))
            },
        );
        let context = format!("{statuses:?}");
        assert_eq!(result.is_ok(), lost.is_some(), "{context}");
        assert_eq!(commands.len(), statuses.len(), "{context}");
        let saved = store.load().unwrap().unwrap();
        assert_eq!(saved.session_restart.is_none(), lost.is_some(), "{context}");
        if let Some(lost) = lost {
            assert_eq!(*reported.borrow(), lost, "{context}");
        }
        if contract.session_restart {
            let expected = format!(
                "horizon-worker-session --relaunch {operation} agent1 codex {}; printf '\\nhorizon-relaunch-status=%s\\n' \"$?\"",
                "a".repeat(40)
            );
            assert_eq!(commands[0], expected);
        }
    }
    // Nothing runs without a request, and an invalid identity never reaches the shell.
    let store = fixture.store();
    let mut state = store.load().unwrap().unwrap();
    relaunch_sessions(&store, &mut state, &restart, &|_| {}, |_| panic!("no request")).unwrap();
    state.session_restart = Some(operation);
    state.sessions[0].panel_id = "agent1; reboot".into();
    assert!(relaunch_sessions(&store, &mut state, &restart, &|_| {}, |_| panic!("invalid")).is_err());
}

#[test]
fn provider_reads_never_wait_past_the_deadline() {
    let now = Instant::now();
    let unbounded = live::observe_timeout(None, now).unwrap();
    let late = now + unbounded + Duration::from_secs(60);
    assert_eq!(live::observe_timeout(Some(late), now), Some(unbounded));
    let soon = now + Duration::from_secs(10);
    assert_eq!(live::observe_timeout(Some(soon), now), Some(Duration::from_secs(10)));
    assert_eq!(live::observe_timeout(Some(now), now), None);
}
