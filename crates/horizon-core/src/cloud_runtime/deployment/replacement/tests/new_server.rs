//! Rebuilds on a provider that releases the server and starts a new one
//! (`provider::Rebuild::NewServer`), against a simulated server release.
use super::super::new_server as flow;
use super::*;

/// A Hetzner cloud as `Fixture` records a `RunPod` one: no `RunPod` storage journal,
/// since its volume lives in `hetzner.json`.
fn hetzner() -> Fixture {
    let fixture = Fixture::new();
    std::fs::remove_file(fixture.root.join("workspace-volume.json")).unwrap();
    fixture.edit(|state| {
        state.profile.provider = horizon_cloud::hetzner::PROVIDER.into();
        state.spec.as_mut().unwrap().profile = state.profile.clone();
        state.spec.as_mut().unwrap().registry_auth_id = None;
    });
    fixture
}

/// The server's release as Hetzner records it: `released` once the stop recorded it,
/// `gone` once the server is proven deleted.
#[derive(Default)]
struct Hosting {
    released: Cell<bool>,
    gone: Cell<bool>,
    /// The release fails after it is recorded, as when the delete is interrupted.
    release_fails: Cell<bool>,
    fail_at: Cell<Option<Boundary>>,
    calls: RefCell<Vec<&'static str>>,
}

impl Server for Hosting {
    fn released(&self, _store: &Store, _state: &Deployment) -> Result<bool> {
        Ok(self.released.get())
    }

    fn release(&self, _store: &Store, state: &mut Deployment) -> Result<()> {
        assert_eq!(state.stage, Stage::Replace, "the rebuild keeps its stage");
        assert!(matches!(state.operation, CreateState::Bound { .. }));
        self.calls.borrow_mut().push("release");
        self.released.set(true);
        if self.release_fails.replace(false) {
            return Err(Error::Invalid(CRASH));
        }
        self.gone.set(true);
        Ok(())
    }

    fn reopen(&self, store: &Store, state: &mut Deployment) -> Result<()> {
        assert!(self.gone.get(), "a fence is cleared only for a server proven gone");
        self.calls.borrow_mut().push("reopen");
        state.operation = CreateState::Prepared;
        state.worker = None;
        state.stop_requested = false;
        store.save(state)?;
        self.released.set(false);
        Ok(())
    }

    fn checkpoint(&self, boundary: Boundary) -> Result<()> {
        if self.fail_at.get() == Some(boundary) {
            return Err(Error::Invalid(CRASH));
        }
        Ok(())
    }
}

/// `Script`'s recipe and image pipeline with a server that is released, never switched.
struct NewServer<'a> {
    script: &'a Script,
    hosting: Hosting,
}

impl Provider for NewServer<'_> {
    fn replace(&self, _: &str, _: &WorkerSpec, _: &WorkerSpec) -> Result<()> {
        panic!("a new-server provider never switches a worker in place")
    }

    fn observe(&self, _: &Deployment, _: Option<Instant>) -> Result<Observed> {
        panic!("a new-server provider cannot report a server's image")
    }

    fn pause(&self, _: Instant) -> Result<bool> {
        panic!("nothing is polled")
    }

    fn checkpoint(&self, boundary: Boundary) -> Result<()> {
        self.hosting.checkpoint(boundary)
    }
}

impl Steps for NewServer<'_> {
    fn build(&self, state: &Deployment, revision: &str, siblings: &[String], tag: &str) -> Result<ReplacementImage> {
        let mut image = self.script.build(state, revision, siblings, tag)?;
        // The host logs in to the registry itself, so the recorded credential is kept.
        image.registry_auth_id = None;
        Ok(image)
    }

    fn verify(&self, state: &Deployment, image: &ReplacementImage) -> Result<()> {
        self.script.verify(state, image)
    }

    fn release_devices(&self, _: &Deployment) -> Result<()> {
        panic!("a new-server cloud holds no hosted devices")
    }

    fn server(&self) -> Option<&dyn Server> {
        Some(&self.hosting)
    }
}

fn rebuild_new(fixture: &Fixture, steps: &NewServer<'_>) -> Result<Driven> {
    let store = fixture.store();
    let mut state = store.load().unwrap().unwrap();
    ready(&state)?;
    let recipes = committed_recipes(steps.script, &state, "dev", &|_| {})?;
    begin(steps, &store, &mut state, recipes)?;
    drive(steps, &store, &mut state, &|_| {})
}

