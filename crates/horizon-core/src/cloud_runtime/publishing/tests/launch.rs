use super::*;
use crate::cloud_runtime::{
    Cancellation,
    command::Runner,
    deployment::{self, Request},
    state::Store,
};
use serde_json::json;
mod source;

#[test]
#[cfg(unix)]
fn a_publishing_login_inside_the_build_source_is_refused_before_sign_in() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repo");
    std::fs::create_dir(&repository).unwrap();
    let mut settings = settings(root.path());
    settings.docker_config = repository.join("private-login");
    let profile: Profile = serde_json::from_value(
        json!({"provider":"runpod","cpu":2,"memory_gb":4,"build":{"context":".","dockerfile":"Dockerfile"}}),
    )
    .unwrap();
    let mut request = Request::new(
        "source-guard".into(),
        repository,
        "a".repeat(40),
        profile,
        root.path().join("cloud"),
        settings,
    );
    request.profile_name = Some("cpu".into());
    deployment::prepare(&request).unwrap();
    let store = Store::lock(&request.state_root).unwrap();
    let mut state = store.load().unwrap().unwrap();
    let cancel = Cancellation::default();
    let events = std::cell::RefCell::new(Vec::new());
    let emit = |event| events.borrow_mut().push(event);
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    assert!(prepare(&request, &store, &mut state, &runner).is_err());
    assert!(!request.settings.docker_config.exists());
    assert!(events.borrow().is_empty(), "no sign-in or credential write starts");
}

fn settings(root: &Path) -> Settings {
    serde_json::from_value(json!({
        "runpod_key_file":root.join("key"),"ssh_identity_file":root.join("ssh"),"docker_config":root.join("docker"),
        "registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[],
        "portable_image_publishing":true,
        "registries":{"root":root.join("registry"),"bindings":[{
            "repository":"registry.example/team/worker","generation":"grant-one","read_only_confirmed":true,
            "pull":{"username":"reader","secret_file":root.join("pull"),"expires_at":null},
            "publish":{"username":"builder","secret_file":root.join("push"),"expires_at":null}
        }]}
    }))
    .unwrap()
}

#[test]
fn profile_omits_an_image_only_with_a_build() {
    let mut profile: Profile = serde_json::from_value(json!({
        "provider":"runpod","cpu":2,"memory_gb":4,
        "build":{"context":".","dockerfile":"Dockerfile"}
    }))
    .unwrap();
    assert!(profile.automatic_image());
    profile.validate(false).unwrap();
    assert!(serde_json::to_value(&profile).unwrap().get("image").is_none());
    profile.build = None;
    assert!(profile.validate(false).is_err());
    profile.image = "registry.example/base:1".into();
    profile.validate(false).unwrap();
}

#[test]
fn execution_uses_the_same_canonical_preference_scope_as_the_confirmed_selection() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repo");
    std::fs::create_dir(&repository).unwrap();
    let alias = repository.join("..").join("repo");
    let mut settings = settings(root.path());
    settings.image_publishing.push(Override {
        repository: alias.clone(),
        profile: "cpu".into(),
        destination: "registry.example/team/worker".into(),
        account: None,
        binding: Some(grant_id(&settings.registries.as_ref().unwrap().bindings[0]).unwrap()),
    });
    let profile: Profile = serde_json::from_value(json!({"provider":"runpod","image":"registry.example/team/from-profile","cpu":2,"memory_gb":4,"build":{"context":".","dockerfile":"Dockerfile"}})).unwrap();
    let cancel = Cancellation::default();
    let shown = selection(&settings, &alias, "cpu", &profile, &cancel).unwrap();
    assert!(matches!(shown, Selection::Recipe { .. }));
    let mut request = Request::new(
        "scope-test".into(),
        alias,
        "a".repeat(40),
        profile,
        root.path().join("cloud"),
        settings,
    );
    request.profile_name = Some("cpu".into());
    request.publishing_selection = Some(shown);
    deployment::prepare(&request).unwrap();
    let store = Store::lock(&request.state_root).unwrap();
    let mut state = store.load().unwrap().unwrap();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    assert!(prepare(&request, &store, &mut state, &runner).unwrap().is_none());
    assert!(state.publication.is_none());
    assert_eq!(state.profile.image, "registry.example/team/from-profile");
}

