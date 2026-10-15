use super::*;

#[test]
#[cfg_attr(windows, ignore = "Registry journals require Unix directory durability")]
fn a_direct_host_grant_needs_no_compute_key_and_cannot_create_provider_access() {
    let (root, mut settings) = fixture();
    std::fs::remove_file(&settings.runpod_key_file).unwrap();
    let mut prepared =
        Prepared::for_image_for_provider(&settings, &image(), None, false, horizon_cloud::provider::Kind::Hetzner)
            .unwrap()
            .unwrap();
    assert!(root.path().join("registry/hosts/generation1.json").is_file());
    assert!(!root.path().join("registry/generation1.json").exists());
    let provider = RunPod::new(horizon_cloud::Credential::new("synthetic-unused-provider-key".into()).unwrap());
    assert!(prepared.ensure_provider(&provider, &Cancellation::default()).is_err());
    drop(prepared);
    settings.runpod_key_file = root.path().join("another-missing-key");
    let direct = Prepared::for_image_for_provider(
        &settings,
        &image(),
        None,
        false,
        horizon_cloud::provider::Kind::DockerHost,
    )
    .unwrap()
    .unwrap();
    drop(direct);
    assert!(
        Prepared::for_image(&settings, &image(), None, false).is_err(),
        "provider bindings still require their original account"
    );
}

#[test]
#[cfg_attr(windows, ignore = "Registry journals require Unix directory durability")]
fn direct_host_and_provider_journals_keep_separate_identity_and_locks() {
    let (root, mut settings) = fixture();
    let direct =
        Prepared::for_image_for_provider(&settings, &image(), None, false, horizon_cloud::provider::Kind::Hetzner)
            .unwrap()
            .unwrap();
    let provider = Prepared::for_image(&settings, &image(), None, false).unwrap().unwrap();
    assert!(root.path().join("registry/hosts/generation1.json").is_file());
    assert!(root.path().join("registry/generation1.json").is_file());
    drop(direct);
    drop(provider);
    let push = binding(&mut settings).publish.as_ref().unwrap().secret_file.clone();
    std::fs::write(&binding(&mut settings).pull.secret_file, "another-read-only-actor").unwrap();
    assert!(
        Prepared::for_image_for_provider(&settings, &image(), None, false, horizon_cloud::provider::Kind::Hetzner)
            .is_err(),
        "in-place pull mutation requires a new generation"
    );
    assert!(push.exists());
}

#[test]
#[cfg_attr(windows, ignore = "Registry journals require Unix directory durability")]
fn the_first_direct_host_use_retains_an_earlier_provider_pull_fingerprint() {
    let (root, mut settings) = fixture();
    let provider = Prepared::for_image(&settings, &image(), None, false).unwrap().unwrap();
    drop(provider);
    let pull = binding(&mut settings).pull.secret_file.clone();
    let original = std::fs::read(&pull).unwrap();
    std::fs::write(&pull, "another-read-only-actor").unwrap();
    std::fs::remove_file(&settings.runpod_key_file).unwrap();
    assert!(
        Prepared::for_image_for_provider(&settings, &image(), None, false, horizon_cloud::provider::Kind::Hetzner)
            .is_err()
    );
    assert!(!root.path().join("registry/hosts/generation1.json").exists());
    std::fs::write(pull, original).unwrap();
    assert!(
        Prepared::for_image_for_provider(&settings, &image(), None, false, horizon_cloud::provider::Kind::Hetzner)
            .is_ok_and(|prepared| prepared.is_some())
    );
}

#[test]
#[cfg_attr(windows, ignore = "Registry journals require Unix directory durability")]
fn the_first_provider_use_retains_an_earlier_direct_host_pull_fingerprint() {
    let (root, mut settings) = fixture();
    let direct =
        Prepared::for_image_for_provider(&settings, &image(), None, false, horizon_cloud::provider::Kind::Hetzner)
            .unwrap()
            .unwrap();
    drop(direct);
    let pull = binding(&mut settings).pull.secret_file.clone();
    let original = std::fs::read(&pull).unwrap();
    std::fs::write(&pull, "another-read-only-actor").unwrap();
    assert!(Prepared::for_image(&settings, &image(), None, false).is_err());
    assert!(!root.path().join("registry/generation1.json").exists());
    std::fs::write(pull, original).unwrap();
    assert!(Prepared::for_image(&settings, &image(), None, false).is_ok_and(|prepared| prepared.is_some()));
}