fn continue_new(fixture: &Fixture, steps: &NewServer<'_>) -> Result<()> {
    let store = fixture.store();
    let mut state = store.load().unwrap().unwrap();
    drive(steps, &store, &mut state, &|_| {}).map(|_| ())
}

fn cancel_new(fixture: &Fixture, steps: &NewServer<'_>) -> Result<flow::Cancelled> {
    let store = fixture.store();
    let mut state = store.load().unwrap().unwrap();
    flow::cancel(&steps.hosting, &store, &mut state, &|_| {})
}

/// The cloud records the new image on a cleared fence, for the reconnect to start.
fn assert_committed(fixture: &Fixture, steps: &NewServer<'_>) {
    let state = fixture.state();
    assert_eq!(state.spec.as_ref().unwrap().image_digest, steps.script.built);
    assert_eq!(state.operation, CreateState::Prepared);
    assert_eq!(state.stage, Stage::Readiness);
    assert!(state.image_replacement.is_none());
    assert!(
        state.session_restart.is_some(),
        "the sessions relaunch on the new server"
    );
    assert!(!state.stop_requested);
    assert!(
        !steps.hosting.released.get(),
        "the release is cleared once the fence is"
    );
}

#[test]
fn a_new_server_rebuild_releases_the_server_and_commits_the_image_with_the_fence_cleared() {
    let fixture = hetzner();
    let script = Script::new(&fixture);
    let steps = NewServer {
        script: &script,
        hosting: Hosting::default(),
    };
    assert_eq!(rebuild_new(&fixture, &steps).unwrap(), Driven::Committed);
    assert_committed(&fixture, &steps);
    assert_eq!(*steps.hosting.calls.borrow(), ["release", "reopen"]);
    assert_eq!(script.count("build"), 1);
    // The previous image's credential is kept: the host logs in to the registry itself.
    assert_eq!(fixture.state().spec.unwrap().registry_auth_id, None);
}

#[test]
fn an_interrupted_new_server_rebuild_continues_to_the_new_image_and_never_builds_twice() {
    for boundary in [Boundary::Built, Boundary::Requested, Boundary::Released] {
        let fixture = hetzner();
        let script = Script::new(&fixture);
        let steps = NewServer {
            script: &script,
            hosting: Hosting::default(),
        };
        steps.hosting.fail_at.set(Some(boundary));
        assert!(rebuild_new(&fixture, &steps).is_err(), "{boundary:?}");
        steps.hosting.fail_at.set(None);
        continue_new(&fixture, &steps).unwrap();
        assert_committed(&fixture, &steps);
        assert_eq!(script.count("build"), 1, "{boundary:?}");
    }
    // A release interrupted after it was recorded is finished by the continue.
    let fixture = hetzner();
    let script = Script::new(&fixture);
    let steps = NewServer {
        script: &script,
        hosting: Hosting::default(),
    };
    steps.hosting.release_fails.set(true);
    assert!(rebuild_new(&fixture, &steps).is_err());
    let pending = fixture.state();
    assert_eq!(pending.stage, Stage::Replace);
    assert!(matches!(pending.operation, CreateState::Bound { .. }));
    continue_new(&fixture, &steps).unwrap();
    assert_committed(&fixture, &steps);
    assert_eq!(*steps.hosting.calls.borrow(), ["release", "release", "reopen"]);
}

#[test]
fn a_rebuild_cancelled_before_the_release_leaves_the_cloud_as_it_was() {
    let fixture = hetzner();
    let before = fixture.state();
    let script = Script::new(&fixture);
    let steps = NewServer {
        script: &script,
        hosting: Hosting::default(),
    };
    steps.hosting.fail_at.set(Some(Boundary::Requested));
    assert!(rebuild_new(&fixture, &steps).is_err());
    assert!(fixture.state().image_replacement.as_ref().unwrap().requested());
    assert_eq!(cancel_new(&fixture, &steps).unwrap(), flow::Cancelled::Untouched);
    let after = fixture.state();
    assert_eq!(
        serde_json::to_value(&after).unwrap(),
        serde_json::to_value(&before).unwrap(),
        "same worker, image, stage and sessions"
    );
    assert!(steps.hosting.calls.borrow().is_empty(), "the server was never touched");
}