#[test]
fn local_overrides_require_an_exact_scope_and_separate_publishing_grant() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = settings(root.path());
    let choice = Override {
        repository: root.path().join("repo"),
        profile: "cpu".into(),
        destination: "registry.example/team/worker".into(),
        account: None,
        binding: Some(grant_id(&settings.registries.as_ref().unwrap().bindings[0]).unwrap()),
    };
    choice.validate(&settings).unwrap();
    assert!(
        Override {
            destination: "registry.example/another/worker".into(),
            ..choice.clone()
        }
        .validate(&settings)
        .is_err()
    );
    settings.image_publishing = vec![choice.clone(), choice];
    assert!(settings.validate().is_err());
    settings.image_publishing.pop();
    settings.registries.as_mut().unwrap().bindings[0].publish = None;
    settings.validate().unwrap();
    assert!(settings.image_publishing[0].validate(&settings).is_err());
}

#[test]
fn publication_rejects_mismatched_profile_and_missing_or_reused_account() {
    let destination = Destination::automatic_github("alice", "gitlab.example/group/app", "cpu").unwrap();
    let mut profile:Profile=serde_json::from_value(json!({"provider":"runpod","image":destination.repository,"cpu":2,"memory_gb":4,"build":{"context":".","dockerfile":"Dockerfile"}})).unwrap();
    let mut publication = Publication {
        profile: "cpu".into(),
        requested_image: String::new(),
        destination,
        account: Some(Account {
            id: 17,
            login: "alice".into(),
        }),
    };
    publication.validate(&profile).unwrap();
    publication.account.as_mut().unwrap().id = 0;
    assert!(publication.validate(&profile).is_err());
    publication.account.as_mut().unwrap().id = 17;
    profile.image = "ghcr.io/bob/app".into();
    assert!(publication.validate(&profile).is_err());
}

#[test]
fn pull_rotation_keeps_the_publisher_but_another_push_grant_does_not() {
    let root = tempfile::tempdir().unwrap();
    let settings = settings(root.path());
    let binding = &settings.registries.as_ref().unwrap().bindings[0];
    let original = grant_id(binding).unwrap();
    let mut rotated = binding.clone();
    rotated.generation = "new-read-only-generation".into();
    rotated.pull.username = "new-reader".into();
    rotated.publish.as_mut().unwrap().expires_at = Some("2030-01-01T00:00:00Z".into());
    assert_eq!(grant_id(&rotated).unwrap(), original);
    rotated.publish.as_mut().unwrap().username = "another-builder".into();
    assert_ne!(grant_id(&rotated).unwrap(), original);
}

#[test]
#[cfg(unix)]
fn retry_keeps_the_destination_when_local_defaults_change() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = settings(root.path());
    let repository = root.path().join("repo");
    std::fs::create_dir(&repository).unwrap();
    let repository = repository.canonicalize().unwrap();
    settings.image_publishing.push(Override {
        repository: repository.clone(),
        profile: "cpu".into(),
        destination: "registry.example/team/worker".into(),
        account: None,
        binding: Some(grant_id(&settings.registries.as_ref().unwrap().bindings[0]).unwrap()),
    });
    let profile: Profile = serde_json::from_value(
        json!({"provider":"runpod","cpu":2,"memory_gb":4,"build":{"context":".","dockerfile":"Dockerfile"}}),
    )
    .unwrap();
    let mut request = Request::new(
        "selection-test".into(),
        repository,
        "a".repeat(40),
        profile,
        root.path().join("cloud"),
        settings,
    );
    request.profile_name = Some("cpu".into());
    request.publishing_selection = Some(choices(&request.settings).unwrap().remove(0));
    deployment::prepare(&request).unwrap();
    let store = Store::lock(&request.state_root).unwrap();
    let mut state = store.load().unwrap().unwrap();
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    assert!(prepare(&request, &store, &mut state, &runner).unwrap().is_none());
    let recorded = store.load().unwrap().unwrap();
    assert_eq!(recorded.profile.image, "registry.example/team/worker");
    assert_eq!(recorded.version, crate::cloud_runtime::state::PUBLISHING_RECORD_VERSION);
    let path = request.state_root.join("deployment.json");
    let original = std::fs::read(&path).unwrap();
    let mut downgraded = serde_json::to_value(&recorded).unwrap();
    downgraded["version"] = json!(1);
    std::fs::write(&path, serde_json::to_vec(&downgraded).unwrap()).unwrap();
    assert!(
        store.load().is_err(),
        "publication metadata cannot use an older readable version"
    );
    downgraded["version"] = json!(recorded.version);
    downgraded.as_object_mut().unwrap().remove("publication");
    std::fs::write(&path, serde_json::to_vec(&downgraded).unwrap()).unwrap();
    assert!(
        store.load().is_err(),
        "removing the publisher cannot keep the version fence valid"
    );
    std::fs::write(&path, original).unwrap();
    request.publishing_selection = Some(Selection::Registry {
        destination: recorded.publication.as_ref().unwrap().destination.clone(),
    });
    let mut changed = recorded.clone();
    changed.profile.image = "registry.example/team/another".into();
    changed
        .publication
        .as_mut()
        .unwrap()
        .destination
        .repository
        .clone_from(&changed.profile.image);
    store.save(&changed).unwrap();
    for built in [false, true] {
        changed.spec = built.then(|| built_spec(&changed));
        assert!(matches!(
            prepare(&request, &store, &mut changed, &runner),
            Err(Error::Invalid(
                "The cloud's recorded destination differs from the confirmed plan; review it before starting"
            ))
        ));
    }
    assert_eq!(
        store.load().unwrap().unwrap().publication,
        changed.publication,
        "refusal preserves the changed record for reconciliation"
    );
    assert!(!request.settings.docker_config.exists());
    store.save(&recorded).unwrap();
    request.settings.image_publishing.clear();
    prepare(&request, &store, &mut state, &runner).unwrap();
    assert_eq!(state.publication, recorded.publication);
    drop(store);
    deployment::prepare(&request).unwrap();
    // A reconnect using the resolved profile matches too.
    request.profile = state.profile;
    deployment::prepare(&request).unwrap();
    // Changing a publishing reference cannot redirect a recorded cloud.
    request.settings.registries.as_mut().unwrap().bindings[0]
        .publish
        .as_mut()
        .unwrap()
        .secret_file = root.path().join("another-account");
    let store = Store::lock(&request.state_root).unwrap();
    let mut saved = store.load().unwrap().unwrap();
    assert!(prepare(&request, &store, &mut saved, &runner).is_err());
}