#[test]
#[cfg_attr(windows, ignore = "Registry journals require Unix directory durability")]
fn concurrent_first_uses_cannot_pin_different_pull_material_for_one_generation() {
    let (root, first) = fixture();
    let mut second = first.clone();
    binding(&mut second).pull.secret_file = private_file(root.path(), "another-pull", "another-read-only-actor");
    let start = std::sync::Barrier::new(2);
    let (host_ok, provider_ok) = std::thread::scope(|scope| {
        let host = scope.spawn(|| {
            start.wait();
            Prepared::for_image_for_provider(&first, &image(), None, false, horizon_cloud::provider::Kind::Hetzner)
                .is_ok_and(|prepared| prepared.is_some())
        });
        let provider = scope.spawn(|| {
            start.wait();
            Prepared::for_image(&second, &image(), None, false).is_ok_and(|prepared| prepared.is_some())
        });
        (host.join().unwrap(), provider.join().unwrap())
    });
    assert_ne!(
        host_ok, provider_ok,
        "exactly one material can become the generation's first binding"
    );
    if host_ok {
        assert!(Prepared::for_image(&second, &image(), None, false).is_err());
    } else {
        assert!(
            Prepared::for_image_for_provider(&first, &image(), None, false, horizon_cloud::provider::Kind::Hetzner)
                .is_err()
        );
    }
}

#[test]
#[cfg_attr(windows, ignore = "Registry journals require Unix directory durability")]
fn concurrent_first_uses_of_the_same_pull_grant_both_succeed() {
    let (_root, settings) = fixture();
    let start = std::sync::Barrier::new(2);
    let (host_ok, provider_ok) = std::thread::scope(|scope| {
        let host = scope.spawn(|| {
            start.wait();
            Prepared::for_image_for_provider(&settings, &image(), None, false, horizon_cloud::provider::Kind::Hetzner)
                .is_ok_and(|prepared| prepared.is_some())
        });
        let provider = scope.spawn(|| {
            start.wait();
            Prepared::for_image(&settings, &image(), None, false).is_ok_and(|prepared| prepared.is_some())
        });
        (host.join().unwrap(), provider.join().unwrap())
    });
    assert!(
        host_ok && provider_ok,
        "independent provider operations can share the same generation's material"
    );
}

#[test]
#[cfg_attr(windows, ignore = "Registry journals require Unix directory durability")]
fn an_already_pinned_grant_does_not_wait_for_another_scopes_first_pin() {
    let (root, settings) = fixture();
    drop(
        Prepared::for_image_for_provider(&settings, &image(), None, false, horizon_cloud::provider::Kind::Hetzner)
            .unwrap(),
    );
    drop(Prepared::for_image(&settings, &image(), None, false).unwrap());
    let identity = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.path().join("registry/generation1.identity.lock"))
        .unwrap();
    identity.lock().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let before_unlock = std::thread::scope(|scope| {
        scope.spawn(|| {
            tx.send(Prepared::for_image(&settings, &image(), None, false).is_ok_and(|prepared| prepared.is_some()))
                .unwrap();
        });
        let result = rx.recv_timeout(Duration::from_secs(2));
        identity.unlock().unwrap();
        result
    });
    assert_eq!(before_unlock, Ok(true));
}

#[test]
#[cfg_attr(windows, ignore = "Registry journals require Unix directory durability")]
fn a_host_journal_cannot_import_provider_mutation_state() {
    let (root, settings) = fixture();
    drop(
        Prepared::for_image_for_provider(&settings, &image(), None, false, horizon_cloud::provider::Kind::Hetzner)
            .unwrap(),
    );
    let path = root.path().join("registry/hosts/generation1.json");
    let original: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for state in [
        State::Requested {
            name: "synthetic-request".into(),
        },
        State::Bound(horizon_cloud::runpod::registry::Binding {
            id: "synthetic-id".into(),
            name: "synthetic-request".into(),
        }),
        State::Revoked,
    ] {
        let mut changed = original.clone();
        changed["state"] = serde_json::to_value(state).unwrap();
        std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(
            Prepared::for_image_for_provider(&settings, &image(), None, false, horizon_cloud::provider::Kind::Hetzner)
                .is_err()
        );
        assert!(!root.path().join("registry/generation1.json").exists());
    }
}

#[test]
#[cfg_attr(windows, ignore = "Registry journals require Unix directory durability")]
fn an_unreadable_other_scope_cannot_be_ignored_while_pinning_a_generation() {
    let (root, settings) = fixture();
    drop(Prepared::for_image(&settings, &image(), None, false).unwrap());
    std::fs::write(root.path().join("registry/generation1.json"), b"not-json").unwrap();
    assert!(
        Prepared::for_image_for_provider(&settings, &image(), None, false, horizon_cloud::provider::Kind::Hetzner)
            .is_err()
    );
    assert!(!root.path().join("registry/hosts/generation1.json").exists());
}
