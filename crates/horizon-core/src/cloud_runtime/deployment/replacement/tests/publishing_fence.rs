use super::*;
use crate::cloud_runtime::publishing::Publication;

#[test]
fn a_rebuild_requires_the_original_grant_before_compute_credentials_are_loaded() {
    let fixture = Fixture::new();
    let mut state = fixture.state();
    state.registry_generation = None;
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
    let store = fixture.store();
    store.save(&state).unwrap();
    let before = std::fs::read(fixture.root.join("deployment.json")).unwrap();
    let settings: Settings = serde_json::from_value(json!({
        "runpod_key_file":fixture.root.join("missing-key"),"ssh_identity_file":fixture.root.join("missing-identity"),
        "docker_config":fixture.root.join("absent-docker"),"cpu_flavors":[],"gpu_types":[]
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
    let cancel = Cancellation::default();
    let emit = |_| {};
    let result = live::Live::new(&request, &store, &state, true, &cancel, &emit);
    assert!(matches!(
        result,
        Err(Error::Invalid(message)) if message.contains("publishing and read-only pull credentials")
    ));
    assert_eq!(std::fs::read(fixture.root.join("deployment.json")).unwrap(), before);
    assert!(!fixture.root.join("absent-docker").exists());
}

#[test]
fn rebuild_confirmation_cannot_cross_publication_grants_with_matching_images_and_files() {
    let fixture = Fixture::new();
    let source = fixture.root.parent().unwrap().join("source");
    std::fs::create_dir_all(source.join(".horizon")).unwrap();
    let mut recipe = fixture.state().profile;
    recipe.image.clear();
    let config = config(&recipe);
    std::fs::write(
        source.join(".horizon/cloud.yml"),
        serde_yaml::to_string(&config).unwrap(),
    )
    .unwrap();
    std::fs::write(source.join("Dockerfile"), "FROM scratch\n").unwrap();
    let repository = git2::Repository::init(&source).unwrap();
    let mut index = repository.index().unwrap();
    index.add_all(["*"], git2::IndexAddOption::DEFAULT, None).unwrap();
    let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
    let signature = git2::Signature::now("Fixture", "fixture@example.invalid").unwrap();
    repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "test: add build recipe",
            &tree,
            &[],
        )
        .unwrap();
    fixture.edit(|state| {
        state.repository = source.clone();
        state.registry_generation = None;
        state.publication = Some(Box::new(Publication {
            profile: "dev".into(),
            requested_image: String::new(),
            destination: horizon_cloud::image_publishing::Destination::bound(
                state.profile.image.clone(),
                "grant-b".into(),
            )
            .unwrap(),
            account: None,
        }));
    });
    let state = fixture.state();
    let mut first = state.clone();
    first.publication.as_mut().unwrap().destination =
        horizon_cloud::image_publishing::Destination::bound(state.profile.image.clone(), "grant-a".into()).unwrap();
    let cancel = Cancellation::default();
    let first_plan = plan_for_cloud(&first, "dev", &cancel).unwrap();
    let second_plan = plan_for_cloud(&state, "dev", &cancel).unwrap();
    assert_eq!(first_plan.profile, second_plan.profile);
    assert_eq!(first_plan.uncommitted, second_plan.uncommitted);
    assert_ne!(first_plan.token(), second_plan.token());
    let settings: Settings = serde_json::from_value(json!({
        "runpod_key_file":fixture.root.join("missing-key"),"ssh_identity_file":fixture.root.join("missing-identity"),
        "docker_config":fixture.root.join("absent-docker"),"cpu_flavors":[],"gpu_types":[]
    }))
    .unwrap();
    let request = Request::new(
        state.cloud_id,
        source,
        state.revision,
        state.profile,
        fixture.root.clone(),
        settings,
    );
    let before = std::fs::read(fixture.root.join("deployment.json")).unwrap();
    for confirmed in [Some(&first_plan), None] {
        let error = rebuild(&request, "dev", confirmed, &cancel, &|_| {}).unwrap_err();
        assert!(error.to_string().contains("publishing identity changed"), "{error}");
    }
    assert_eq!(std::fs::read(fixture.root.join("deployment.json")).unwrap(), before);
    assert!(!fixture.root.join("absent-docker").exists());
}

#[test]
fn rebuild_tokens_distinguish_personal_account_ids_with_the_same_namespace() {
    let destination = horizon_cloud::image_publishing::Destination::automatic_github(
        "builder",
        "https://git.example/team/source",
        "dev",
    )
    .unwrap();
    let profile = serde_json::from_value(json!({
        "provider":"runpod","image":destination.repository,"cpu":2,"memory_gb":4,
        "build":{"context":".","dockerfile":"Dockerfile"}
    }))
    .unwrap();
    let mut plan = Plan {
        profile,
        changes: Vec::new(),
        notes: Vec::new(),
        uncommitted: Uncommitted::default(),
        publication: Some(Publication {
            profile: "dev".into(),
            requested_image: String::new(),
            destination,
            account: Some(crate::cloud_runtime::github::publish::publication::Account {
                id: 17,
                login: "builder".into(),
            }),
        }),
    };
    let first = plan.token();
    plan.publication.as_mut().unwrap().account.as_mut().unwrap().id = 18;
    assert_ne!(first, plan.token());
    plan.publication = None;
    assert_eq!(
        plan.token(),
        crate::cloud_runtime::repository::uncommitted::token(&plan.profile, &plan.uncommitted)
    );
}