#[test]
#[cfg(unix)]
fn a_disabled_automatic_launch_leaves_no_unreadable_deployment_record() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repo");
    std::fs::create_dir(&repository).unwrap();
    let mut settings = settings(root.path());
    settings.portable_image_publishing = false;
    let profile: Profile = serde_json::from_value(json!({
        "provider":"runpod", "cpu":2, "memory_gb":4,
        "build":{"context":".","dockerfile":"Dockerfile"}
    }))
    .unwrap();
    let request = Request::new(
        "disabled-publication".into(),
        repository,
        "a".repeat(40),
        profile,
        root.path().join("cloud"),
        settings,
    );
    assert!(matches!(deployment::prepare(&request), Err(Error::Invalid(STAGED))));
    assert!(!request.state_root.join("deployment.json").exists());
    assert!(!request.settings.docker_config.exists());
}

#[test]
fn a_recipe_confirmation_does_not_confirm_a_new_publisher_at_the_same_uri() {
    let publication = Publication {
        profile: "cpu".into(),
        requested_image: "registry.example/team/worker".into(),
        destination: Destination::bound("registry.example/team/worker".into(), "selected-grant".into()).unwrap(),
        account: None,
    };
    assert!(
        publication
            .check_selection(&Selection::Recipe {
                image: publication.destination.repository.clone()
            })
            .is_err()
    );
}

// Only Unix record-backed tests construct this worker specification.
#[cfg(unix)]
fn built_spec(state: &crate::cloud_runtime::state::Deployment) -> horizon_cloud::WorkerSpec {
    horizon_cloud::WorkerSpec {
        exact_placement: false,
        operation_id: state.cloud_id.clone(),
        image_digest: format!("{}@sha256:{}", state.profile.image, "a".repeat(64)),
        profile: state.profile.clone(),
        public_key: "synthetic-unused-public-key".into(),
        registry_auth_id: None,
        gpu_types: Vec::new(),
        cpu_flavors: Vec::new(),
        data_centers: Vec::new(),
        startup_metadata: None,
    }
}

