//! The committed profile a rebuild accepts: the choices a cloud binds when it is created
//! stay its worker's, and every other difference is refused with the field it is in.
use super::super::compatibility::{NO_CONFIG, NO_PROFILE};
use super::*;

/// Refuses `head` before anything is journaled, naming `reason`.
fn assert_refused(fixture: &Fixture, head: Option<CloudConfig>, reason: &str) {
    let mut script = Script::new(fixture);
    script.head.config = head;
    let error = rebuild_with(fixture, &script).unwrap_err().to_string();
    assert!(error.contains(reason), "{reason}: {error}");
    assert!(!error.contains("latest commit changes"), "{error}");
    assert!(fixture.state().image_replacement.is_none());
    assert_eq!(*script.calls.borrow(), ["head"]);
}

/// An edit of the bound profile, as a commit makes it.
type Change = fn(&mut Profile);

fn changed(bound: &Profile, change: impl FnOnce(&mut Profile)) -> CloudConfig {
    let mut head = bound.clone();
    change(&mut head);
    config(&head)
}

#[test]
fn a_changed_or_missing_committed_profile_refuses_with_its_field_before_anything_is_journaled() {
    let fixture = Fixture::new();
    let bound = fixture.state().profile;
    let mut renamed = config(&bound);
    renamed.profiles = [("other".into(), bound.clone())].into();
    assert_refused(&fixture, None, NO_CONFIG);
    assert_refused(&fixture, Some(renamed), NO_PROFILE);
    let changes: &[(Change, &str)] = &[
        (|profile| profile.gpu = true, "in gpu."),
        (
            |profile| profile.min_gpu_memory_gb = Some(24),
            "in min_cuda_version or min_gpu_memory_gb.",
        ),
        (|profile| profile.storage.container_gb = 40, "in storage.container_gb."),
        (
            |profile| profile.storage.volume_gb = 40,
            "storage.volume_gb or another storage.volume_tier",
        ),
        (
            |profile| profile.storage.volume_tier = horizon_cloud::runpod::volumes::Tier::HighPerformance,
            "storage.volume_gb or another storage.volume_tier",
        ),
        (|profile| profile.capabilities.desktop = true, "in capabilities."),
        (|profile| profile.image = "registry.example/other".into(), "in image."),
        (
            |profile| profile.build.as_mut().unwrap().dockerfile = "Other".into(),
            "in its build section",
        ),
        (
            |profile| profile.build.as_mut().unwrap().platform = "linux/arm64".into(),
            "in its build section",
        ),
        (|profile| profile.bootstrap.readiness_seconds = 900, "in bootstrap"),
        (|profile| profile.idle_stop_minutes = Some(30), "in idle_stop_minutes."),
        // Another default provider never hides a change the worker cannot take.
        (
            |profile| {
                profile.provider = horizon_cloud::hetzner::PROVIDER.into();
                profile.build.as_mut().unwrap().dockerfile = "Other".into();
            },
            "in its build section",
        ),
    ];
    for (change, reason) in changes {
        assert_refused(&fixture, Some(changed(&bound, change)), reason);
    }
    // A GPU worker's host minimums are not chosen at creation, so they must match.
    let gpu = Fixture::new();
    gpu.edit(|state| {
        state.profile.gpu = true;
        state.spec.as_mut().unwrap().profile = state.profile.clone();
    });
    let bound = gpu.state().profile;
    let doubled = changed(&bound, |profile| profile.cpu *= 2);
    assert_refused(&gpu, Some(doubled), "in cpu or memory_gb.");
}

#[test]
fn a_cloud_created_on_another_provider_than_the_committed_default_rebuilds_where_it_is() {
    let fixture = Fixture::new();
    let before = fixture.state();
    let mut head = before.profile.clone();
    head.provider = horizon_cloud::hetzner::PROVIDER.into();
    // Its vCPU and memory were chosen at creation too.
    (head.cpu, head.memory_gb) = (head.cpu * 2, head.memory_gb * 2);
    let mut script = Script::new(&fixture);
    // The latest commit is the one the cloud was created at.
    script.head.revision.clone_from(&before.revision);
    script.head.config = Some(config(&head));
    assert_eq!(rebuild_with(&fixture, &script).unwrap(), Driven::Committed);
    // The worker is switched in place on its own provider and keeps its profile.
    assert_eq!(script.calls.borrow()[..3], ["head", "build", "replace Next"]);
    let after = fixture.state();
    assert_eq!(after.profile, before.profile);
    assert_eq!(after.spec.as_ref().unwrap().profile, before.profile);
    assert_eq!(after.spec.unwrap().image_digest, digest('b'));
    assert_eq!(after.operation, before.operation, "the same worker");
    assert_eq!(after.revision, before.revision);
}

#[test]
fn a_workspace_grown_after_creation_is_not_a_change() {
    let fixture = Fixture::new();
    let committed = fixture.state().profile;
    fixture.edit(|state| {
        state.profile.storage.volume_gb *= 2;
        state.spec.as_mut().unwrap().profile = state.profile.clone();
    });
    // As `storage::growth` records it in the storage journal too.
    let path = fixture.root.join("workspace-volume.json");
    let mut journal: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    journal["worker"] = serde_json::to_value(fixture.state().spec).unwrap();
    journal["spec"]["size"] = json!(fixture.state().profile.storage.volume_gb);
    std::fs::write(&path, journal.to_string()).unwrap();
    let mut script = Script::new(&fixture);
    script.head.config = Some(config(&committed));
    assert_eq!(rebuild_with(&fixture, &script).unwrap(), Driven::Committed);
}
