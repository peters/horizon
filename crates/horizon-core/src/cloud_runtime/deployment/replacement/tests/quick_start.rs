//! A cloud on the public base image without a recipe, as quick start makes it, rebuilds
//! by moving to the base image that this Horizon version pins.
use super::super::live;
use super::*;
use crate::cloud_runtime::{repository::launch::quick_start, settings::Settings};

/// An earlier pin of the base image, as a cloud made by an older Horizon runs it.
fn earlier_pin() -> String {
    format!("{}@sha256:{}", quick_start::REPOSITORY, "e".repeat(64))
}

/// A ready quick-start cloud on `image`, with the storage journal of its volume.
fn on_base(image: &str) -> Fixture {
    let fixture = Fixture::new();
    let mut profile = quick_start::builtin().unwrap().profiles[quick_start::PROFILE].clone();
    profile.image = image.into();
    fixture.edit(|state| {
        state.profile = profile.clone();
        state.registry_generation = None;
        let spec = state.spec.as_mut().unwrap();
        spec.profile = profile;
        spec.image_digest = image.into();
        spec.registry_auth_id = None;
    });
    let journal = json!({
        "version":1,"worker":fixture.state().spec,"state":{"state":"prepared"},
        "spec":{"operation_id":"rebuilt","size":20,"data_center_id":"EU-TEST-1"}
    });
    std::fs::write(fixture.root.join("workspace-volume.json"), journal.to_string()).unwrap();
    fixture
}

/// The steps of a quick-start rebuild: a latest commit without `.horizon/cloud.yml` and
/// the pin of this version, which needs no login.
fn pinned(fixture: &Fixture) -> Script {
    let mut script = Script::new(fixture);
    script.head.config = None;
    script.built = quick_start::IMAGE.into();
    script.built_login = None;
    script
}

#[test]
fn a_quick_start_cloud_moves_to_the_pin_and_keeps_its_volume() {
    let fixture = on_base(&earlier_pin());
    let script = pinned(&fixture);
    let before = fixture.state();
    assert!(rebuildable(&before.profile));
    assert_eq!(rebuild_with(&fixture, &script).unwrap(), Driven::Committed);
    assert_eq!(*script.calls.borrow(), ["head", "build", "replace Next", "observe"]);
    let state = fixture.state();
    let spec = state.spec.as_ref().unwrap();
    assert_eq!(spec.image_digest, quick_start::IMAGE);
    assert_eq!(spec.registry_auth_id, None);
    assert_eq!(state.registry_generation, None);
    assert_eq!(state.profile, before.profile, "the worker keeps its profile");
    assert_eq!(state.revision, before.revision, "the worktrees keep their commit");
    assert_eq!(
        fixture.storage_image(),
        quick_start::IMAGE,
        "the workspace volume stays bound to the worker"
    );
    assert!(state.session_restart.is_some());
    assert_eq!(state.stage, Stage::Readiness);
}

#[test]
fn a_quick_start_cloud_already_on_the_pin_is_unchanged() {
    let fixture = on_base(quick_start::IMAGE);
    let script = pinned(&fixture);
    assert_eq!(rebuild_with(&fixture, &script).unwrap(), Driven::Unchanged);
    assert_eq!(*script.calls.borrow(), ["head", "build"]);
    let state = fixture.state();
    assert!(state.image_replacement.is_none() && state.session_restart.is_none());
    assert_eq!(state.stage, Stage::Ready);
}

#[test]
fn committed_settings_are_never_passed_over() {
    let fixture = on_base(&earlier_pin());
    let mut script = pinned(&fixture);
    script.head.config = Some(config(&fixture.state().profile));
    let error = rebuild_with(&fixture, &script).unwrap_err().to_string();
    assert!(error.contains("its own .horizon/cloud.yml"), "{error}");
    assert_eq!(*script.calls.borrow(), ["head"], "nothing is built or switched");
    let state = fixture.state();
    assert!(state.image_replacement.is_none());
    assert_eq!(state.spec.unwrap().image_digest, earlier_pin());
}

#[test]
fn an_image_only_cloud_rebuilds_only_on_the_public_base_image() {
    let fixture = on_base(&earlier_pin());
    fixture.edit(|state| {
        state.profile.image = digest('a');
        state.spec.as_mut().unwrap().profile = state.profile.clone();
    });
    assert!(!rebuildable(&fixture.state().profile));
    let script = pinned(&fixture);
    let error = rebuild_with(&fixture, &script).unwrap_err().to_string();
    assert!(error.contains("no build section"), "{error}");
    assert!(script.calls.borrow().is_empty());
    assert!(fixture.state().image_replacement.is_none());
}

/// The production steps take the pin with its known contract: no recipe, no registry
/// and no Docker. The test runs again in a child process that has no `docker` program.
#[test]
fn the_live_rebuild_of_a_quick_start_cloud_needs_no_local_docker() {
    use std::os::unix::fs::PermissionsExt;
    if !crate::cloud_runtime::image::without_docker::child(
        "cloud_runtime::deployment::replacement::tests::quick_start::the_live_rebuild_of_a_quick_start_cloud_needs_no_local_docker",
    ) {
        return;
    }
    let fixture = on_base(&earlier_pin());
    // Git grants look the checkout up, so it must exist; nothing reads it.
    let checkout = fixture.root.clone();
    fixture.edit(|state| state.repository = checkout);
    let state = fixture.state();
    let key = fixture.root.join("compute-key");
    std::fs::write(&key, "synthetic-compute-key").unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    let settings: Settings = serde_json::from_value(json!({
        "runpod_key_file": key, "ssh_identity_file": "/absent", "docker_config": "/absent",
        "registry_pull_auth_id": "saved-pull-login", "cpu_flavors": [], "gpu_types": []
    }))
    .unwrap();
    let request = Request::new(
        state.cloud_id.clone(),
        state.repository.clone(),
        state.revision.clone(),
        state.profile.clone(),
        fixture.root.clone(),
        settings,
    );
    let store = fixture.store();
    let cancel = Cancellation::default();
    let output = RefCell::new(Vec::new());
    let emit = |event| {
        if let Event::Output(line) = event {
            output.borrow_mut().push(line);
        }
    };
    let steps = live::Live::new(&request, &store, &state, true, &cancel, &emit).unwrap();
    let image = steps
        .build(&state, &state.revision, &[], "horizon-rebuilt-pin")
        .unwrap();
    assert_eq!(
        image,
        ReplacementImage {
            digest: quick_start::IMAGE.into(),
            registry_auth_id: None,
            registry_generation: None,
        }
    );
    assert!(
        output
            .borrow()
            .iter()
            .any(|line| line.contains("does not run in local Docker")),
        "{:?}",
        output.borrow()
    );
}