#[test]
fn a_rebuild_cancelled_after_the_release_starts_a_new_server_on_the_previous_image() {
    let fixture = hetzner();
    let before = fixture.state();
    let script = Script::new(&fixture);
    let steps = NewServer {
        script: &script,
        hosting: Hosting::default(),
    };
    steps.hosting.release_fails.set(true);
    assert!(rebuild_new(&fixture, &steps).is_err());
    assert_eq!(cancel_new(&fixture, &steps).unwrap(), flow::Cancelled::Released);
    let after = fixture.state();
    assert_eq!(after.spec, before.spec, "the previous image is kept");
    assert_eq!(after.operation, CreateState::Prepared);
    assert_eq!(after.stage, Stage::Readiness);
    assert!(after.image_replacement.is_none());
    assert!(after.session_restart.is_some());
    assert!(!steps.hosting.released.get());
    // A cancel interrupted before the fence was cleared is repeated safely.
    let fixture = hetzner();
    let script = Script::new(&fixture);
    let steps = NewServer {
        script: &script,
        hosting: Hosting::default(),
    };
    steps.hosting.fail_at.set(Some(Boundary::Released));
    assert!(rebuild_new(&fixture, &steps).is_err());
    assert!(cancel_new(&fixture, &steps).is_err());
    steps.hosting.fail_at.set(None);
    assert_eq!(cancel_new(&fixture, &steps).unwrap(), flow::Cancelled::Released);
    assert_eq!(fixture.state().spec, before.spec);
}

#[test]
fn an_interrupted_release_is_never_reconnected_around_or_read_as_a_stop() {
    use crate::cloud_runtime::{deployment::Compute as _, settings::Settings, state::REPLACEMENT_PENDING};
    let fixture = hetzner();
    let script = Script::new(&fixture);
    let steps = NewServer {
        script: &script,
        hosting: Hosting::default(),
    };
    steps.hosting.release_fails.set(true);
    assert!(rebuild_new(&fixture, &steps).is_err());
    let token = fixture.root.join("hetzner-token");
    std::fs::write(&token, "synthetic-token").unwrap();
    std::fs::set_permissions(&token, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let settings: Settings = serde_json::from_value(json!({
        "runpod_key_file": fixture.root.join("unused"), "ssh_identity_file": fixture.root.join("unused"),
        "docker_config": fixture.root.join("unused"), "cpu_flavors": [], "gpu_types": [],
        "hetzner": {"token_file": token, "server_types": ["cx23"], "locations": ["hel1"]}
    }))
    .unwrap();
    let store = fixture.store();
    let mut state = store.load().unwrap().unwrap();
    let cancel = Cancellation::default();
    // A reconnect would place a server around the rebuild.
    let compute = crate::cloud_runtime::deployment::hetzner::Compute::new(&settings).unwrap();
    let refused = compute
        .settle(&store, &mut state, &cancel, crate::cloud_runtime::mutation::IGNORE)
        .unwrap_err();
    assert_eq!(refused.to_string(), REPLACEMENT_PENDING);
    // A provider check would record the released server as a stop.
    let refused = crate::cloud_runtime::providers::lifecycle(&state, &settings)
        .check(&store, &mut state, None, &cancel)
        .unwrap_err();
    assert_eq!(refused.to_string(), REPLACEMENT_PENDING);
    drop(store);
    let kept = fixture.state();
    assert_eq!(kept.stage, Stage::Replace);
    assert!(kept.image_replacement.as_ref().unwrap().requested());
}

#[test]
fn a_mismatched_journal_is_refused_before_the_server_is_touched() {
    let fixture = hetzner();
    let script = Script::new(&fixture);
    let steps = NewServer {
        script: &script,
        hosting: Hosting::default(),
    };
    steps.hosting.fail_at.set(Some(Boundary::Requested));
    assert!(rebuild_new(&fixture, &steps).is_err());
    // A journal for another worker, as a stale or foreign record would carry.
    fixture.edit(|state| state.image_replacement.as_mut().unwrap().worker_id = "worker9".into());
    assert!(continue_new(&fixture, &steps).is_err());
    // As `continue_replacement` calls it for a requested journal, without `drive`.
    let store = fixture.store();
    let mut state = store.load().unwrap().unwrap();
    assert!(flow::switch(&steps.hosting, &store, &mut state, &|_| {}).is_err());
    drop(store);
    assert!(cancel_new(&fixture, &steps).is_err());
    assert!(steps.hosting.calls.borrow().is_empty(), "the server was never released");
}
