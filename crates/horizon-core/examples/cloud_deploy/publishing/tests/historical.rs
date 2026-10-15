use super::*;
use horizon_core::cloud_runtime::{Stage, deployment::Request, repository::uncommitted};

fn commit(repository: &git2::Repository, path: &str) -> String {
    let mut index = repository.index().unwrap();
    index.add_path(Path::new(path)).unwrap();
    let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
    index.write().unwrap();
    let signature = git2::Signature::now("Fixture", "fixture@example.invalid").unwrap();
    let parent = repository.head().ok().and_then(|head| head.peel_to_commit().ok());
    repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "test: save fixture source",
            &tree,
            &parent.iter().collect::<Vec<_>>(),
        )
        .unwrap()
        .to_string()
}

#[test]
fn a_historical_image_retry_keeps_the_recorded_working_copy_fence() {
    for had_edits in [false, true] {
        let (root, repo, settings, mut profile) = fixture();
        profile.image = "registry.example/team/worker".into();
        profile.capabilities.agents.clear();
        let build = profile.build.as_mut().unwrap();
        build.context = "worker".into();
        build.dockerfile = "worker/Dockerfile".into();
        std::fs::create_dir(repo.join("worker")).unwrap();
        let dockerfile = repo.join("worker/Dockerfile");
        std::fs::write(&dockerfile, "FROM scratch\n").unwrap();
        let git = git2::Repository::open(&repo).unwrap();
        let revision = commit(&git, "worker/Dockerfile");
        if had_edits {
            std::fs::write(&dockerfile, "FROM scratch\nLABEL confirmed=first\n").unwrap();
        }
        let files = uncommitted::collect(&repo, &revision, profile.build.as_ref().unwrap()).unwrap();
        let confirmation = uncommitted::Confirmed { hash: files.hash() };
        assert_eq!(confirmation.hash.is_some(), had_edits);
        let mut request = Request::new(
            "historical".into(),
            repo.clone(),
            revision.clone(),
            profile,
            root.path().join("state"),
            settings,
        );
        request.working_copy = Some(confirmation.clone());
        deployment::prepare_image_record(&request).unwrap();
        std::fs::write(repo.join("README.md"), "A later commit outside the build context\n").unwrap();
        assert_ne!(commit(&git, "README.md"), revision);
        request.working_copy = None;
        let before = std::fs::read(request.state_root.join("deployment.json")).unwrap();
        std::fs::write(&dockerfile, "FROM scratch\nLABEL unconfirmed=later\n").unwrap();
        let cancel = Cancellation::default();
        let build_started = std::cell::Cell::new(false);
        let emit = |event| {
            if matches!(event, Event::Stage(Stage::Build, _)) {
                build_started.set(true);
                cancel.cancel();
            }
        };
        let error = super::super::super::prepare_image(&request, &cancel, &emit).unwrap_err();
        assert!(
            matches!(error, Error::Invalid(message) if message.contains("uncommitted files") && message.contains("changed")),
            "{error}"
        );
        assert!(
            !build_started.get(),
            "the retry must refuse before Docker or release lookups"
        );
        let saved = Store::lock(&request.state_root).unwrap().load().unwrap().unwrap();
        assert_eq!(saved.working_copy, Some(confirmation));
        assert_eq!(
            std::fs::read(request.state_root.join("deployment.json")).unwrap(),
            before
        );
        assert!(!request.settings.docker_config.exists());
    }
}
