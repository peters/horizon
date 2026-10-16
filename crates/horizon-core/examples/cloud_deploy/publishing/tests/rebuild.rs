use super::*;

#[test]
fn a_clean_retained_rebuild_requires_its_printed_publisher_token() {
    let (root, repo, settings, mut recipe) = fixture();
    recipe.capabilities.agents.clear();
    std::fs::create_dir(repo.join(".horizon")).unwrap();
    std::fs::write(repo.join("Dockerfile"), "FROM scratch\n").unwrap();
    std::fs::write(
        repo.join(".horizon/cloud.yml"),
        serde_yaml::to_string(&serde_json::json!({
            "version":1,"default":"cpu","profiles":{"cpu":recipe}
        }))
        .unwrap(),
    )
    .unwrap();
    let repository = git2::Repository::open(&repo).unwrap();
    let mut index = repository.index().unwrap();
    index.add_all(["*"], git2::IndexAddOption::DEFAULT, None).unwrap();
    let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
    index.write().unwrap();
    let signature = git2::Signature::now("Fixture", "fixture@example.invalid").unwrap();
    let revision = repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "test: add build recipe",
            &tree,
            &[],
        )
        .unwrap()
        .to_string();
    let destination = horizon_cloud::image_publishing::Destination::bound(
        "registry.example/team/worker".into(),
        "synthetic-grant".into(),
    )
    .unwrap();
    let mut profile = recipe;
    profile.image.clone_from(&destination.repository);
    let publication = Publication {
        profile: "cpu".into(),
        requested_image: String::new(),
        destination,
        account: None,
    };
    let state: Deployment = serde_json::from_value(serde_json::json!({
        "version":5,"cloud_id":"retained","repository":repo,"revision":revision,"profile":profile,
        "publication":publication,"stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},
        "worker":null,"sessions":[],"spec":{"operation_id":"retained","profile":profile,
        "image_digest":format!("registry.example/team/worker@sha256:{}","a".repeat(64)),
        "public_key":"unused","registry_auth_id":null,"gpu_types":[],"cpu_flavors":[],"data_centers":[]}
    }))
    .unwrap();
    let state_root = root.path().join("cloud");
    Store::lock(&state_root).unwrap().save(&state).unwrap();
    let plan = super::super::super::replacement::plan_for_cloud(&state, "cpu", &Cancellation::default()).unwrap();
    assert!(plan.uncommitted.is_empty());
    let before = std::fs::read(state_root.join("deployment.json")).unwrap();
    let mut args = vec![
        "rebuild".into(),
        "not-consumed".into(),
        state_root.to_str().unwrap().into(),
        "cpu".into(),
    ];
    let error = super::super::super::rebuild(&args, settings.clone(), &Cancellation::default()).unwrap_err();
    assert!(
        matches!(error, Error::Invalid(message) if message.contains("confirmation token")),
        "{error}"
    );
    args.push(plan.token());
    let error = super::super::super::rebuild(&args, settings.clone(), &Cancellation::default()).unwrap_err();
    assert!(
        matches!(error, Error::Invalid(message) if message.contains("publishing and read-only pull credentials")),
        "{error}"
    );
    assert_eq!(std::fs::read(state_root.join("deployment.json")).unwrap(), before);
    assert!(!settings.docker_config.exists());
}