#[test]
#[cfg(unix)]
fn a_fresh_cpu_override_is_recorded_before_admitting_the_recipe_image() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repo");
    std::fs::create_dir(&repository).unwrap();
    let repository = repository.canonicalize().unwrap();
    let mut settings = settings(root.path());
    for (name, value) in [
        ("host-token", "synthetic-host-token"),
        ("pull", "synthetic-pull"),
        ("push", "synthetic-push"),
    ] {
        let file = tempfile::NamedTempFile::new_in(root.path()).unwrap();
        std::fs::write(file.path(), value).unwrap();
        file.persist(root.path().join(name)).unwrap();
    }
    settings.hetzner = Some(
        serde_json::from_value(
            json!({"token_file":root.path().join("host-token"),"server_types":["cx33"],"locations":["hel1"]}),
        )
        .unwrap(),
    );
    settings.image_publishing.push(Override {
        repository: repository.clone(),
        profile: "cpu".into(),
        destination: "registry.example/team/worker".into(),
        account: None,
        binding: Some(grant_id(&settings.registries.as_ref().unwrap().bindings[0]).unwrap()),
    });
    let mut unused = settings.registries.as_ref().unwrap().bindings[0].clone();
    unused.repository = "ghcr.io/another-owner/worker".into();
    unused.generation = "old-recipe-grant".into();
    unused.publish = None;
    unused.pull.secret_file = root.path().join("absent-old-pull");
    settings.registries.as_mut().unwrap().bindings.push(unused);
    let profile=serde_json::from_value(json!({"provider":"hetzner","image":"ghcr.io/another-owner/worker","cpu":2,"memory_gb":4,"build":{"context":".","dockerfile":"Dockerfile"},"storage":{"volume_gb":10}})).unwrap();
    let mut request = Request::new(
        "fresh-override".into(),
        repository,
        "a".repeat(40),
        profile,
        root.path().join("cloud"),
        settings,
    );
    request.profile_name = Some("cpu".into());
    request.publishing_selection = Some(choices(&request.settings).unwrap().remove(0));
    deployment::prepare(&request).unwrap();
    let state = Store::lock(&request.state_root).unwrap().load().unwrap().unwrap();
    assert_eq!(state.profile.image, "registry.example/team/worker");
    assert!(state.publication.is_some());

    request.cloud_id = "missing-selected-pull".into();
    request.state_root = root.path().join("missing-selected-pull");
    request.settings.registries.as_mut().unwrap().bindings[0]
        .pull
        .secret_file = root.path().join("absent-selected-pull");
    assert!(deployment::prepare(&request).is_err());
    let retained = Store::snapshot(&request.state_root).unwrap().unwrap();
    assert_eq!(retained.profile.image, "registry.example/team/worker");
    assert_eq!(retained.publication, state.publication);
}

#[test]
#[cfg(unix)]
fn a_legacy_unbuilt_cloud_does_not_adopt_a_later_local_destination() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repo");
    std::fs::create_dir(&repository).unwrap();
    let profile=serde_json::from_value(json!({"provider":"runpod","image":"registry.example/team/original","cpu":2,"memory_gb":4,"build":{"context":".","dockerfile":"Dockerfile"}})).unwrap();
    let mut request = Request::new(
        "legacy-target".into(),
        repository.clone(),
        "a".repeat(40),
        profile,
        root.path().join("cloud"),
        settings(root.path()),
    );
    request.profile_name = Some("cpu".into());
    deployment::prepare(&request).unwrap();
    request.settings.image_publishing.push(Override {
        repository,
        profile: "cpu".into(),
        destination: "registry.example/team/worker".into(),
        account: None,
        binding: Some(grant_id(&request.settings.registries.as_ref().unwrap().bindings[0]).unwrap()),
    });
    let store = Store::lock(&request.state_root).unwrap();
    let mut state = store.load().unwrap().unwrap();
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    prepare(&request, &store, &mut state, &runner).unwrap();
    assert!(state.publication.is_none());
    assert_eq!(state.profile.image, "registry.example/team/original");
    request.publishing_selection = Some(
        request.settings.image_publishing[0]
            .selection(&request.settings)
            .unwrap(),
    );
    assert!(prepare(&request, &store, &mut state, &runner).is_err());
    request.settings.portable_image_publishing = false;
    assert!(matches!(prepare(&request, &store, &mut state, &runner),
        Err(Error::Invalid(message)) if message == super::super::STAGED));
    request.publishing_selection = Some(Selection::Recipe {
        image: state.profile.image.clone(),
    });
    prepare(&request, &store, &mut state, &runner).unwrap();
    drop(store);
    request.profile.image.clear();
    deployment::prepare(&request).unwrap();
    assert_eq!(
        Store::snapshot(&request.state_root).unwrap().unwrap().profile.image,
        "registry.example/team/original"
    );
}
