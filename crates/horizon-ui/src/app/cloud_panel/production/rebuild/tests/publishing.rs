use super::*;
use horizon_core::cloud_runtime::publishing::{Publication, Selection};

fn read_preview(app: &mut HorizonApp, ctx: &egui::Context) -> Result<Plan, String> {
    app.preview_production_rebuild(1, ctx);
    let Confirmation::Rebuild(preview) = &mut app
        .cloud_prototype
        .production
        .runtimes
        .get_mut(&1)
        .unwrap()
        .confirmation
    else {
        panic!("the rebuild confirmation remains open");
    };
    preview
        .reading
        .take()
        .unwrap()
        .recv_timeout(Duration::from_secs(20))
        .unwrap()
}

#[test]
fn desktop_rebuild_preview_keeps_the_retained_publisher_and_profile() {
    let (temp, mut app) = test_app();
    let ctx = egui::Context::default();
    let repository = temp.path().join("repository");
    std::fs::create_dir_all(repository.join(".horizon")).unwrap();
    let mut config = CloudConfig::parse(CONFIG).unwrap();
    let recipe = config.profiles["dev"].clone();
    config.profiles.insert("other".into(), recipe.clone());
    std::fs::write(
        repository.join(".horizon/cloud.yml"),
        serde_yaml::to_string(&config).unwrap(),
    )
    .unwrap();
    std::fs::write(repository.join(".horizon/Dockerfile"), "FROM scratch\n").unwrap();
    git(&repository, &["init", "--quiet"]);
    git(&repository, &["add", "."]);
    git(&repository, &["commit", "--quiet", "-m", "test: add build recipe"]);
    let state_root = seed_built_cloud(&mut app, temp.path());
    let destination = "registry.example/team/selected";
    let mut state = deployment(&repository, true, Phase::None);
    state.profile.image = destination.into();
    state.worker.as_mut().unwrap().image_name = destination.into();
    let spec = state.spec.as_mut().unwrap();
    spec.profile = state.profile.clone();
    spec.image_digest = format!("{destination}@sha256:{}", "a".repeat(64));
    state.publication = Some(Box::new(Publication {
        profile: "dev".into(),
        requested_image: recipe.image,
        destination: serde_json::from_value(serde_json::json!({
            "repository":destination,"publisher":{"kind":"registry","binding":"synthetic-grant"}
        }))
        .unwrap(),
        account: None,
    }));
    // The public version-5 wire shape retains publishing metadata.
    state.version = 5;
    let store = Store::lock(&state_root).unwrap();
    store.save(&state).unwrap();
    let state = store.load().unwrap().unwrap();
    drop(store);
    let group = &mut app.cloud_prototype.groups.0[0];
    group.cwd.clone_from(&repository);
    group.remote.as_mut().unwrap().publishing_selection = Some(Selection::Registry {
        destination: state.publication.as_ref().unwrap().destination.clone(),
    });
    let runtime = app.cloud_prototype.production.runtimes.entry(1).or_default();
    runtime.state = Some(state);
    runtime.confirmation = Confirmation::Rebuild(Box::default());
    let plan = read_preview(&mut app, &ctx).unwrap();
    assert_eq!(plan.profile.image, destination);
    let publication = plan.publication.as_ref().unwrap();
    assert_eq!(publication.profile, "dev");
    assert_eq!(publication.destination.repository, destination);
    assert!(!app.cloud_prototype.production.runtimes[&1].busy());
    app.cloud_prototype.groups.0[0].remote.as_mut().unwrap().profile_name = "other".into();
    let refusal = read_preview(&mut app, &ctx).unwrap_err();
    assert!(refusal.contains("recorded build profile"), "{refusal}");
    assert!(!temp.path().join("registry").exists());
    assert!(!temp.path().join("docker").exists());
}
