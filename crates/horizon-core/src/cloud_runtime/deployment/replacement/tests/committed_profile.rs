//! The committed profile a rebuild accepts: the choices a cloud binds when it is created
//! stay its worker's, the rebuild settings come from the commit, and every other
//! difference is refused with the field it is in.
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
pub(super) type Change = fn(&mut Profile);

pub(super) fn changed(bound: &Profile, change: impl FnOnce(&mut Profile)) -> CloudConfig {
    let mut head = bound.clone();
    change(&mut head);
    config(&head)
}

#[test]
fn an_overridden_recipe_rebuilds_and_opens_with_its_recorded_destination() {
    use crate::cloud_runtime::publishing::Publication;
    let fixture = Fixture::new();
    fixture.edit(|state| {
        state.publication = Some(Box::new(Publication {
            profile: "dev".into(),
            requested_image: "ghcr.io/recipe-owner/worker".into(),
            destination: horizon_cloud::image_publishing::Destination::bound(
                state.profile.image.clone(),
                "synthetic-grant".into(),
            )
            .unwrap(),
            account: None,
        }));
    });
    let state = fixture.state();
    let mut recipe = state.profile.clone();
    recipe.image = state.publication.as_deref().unwrap().requested_image.clone();
    let configuration = config(&recipe);
    let next = compatibility::next_profile(
        &state.profile,
        Some(&configuration),
        "dev",
        state.publication.as_deref(),
    )
    .unwrap();
    assert_eq!(next.image, state.profile.image);
    let settings = serde_json::from_value(json!({
        "runpod_key_file":fixture.root.join("unused"), "ssh_identity_file":fixture.root.join("unused-ssh"),
        "docker_config":fixture.root.join("docker"), "registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[]
    }))
    .unwrap();
    let request = Request::new(
        state.cloud_id,
        state.repository,
        state.revision,
        recipe,
        fixture.root.clone(),
        settings,
    );
    let (_store, reopened) = open(&request).unwrap();
    assert_eq!(reopened.profile.image, next.image);
    assert_eq!(reopened.publication, state.publication);
}

#[test]
fn a_published_cloud_refuses_another_rebuild_profile_even_when_its_image_matches_or_is_omitted() {
    use crate::cloud_runtime::publishing::Publication;
    let fixture = Fixture::new();
    let mut state = fixture.state();
    state.publication = Some(Box::new(Publication {
        profile: "dev".into(),
        requested_image: state.profile.image.clone(),
        destination: horizon_cloud::image_publishing::Destination::bound(
            state.profile.image.clone(),
            "synthetic-grant".into(),
        )
        .unwrap(),
        account: None,
    }));
    for automatic in [false, true] {
        let mut recipe = state.profile.clone();
        if automatic {
            recipe.image.clear();
        }
        recipe.build.as_mut().unwrap().dockerfile = "Other.Dockerfile".into();
        let mut configuration = config(&recipe);
        configuration.profiles = [("other".into(), recipe)].into();
        let error = compatibility::next_profile(
            &state.profile,
            Some(&configuration),
            "other",
            state.publication.as_deref(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("recorded build profile"), "{error}");
    }
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
        (|profile| profile.image = "registry.example/other".into(), "in image."),
        (
            |profile| profile.capabilities.browserstack = Some(horizon_cloud::BrowserStack::default()),
            "in capabilities.browserstack.",
        ),
        (
            |profile| profile.bootstrap.contract_version = 2,
            "in bootstrap.contract_version.",
        ),
        (|profile| profile.build = None, "has no build section"),
        // A rebuild setting is still checked as every profile is.
        (|profile| profile.bootstrap.readiness_seconds = 5, "readiness timeout"),
        (
            |profile| profile.build.as_mut().unwrap().platform = "linux/arm64".into(),
            "platform must be linux/amd64",
        ),
        // Another default provider never hides a change the worker cannot take.
        (
            |profile| {
                profile.provider = horizon_cloud::hetzner::PROVIDER.into();
                profile.image = "registry.example/other".into();
            },
            "in image.",
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
    fixture.bind_storage(|storage| storage.volume_gb *= 2);
    let mut script = Script::new(&fixture);
    script.head.config = Some(config(&committed));
    assert_eq!(rebuild_with(&fixture, &script).unwrap(), Driven::Committed);
}

#[test]
fn a_rebuilt_explicit_recipe_can_omit_its_image_when_reconnecting_to_the_retained_publisher() {
    use crate::cloud_runtime::publishing::Publication;
    let fixture = Fixture::new();
    let original_image = "ghcr.io/recipe-owner/worker";
    fixture.edit(|state| {
        state.publication = Some(Box::new(Publication {
            profile: "dev".into(),
            requested_image: original_image.into(),
            destination: horizon_cloud::image_publishing::Destination::bound(
                state.profile.image.clone(),
                "synthetic-grant".into(),
            )
            .unwrap(),
            account: None,
        }));
    });
    let mut recipe = fixture.state().profile;
    recipe.image.clear();
    recipe.build.as_mut().unwrap().dockerfile = "Dockerfile.rebuilt".into();
    let mut script = Script::new(&fixture);
    script.head.config = Some(config(&recipe));
    assert_eq!(rebuild_with(&fixture, &script).unwrap(), Driven::Committed);
    let rebuilt = fixture.state();
    assert_eq!(rebuilt.profile.build, recipe.build);
    assert!(!rebuilt.profile.image.is_empty());
    let settings = serde_json::from_value(json!({
        "runpod_key_file":fixture.root.join("missing-key"),"ssh_identity_file":fixture.root.join("missing-identity"),
        "docker_config":fixture.root.join("absent-docker"),"cpu_flavors":[],"gpu_types":[]
    }))
    .unwrap();
    let mut request = Request::new(
        rebuilt.cloud_id.clone(),
        rebuilt.repository.clone(),
        rebuilt.revision.clone(),
        recipe,
        fixture.root.clone(),
        settings,
    );
    request.profile_name = Some("dev".into());
    let before = std::fs::read(fixture.root.join("deployment.json")).unwrap();
    for image in ["", original_image, rebuilt.profile.image.as_str()] {
        request.profile.image = image.into();
        super::super::super::prepare(&request).unwrap();
        let mut unchanged = rebuilt.clone();
        assert!(!super::super::super::sizing::assign_requested_size(&request, &mut unchanged).unwrap());
        assert_eq!(unchanged.profile, rebuilt.profile);
        let (_guard, reopened) = open(&request).unwrap();
        assert_eq!(reopened.profile, rebuilt.profile);
        assert_eq!(reopened.publication, rebuilt.publication);
        assert_eq!(reopened.spec, rebuilt.spec);
    }
    request.profile.image = "registry.example/another/worker".into();
    assert!(super::super::super::prepare(&request).is_err());
    assert!(open(&request).is_err());
    assert!(super::super::super::sizing::assign_requested_size(&request, &mut rebuilt.clone()).is_err());
    request.profile.image.clear();
    request.profile_name = Some("another-profile".into());
    assert!(super::super::super::prepare(&request).is_err());
    assert_eq!(std::fs::read(fixture.root.join("deployment.json")).unwrap(), before);
    assert!(!request.settings.docker_config.exists());
}
