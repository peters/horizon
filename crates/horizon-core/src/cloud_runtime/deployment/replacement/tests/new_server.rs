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

    fn unpacked_bytes(&self, state: &Deployment, tag: &str, digest: &str) -> Result<Option<u64>> {
        self.script.unpacked_bytes(state, tag, digest)
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
    let recipes = latest_recipes(steps.script, &state, "dev", &|_| {})?;
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
fn a_hetzner_cloud_whose_committed_profile_names_runpod_rebuilds_on_hetzner_at_the_same_commit() {
    let fixture = hetzner();
    let before = fixture.state();
    let mut head = before.profile.clone();
    head.provider = "runpod".into();
    let mut script = Script::new(&fixture);
    script.head.revision.clone_from(&before.revision);
    script.head.config = Some(config(&head));
    let steps = NewServer {
        script: &script,
        hosting: Hosting::default(),
    };
    assert_eq!(rebuild_new(&fixture, &steps).unwrap(), Driven::Committed);
    assert_committed(&fixture, &steps);
    // Hetzner's own rebuild: the server is released, never switched in place.
    assert_eq!(*steps.hosting.calls.borrow(), ["release", "reopen"]);
    let after = fixture.state();
    assert_eq!(after.profile, before.profile, "the cloud stays on Hetzner");
    assert_eq!(after.spec.as_ref().unwrap().profile, before.profile);
    assert_eq!(after.cloud_id, before.cloud_id);
}

#[test]
fn a_hetzner_cloud_with_storage_chosen_in_new_cloud_rebuilds_on_the_same_volume() {
    let fixture = hetzner();
    // New cloud's Hetzner storage: a system disk requirement and a workspace size of its
    // own; the volume type is always standard there.
    fixture.bind_storage(|storage| (storage.container_gb, storage.volume_gb) = (30, 10));
    let before = fixture.state();
    let mut head = before.profile.clone();
    head.provider = "runpod".into();
    head.storage = horizon_cloud::Storage {
        container_gb: 20,
        volume_gb: 40,
        volume_tier: horizon_cloud::runpod::volumes::Tier::HighPerformance,
    };
    let mut script = Script::new(&fixture);
    script.head.revision.clone_from(&before.revision);
    script.head.config = Some(config(&head));
    let steps = NewServer {
        script: &script,
        hosting: Hosting::default(),
    };
    assert_eq!(rebuild_new(&fixture, &steps).unwrap(), Driven::Committed);
    assert_committed(&fixture, &steps);
    assert_eq!(*steps.hosting.calls.borrow(), ["release", "reopen"]);
    let after = fixture.state();
    assert_eq!(after.profile, before.profile, "the cloud keeps its storage");
    assert_eq!(after.spec.as_ref().unwrap().profile, before.profile);
}

#[test]
fn an_image_larger_than_the_system_disk_requirement_is_refused_before_the_server_is_released() {
    let fixture = hetzner();
    let before = fixture.state();
    let mut script = Script::new(&fixture);
    let disk = u64::from(before.profile.storage.container_gb) * super::super::compatibility::BYTES_PER_GB;
    script.unpacked = Some(disk + 1);
    let steps = NewServer {
        script: &script,
        hosting: Hosting::default(),
    };
    let error = rebuild_new(&fixture, &steps).unwrap_err().to_string();
    assert_eq!(error, super::super::compatibility::IMAGE_TOO_LARGE);
    assert!(steps.hosting.calls.borrow().is_empty(), "the server is never released");
    let after = fixture.state();
    assert!(after.image_replacement.is_none());
    assert_eq!(after.spec, before.spec);
    assert_eq!(after.operation, before.operation);
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
fn a_new_server_binds_changed_settings_with_the_image_and_a_cancel_keeps_the_old_ones() {
    let settings = |fixture: &Fixture| {
        let mut committed = fixture.state().profile;
        committed.capabilities.agents.insert(horizon_cloud::Agent::Codex);
        let mut script = Script::new(fixture);
        script.head.config = Some(config(&committed));
        (committed, script)
    };
    let fixture = hetzner();
    let (committed, script) = settings(&fixture);
    let steps = NewServer {
        script: &script,
        hosting: Hosting::default(),
    };
    assert_eq!(rebuild_new(&fixture, &steps).unwrap(), Driven::Committed);
    assert_committed(&fixture, &steps);
    // The reconnect starts the new server from this specification, environment included.
    let state = fixture.state();
    assert_eq!(state.profile, committed);
    assert_eq!(state.spec.unwrap().profile, committed);
    // Cancelled after the release, the new server starts with the previous settings.
    let fixture = hetzner();
    let before = fixture.state();
    let (_, script) = settings(&fixture);
    let steps = NewServer {
        script: &script,
        hosting: Hosting::default(),
    };
    steps.hosting.release_fails.set(true);
    assert!(rebuild_new(&fixture, &steps).is_err());
    assert_eq!(cancel_new(&fixture, &steps).unwrap(), flow::Cancelled::Released);
    let after = fixture.state();
    assert_eq!(after.profile, before.profile);
    assert_eq!(after.spec, before.spec);
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

/// An Azure cloud as `Fixture` records a `RunPod` one: its workspace disk lives in
/// its resource group, not in a `RunPod` storage journal.
fn azure() -> Fixture {
    let fixture = Fixture::new();
    std::fs::remove_file(fixture.root.join("workspace-volume.json")).unwrap();
    fixture.edit(|state| {
        state.profile.provider = horizon_cloud::azure::PROVIDER.into();
        state.spec.as_mut().unwrap().profile = state.profile.clone();
        state.spec.as_mut().unwrap().registry_auth_id = None;
    });
    fixture
}

fn new_server(script: &Script) -> NewServer<'_> {
    NewServer {
        script,
        hosting: Hosting::default(),
    }
}

#[test]
fn every_provider_is_released_by_its_own_rebuild() {
    use super::super::live::{Host, host};
    use horizon_cloud::provider::{AZURE, DOCKER_HOST, HETZNER, RUNPOD};
    let of = |description: &horizon_cloud::provider::Description| {
        host(description.rebuild, description.kind).map_err(|error| error.to_string())
    };
    assert_eq!(of(&RUNPOD), Ok(None), "RunPod switches the worker in place");
    assert_eq!(of(&HETZNER), Ok(Some(Host::Hetzner)));
    assert_eq!(
        of(&AZURE),
        Ok(Some(Host::Azure)),
        "Azure releases its own VM, not through Hetzner"
    );
    assert_eq!(of(&DOCKER_HOST), Err(super::super::UNSUPPORTED.to_owned()));
}

#[test]
fn only_a_docker_host_is_refused_as_a_provider_and_its_record_is_unchanged() {
    let fixture = Fixture::new();
    fixture.edit(|state| {
        state.profile.provider = horizon_cloud::provider::DOCKER_HOST.id.into();
        state.spec.as_mut().unwrap().profile = state.profile.clone();
    });
    let before = std::fs::read(fixture.root.join("deployment.json")).unwrap();
    let state = fixture.state();
    let request = Request::new(
        state.cloud_id.clone(),
        state.repository.clone(),
        state.revision.clone(),
        state.profile.clone(),
        fixture.root.clone(),
        serde_json::from_value(json!({
            "runpod_key_file": "/unused", "ssh_identity_file": "/unused", "docker_config": "/unused",
            "cpu_flavors": [], "gpu_types": []
        }))
        .unwrap(),
    );
    // Refused before the checkout's recipe is read: the repository does not exist.
    let refusal = super::super::rebuild(&request, "dev", None, &Cancellation::default(), &|_| {})
        .unwrap_err()
        .to_string();
    assert_eq!(refusal, super::super::UNSUPPORTED);
    assert_eq!(std::fs::read(fixture.root.join("deployment.json")).unwrap(), before);

    // An Azure cloud passes the provider check and meets the next refusal in order.
    let fixture = azure();
    assert!(
        ready(&fixture.state()).is_ok(),
        "a ready Azure cloud with a recipe rebuilds"
    );
    fixture.edit(|state| state.profile.build = None);
    assert_eq!(
        ready(&fixture.state()).unwrap_err().to_string(),
        super::super::NO_RECIPE
    );
    fixture.edit(|state| state.stage = Stage::Stopped);
    assert_eq!(
        ready(&fixture.state()).unwrap_err().to_string(),
        super::super::NO_RECIPE
    );
    let fixture = azure();
    fixture.edit(|state| state.stage = Stage::Stopped);
    assert_eq!(
        ready(&fixture.state()).unwrap_err().to_string(),
        super::super::NOT_READY
    );
}

#[test]
fn an_azure_rebuild_releases_the_vm_and_commits_the_image_with_the_fence_cleared() {
    let fixture = azure();
    let script = Script::new(&fixture);
    let steps = new_server(&script);
    assert_eq!(rebuild_new(&fixture, &steps).unwrap(), Driven::Committed);
    assert_committed(&fixture, &steps);
    assert_eq!(*steps.hosting.calls.borrow(), ["release", "reopen"]);
    assert_eq!(script.count("build"), 1);
}

#[test]
fn an_interrupted_azure_rebuild_continues_and_a_cancel_keeps_or_replaces_the_vm() {
    for boundary in [Boundary::Built, Boundary::Requested, Boundary::Released] {
        let fixture = azure();
        let script = Script::new(&fixture);
        let steps = new_server(&script);
        steps.hosting.fail_at.set(Some(boundary));
        assert!(rebuild_new(&fixture, &steps).is_err(), "{boundary:?}");
        steps.hosting.fail_at.set(None);
        continue_new(&fixture, &steps).unwrap();
        assert_committed(&fixture, &steps);
        assert_eq!(script.count("build"), 1, "{boundary:?}");
    }
    // Cancelled before the release: the same VM, image and stage.
    let fixture = azure();
    let before = fixture.state();
    let script = Script::new(&fixture);
    let steps = new_server(&script);
    steps.hosting.fail_at.set(Some(Boundary::Requested));
    assert!(rebuild_new(&fixture, &steps).is_err());
    assert_eq!(cancel_new(&fixture, &steps).unwrap(), flow::Cancelled::Untouched);
    assert_eq!(
        serde_json::to_value(fixture.state()).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    assert!(steps.hosting.calls.borrow().is_empty(), "the VM was never touched");
    // Cancelled after the release began: a new VM on the previous image.
    let fixture = azure();
    let script = Script::new(&fixture);
    let steps = new_server(&script);
    steps.hosting.release_fails.set(true);
    assert!(rebuild_new(&fixture, &steps).is_err());
    assert_eq!(cancel_new(&fixture, &steps).unwrap(), flow::Cancelled::Released);
    let after = fixture.state();
    assert_eq!(after.spec, before.spec, "the previous image is kept");
    assert_eq!(
        (after.operation, after.stage),
        (CreateState::Prepared, Stage::Readiness)
    );
    assert!(after.image_replacement.is_none() && after.session_restart.is_some());
}

#[test]
fn an_interrupted_azure_release_is_never_reconnected_around_or_read_as_a_stop() {
    use crate::cloud_runtime::{deployment::Compute as _, settings::Settings, state::REPLACEMENT_PENDING};
    let fixture = azure();
    let script = Script::new(&fixture);
    let steps = new_server(&script);
    steps.hosting.release_fails.set(true);
    assert!(rebuild_new(&fixture, &steps).is_err());
    let token = fixture.root.join("azure-token");
    std::fs::write(&token, "synthetic-arm-token").unwrap();
    std::fs::set_permissions(&token, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let settings: Settings = serde_json::from_value(json!({
        "runpod_key_file": fixture.root.join("unused"), "ssh_identity_file": fixture.root.join("unused"),
        "docker_config": fixture.root.join("unused"), "cpu_flavors": [], "gpu_types": [],
        "azure": {"token_file": token, "subscription_id": "00000000-0000-4000-8000-000000000001",
            "vm_sizes": ["Standard_D4s_v3"], "regions": ["northeurope"]}
    }))
    .unwrap();
    let store = fixture.store();
    let mut state = store.load().unwrap().unwrap();
    let cancel = Cancellation::default();
    // A reconnect would place a VM around the rebuild.
    let compute = crate::cloud_runtime::deployment::azure::Compute::new(&settings).unwrap();
    let refused = compute
        .settle(&store, &mut state, &cancel, crate::cloud_runtime::mutation::IGNORE)
        .unwrap_err();
    assert_eq!(refused.to_string(), REPLACEMENT_PENDING);
    // A provider check would read the deleted VM as missing.
    let refused = crate::cloud_runtime::providers::lifecycle(&state, &settings)
        .check(&store, &mut state, None, &cancel)
        .unwrap_err();
    assert_eq!(refused.to_string(), REPLACEMENT_PENDING);
    drop(store);
    let kept = fixture.state();
    assert_eq!(kept.stage, Stage::Replace);
    assert!(kept.image_replacement.as_ref().unwrap().requested());
}
