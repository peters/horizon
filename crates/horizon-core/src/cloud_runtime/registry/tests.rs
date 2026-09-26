use super::*;
#[cfg(unix)]
use base64::Engine as _;

#[test]
fn github_pull_grants_reject_write_delete_and_unknown_scopes() {
    assert!(credentials::verify_github_scopes(Some("read:packages")).is_ok());
    for scopes in [
        None,
        Some(""),
        Some("read:packages, write:packages"),
        Some("read:packages, delete:packages"),
        Some("repo, read:packages"),
    ] {
        assert!(credentials::verify_github_scopes(scopes).is_err());
    }
}

fn private_file(root: &Path, name: &str, value: &str) -> PathBuf {
    let file = tempfile::NamedTempFile::new_in(root).unwrap();
    std::fs::write(file.path(), value).unwrap();
    let path = root.join(name);
    file.persist(&path).unwrap();
    path
}

fn fixture() -> (tempfile::TempDir, Settings) {
    let root = tempfile::tempdir().unwrap();
    let path = |name| root.path().join(name);
    let compute = private_file(root.path(), "compute", "synthetic-compute");
    let pull = private_file(root.path(), "pull", "synthetic-pull");
    let push = private_file(root.path(), "push", "synthetic-push");
    let settings = serde_json::from_value(serde_json::json!({
        "runpod_key_file":compute,"ssh_identity_file":path("identity"),"docker_config":path("legacy-docker"),
        "registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[],
        "registries":{"root":path("registry"),"bindings":[{
            "repository":"registry.example/team/worker", "generation":"generation1", "read_only_confirmed":true,
            "pull":{"username":"reader","secret_file":pull,"expires_at":null},
            "publish":{"username":"writer","secret_file":push,"expires_at":null}
        }]}
    }))
    .unwrap();
    (root, settings)
}

fn binding(settings: &mut Settings) -> &mut Binding {
    &mut settings.registries.as_mut().unwrap().bindings[0]
}

#[test]
fn new_pull_inputs_follow_provider_limit_without_restricting_publish_or_legacy_status() {
    let (_root, mut settings) = fixture();
    let binding = binding(&mut settings);
    binding.publish.as_mut().unwrap().username = "p".repeat(256);
    for length in [191, 192] {
        binding.pull.username = "r".repeat(length);
        assert!(binding.validate().is_ok());
        assert_eq!(draft::Draft::from_binding(binding).validate().is_ok(), length == 191);
    }
    binding.pull.username = "é".repeat(191);
    assert!(binding.validate().is_ok());
    let draft = draft::Draft::from_binding(binding);
    assert!(draft.validate().is_ok());
    let saved = draft.save(|_, _| panic!("saved secrets are reused")).unwrap();
    assert!(saved.validate().is_ok());
    assert!(credentials::Material::load(&saved.pull, &saved.repository, None).is_ok());
    binding.pull.username = "é".repeat(192);
    assert!(binding.validate().is_err());
    assert!(draft::Draft::from_binding(binding).validate().is_err());
    binding.pull.username = "r".repeat(256);
    assert!(binding.validate().is_ok());
    binding.pull.username = "reader".into();
    for username in ["p".repeat(257), "é".repeat(129)] {
        binding.publish.as_mut().unwrap().username = username;
        assert!(binding.validate().is_err());
        assert!(draft::Draft::from_binding(binding).validate().is_err());
    }
}
fn image() -> String {
    format!("registry.example/team/worker@sha256:{}", "a".repeat(64))
}

#[test]
fn exact_scope_rejects_neighbors_and_preserves_unbound_public_registries() {
    let (_root, settings) = fixture();
    let config = settings.registries.as_ref().unwrap();
    assert!(config.select(&image()).unwrap().is_some());
    assert!(config.select("registry.example/team/other:latest").is_err());
    assert!(config.select("registry.example/team/worker-evil:latest").is_err());
    for reference in [
        "ubuntu:22.04",
        "namespace/worker:tag",
        "elsewhere.example/team/worker:tag",
    ] {
        assert!(config.select(reference).unwrap().is_none());
    }
}

#[test]
fn duplicate_repository_aliases_are_rejected_but_distinct_paths_and_ports_are_allowed() {
    for (first, alias) in [
        ("docker.io/team/worker", "index.docker.io/team/worker"),
        ("ghcr.io/team/worker", "GHCR.IO./team/worker"),
        ("ghcr.io/team/worker", "ghcr.io:0443/team/worker"),
        ("localhost:5000/team/worker", "localhost:05000/team/worker"),
    ] {
        let (_root, mut settings) = fixture();
        let config = settings.registries.as_mut().unwrap();
        config.bindings[0].repository = first.into();
        let mut duplicate = config.bindings[0].clone();
        duplicate.repository = alias.into();
        duplicate.generation = "generation2".into();
        config.bindings.push(duplicate);
        assert!(config.validate().is_err(), "{first} and {alias}");
        config.bindings[1].repository = format!("{alias}-other");
        assert!(config.validate().is_ok());
        config.bindings[1].repository = "ghcr.io:5001/team/worker".into();
        assert!(config.validate().is_ok());
    }
}

#[test]
fn independent_registry_ports_retain_their_own_scope() {
    let (_root, mut settings) = fixture();
    binding(&mut settings).repository = "localhost:5000/team/worker".into();
    let config = settings.registries.as_ref().unwrap();
    assert!(config.select("localhost:5000/team/worker:tag").unwrap().is_some());
    assert!(config.select("localhost:5000/team/other:tag").is_err());
    assert!(config.select("localhost:05000/team/other:tag").is_err());
    assert!(config.select("localhost:5001/team/other:tag").unwrap().is_none());
    binding(&mut settings).repository = "registry.example:443/team/worker".into();
    let config = settings.registries.as_ref().unwrap();
    assert!(config.select("registry.example/team/other:tag").is_err());
    assert!(config.select("registry.example:0443/team/other:tag").is_err());
    assert!(config.select("REGISTRY.EXAMPLE.:443/team/other:tag").is_err());
    assert!(config.select("registry.example:5000/team/other:tag").unwrap().is_none());
}

#[test]
fn empty_repository_components_are_rejected_before_saving() {
    let (_root, mut settings) = fixture();
    for repository in [
        "ghcr.io/",
        "ghcr.io/team/",
        "ghcr.io//worker",
        "ghcr.io/./worker",
        "ghcr.io/team/../worker",
    ] {
        binding(&mut settings).repository = repository.into();
        assert!(settings.registries.as_ref().unwrap().validate().is_err());
        assert!(draft::Draft::from_binding(binding(&mut settings)).validate().is_err());
    }
}

#[test]
fn rejected_registry_root_does_not_create_directories_in_source() {
    let (root, mut settings) = fixture();
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let rejected = source.join("nested/registry");
    settings.registries.as_mut().unwrap().root = rejected;
    assert!(Prepared::for_image(&settings, &image(), Some(&source), false).is_err());
    assert_eq!(std::fs::read_dir(&source).unwrap().count(), 0);
    #[cfg(unix)]
    {
        let alias = root.path().join("source-alias");
        std::os::unix::fs::symlink(&source, &alias).unwrap();
        settings.registries.as_mut().unwrap().root = alias.join("nested/registry");
        assert!(Prepared::for_image(&settings, &image(), Some(&source), false).is_err());
        assert_eq!(std::fs::read_dir(&source).unwrap().count(), 0);
    }
}

#[test]
fn scope_authorization_uses_the_actual_secret_without_debug_masking() {
    let (_root, settings) = fixture();
    let binding = &settings.registries.as_ref().unwrap().bindings[0];
    let material = credentials::Material::load(&binding.pull, &binding.repository, None).unwrap();
    let header = material.authorization_header();
    assert_eq!(header.split_once(' '), Some(("Bearer", "synthetic-pull")));
}

#[test]
#[cfg(unix)]
fn existing_registry_root_permissions_are_verified_without_mutation() {
    use std::os::unix::fs::PermissionsExt;
    let (root, mut settings) = fixture();
    let shared = root.path().join("shared");
    std::fs::create_dir(&shared).unwrap();
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).unwrap();
    settings.registries.as_mut().unwrap().root = shared.clone();
    let action = Action::Status {
        repository: "registry.example/team/worker".into(),
        generation: "generation1".into(),
    };
    assert!(manage(&settings, &action, &Cancellation::default()).is_err());
    assert_eq!(std::fs::metadata(&shared).unwrap().permissions().mode() & 0o777, 0o755);
    assert_eq!(std::fs::read_dir(&shared).unwrap().count(), 0);
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&shared, &alias).unwrap();
    settings.registries.as_mut().unwrap().root = alias;
    assert!(manage(&settings, &action, &Cancellation::default()).is_err());
    assert_eq!(std::fs::metadata(&shared).unwrap().permissions().mode() & 0o777, 0o755);
    let fresh = root.path().join("fresh/registry");
    settings.registries.as_mut().unwrap().root = fresh.clone();
    assert!(manage(&settings, &action, &Cancellation::default()).is_ok());
    assert_eq!(std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777, 0o700);
}

#[test]
fn generations_must_be_persisted_and_metadata_contains_no_secret() {
    let (_root, settings) = fixture();
    let json = serde_json::to_string(&settings).unwrap();
    assert!(!json.contains("synthetic-pull"));
    let mut json = serde_json::to_value(&settings).unwrap();
    json["registries"]["bindings"][0]
        .as_object_mut()
        .unwrap()
        .remove("generation");
    assert!(serde_json::from_value::<Settings>(json).is_err());
}

#[test]
fn expired_or_shared_pull_grants_fail_before_provider_contact() {
    let (_root, mut settings) = fixture();
    binding(&mut settings).pull.expires_at = Some("2000-01-01T00:00:00Z".into());
    assert!(Prepared::for_image(&settings, &image(), None, false).is_err());
    binding(&mut settings).pull.expires_at = None;
    let push = binding(&mut settings).publish.as_ref().unwrap().secret_file.clone();
    binding(&mut settings).pull.secret_file = push;
    assert!(Prepared::for_image(&settings, &image(), None, false).is_err());
}

#[test]
fn secrets_in_source_and_unconfirmed_pull_grants_are_refused() {
    let (root, mut settings) = fixture();
    assert!(Prepared::for_image(&settings, &image(), Some(root.path()), false).is_err());
    binding(&mut settings).read_only_confirmed = false;
    assert!(Prepared::for_image(&settings, &image(), None, false).is_err());
}

#[test]
#[cfg_attr(
    windows,
    ignore = "Cloud journals require Unix directory durability, matching deployment support"
)]
fn pull_only_work_does_not_require_a_live_publishing_credential() {
    let (_root, mut settings) = fixture();
    binding(&mut settings).publish.as_mut().unwrap().expires_at = Some("2000-01-01T00:00:00Z".into());
    assert!(Prepared::for_image(&settings, &image(), None, false).is_ok());
    assert!(Prepared::for_image(&settings, &image(), None, true).is_err());
    std::fs::remove_file(&binding(&mut settings).publish.as_ref().unwrap().secret_file).unwrap();
    assert!(Prepared::for_image(&settings, &image(), None, false).is_ok());
}

#[test]
#[cfg_attr(
    windows,
    ignore = "Cloud journals require Unix directory durability, matching deployment support"
)]
fn generation_fences_changed_login_and_survives_reload() {
    let (_root, mut settings) = fixture();
    let mut prepared = Prepared::for_image(&settings, &image(), None, false).unwrap().unwrap();
    prepared
        .journal
        .save(&State::Requested {
            name: "horizon-pull-generation1".into(),
        })
        .unwrap();
    assert!(matches!(
        Prepared::for_image(&settings, &image(), None, false),
        Err(Error::Busy)
    ));
    drop(prepared);
    binding(&mut settings).pull.username = "another-reader".into();
    assert!(Prepared::for_image(&settings, &image(), None, false).is_err());
    binding(&mut settings).pull.username = "reader".into();
    let mut prepared = Prepared::for_image(&settings, &image(), None, false).unwrap().unwrap();
    assert!(matches!(prepared.journal.state(), State::Requested { .. }));
    prepared.journal.save(&State::Revoked).unwrap();
    drop(prepared);
    assert!(Prepared::for_image(&settings, &image(), None, false).is_err());
}

#[test]
fn rotation_retains_old_handles_and_does_not_change_old_material() {
    let (root, mut settings) = fixture();
    let original = binding(&mut settings).clone();
    let mut draft = draft::Draft::from_binding(&original);
    assert!(draft.is_saved());
    draft.pull_secret = zeroize::Zeroizing::new("replacement-pull".into());
    assert!(!draft.is_saved());
    let rotated = draft
        .save(|name, value| Ok(private_file(root.path(), name, value)))
        .unwrap();
    assert_ne!(rotated.generation, original.generation);
    assert_eq!(rotated.retired, [original.generation]);
    assert_eq!(
        std::fs::read_to_string(original.pull.secret_file).unwrap(),
        "synthetic-pull"
    );
    assert_eq!(
        std::fs::read_to_string(rotated.pull.secret_file).unwrap(),
        "replacement-pull"
    );
}

#[test]
#[cfg_attr(
    windows,
    ignore = "Cloud journals require Unix directory durability, matching deployment support"
)]
fn provider_transfer_requires_completed_image_validation() {
    let (_root, settings) = fixture();
    let mut prepared = Prepared::for_image(&settings, &image(), None, false).unwrap().unwrap();
    let provider = RunPod::new(settings.credential().unwrap());
    assert!(prepared.ensure_provider(&provider, &Cancellation::default()).is_err());
}

#[cfg(unix)]
#[test]
fn immutable_pull_probe_uses_only_the_pull_config_and_handles_failure_and_cancellation() {
    use std::os::unix::fs::PermissionsExt;
    let (root, mut settings) = fixture();
    settings.docker_host = Some("unix:///synthetic/docker.sock".into());
    let mut prepared = Prepared::for_image(&settings, &image(), None, true).unwrap().unwrap();
    assert_ne!(prepared.docker_config(true), prepared.docker_config(false));
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(prepared.docker_config(false).join("config.json")).unwrap()).unwrap();
    let auth = base64::engine::general_purpose::STANDARD
        .decode(config["auths"]["registry.example"]["auth"].as_str().unwrap())
        .unwrap();
    assert_eq!(String::from_utf8(auth).unwrap(), "reader:synthetic-pull");
    let script = root.path().join("docker-fixture");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n[ -z \"${{DOCKER_AUTH_CONFIG+x}}\" ] || exit 2\n[ \"$1\" = --host ] && [ \"$2\" = unix:///synthetic/docker.sock ] || exit 1\nprintf '%s' '{{\"digest\":\"sha256:{}\"}}'\n",
            "a".repeat(64)
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    prepared
        .verify_image_with(&image(), &Cancellation::default(), {
            let mut command = Command::new(&script);
            command.env("DOCKER_AUTH_CONFIG", "ambient-credential");
            command
        })
        .unwrap();
    assert!(prepared.verified);
    assert_eq!(prepared.journal.validation().unwrap().image, image());
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(
        prepared
            .verify_image_with(&image(), &cancel, Command::new(&script))
            .is_err()
    );
    assert!(!prepared.verified);
    std::fs::write(&script, "#!/bin/sh\n[ \"$1\" = --host ] && [ \"$2\" = unix:///synthetic/docker.sock ] || exit 1\nprintf synthetic-pull >&2\nexit 1\n").unwrap();
    let error = prepared
        .verify_image_with(&image(), &Cancellation::default(), Command::new(&script))
        .unwrap_err();
    assert!(!error.to_string().contains("synthetic-pull"));
    assert!(!prepared.verified);
}

#[test]
fn pull_only_deployment_still_excludes_the_publishing_secret_from_source() {
    let (root, mut settings) = fixture();
    let source = root.path().join("repository");
    std::fs::create_dir(&source).unwrap();
    let path = private_file(&source, "push-token", "synthetic-push");
    binding(&mut settings).publish.as_mut().unwrap().secret_file = path;
    assert!(matches!(
        Prepared::for_image(&settings, &image(), Some(&source), false),
        Err(Error::Invalid(
            "Registry secrets must be outside the source and build context"
        ))
    ));
}

#[test]
fn equivalent_issuer_hosts_cannot_bypass_scope_policy() {
    for reference in [
        "ghcr.io/team/worker",
        "ghcr.io:443/team/worker",
        "GHCR.IO.:0443/team/worker",
        "GHCR.IO/team/worker",
        "ghcr.io./team/worker",
    ] {
        assert!(credentials::is_github_registry(reference));
    }
    for reference in ["ghcr.io.example/team/worker", "ghcr.io:5000/team/worker"] {
        assert!(!credentials::is_github_registry(reference));
    }
    let (_root, settings) = fixture();
    let auth = &settings.registries.as_ref().unwrap().bindings[0].pull;
    let material = Material::load(auth, "ghcr.io:5000/team/worker", None).unwrap();
    assert_eq!(
        material
            .verify_scope("ghcr.io:5000/team/worker", &Cancellation::default())
            .unwrap(),
        None
    );
    let (_root, mut settings) = fixture();
    binding(&mut settings).repository = "ghcr.io/team/worker".into();
    assert!(
        settings
            .registries
            .as_ref()
            .unwrap()
            .select("ghcr.io:443/team/other:latest")
            .is_err()
    );
}

#[test]
fn docker_hub_material_uses_the_canonical_login_key() {
    let (_root, settings) = fixture();
    let auth = &settings.registries.as_ref().unwrap().bindings[0].pull;
    let material = Material::load(auth, "docker.io/team/worker", None).unwrap();
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(material.config.path().join("config.json")).unwrap()).unwrap();
    assert!(config["auths"]["https://index.docker.io/v1/"]["auth"].is_string());
    assert_eq!(config["auths"].as_object().unwrap().len(), 1);
    assert_eq!(
        credentials::docker_auth_key("registry.example:5000/team/worker"),
        "registry.example:5000"
    );
}

#[test]
fn duplicate_current_and_retired_generations_are_rejected_globally() {
    let (_root, mut settings) = fixture();
    let mut second = binding(&mut settings).clone();
    second.repository = "registry.example/team/other".into();
    let config = settings.registries.as_mut().unwrap();
    config.bindings.push(second);
    assert!(config.validate().is_err());
    config.bindings[1].generation = "generation2".into();
    assert!(config.validate().is_ok());
    config.bindings[1].retired.push("generation1".into());
    assert!(config.validate().is_err());
}

#[cfg(unix)]
#[test]
fn status_exposes_configured_expiry_before_verification_without_mislabeling_retired_grants() {
    let (_root, mut settings) = fixture();
    binding(&mut settings).pull.expires_at = Some("2030-01-01T00:00:00Z".into());
    binding(&mut settings).retired.push("previous".into());
    let status = |generation: &str| {
        manage(
            &settings,
            &Action::Status {
                repository: "registry.example/team/worker".into(),
                generation: generation.into(),
            },
            &Cancellation::default(),
        )
        .unwrap()
    };
    let current = status("generation1");
    assert!(current.validation.is_none());
    assert_eq!(current.configured_pull_expiry.as_deref(), Some("2030-01-01T00:00:00Z"));
    assert!(status("previous").configured_pull_expiry.is_none());
}

#[test]
fn docker_hub_aliases_and_shorthand_cannot_escape_explicit_bindings() {
    for bound in ["docker.io/team/worker", "index.docker.io/team/worker"] {
        let (_root, mut settings) = fixture();
        binding(&mut settings).repository = bound.into();
        let config = settings.registries.as_ref().unwrap();
        assert!(config.select(&format!("{bound}:latest")).unwrap().is_some());
        for image in [
            "docker.io/team/other:latest",
            "index.docker.io/team/other:latest",
            "team/worker:latest",
            "ubuntu:latest",
        ] {
            assert!(config.select(image).is_err(), "{bound}: {image}");
        }
    }
}

#[cfg(unix)]
#[test]
fn cancelled_pull_probe_preserves_cancellation_instead_of_reporting_authentication_failure() {
    use std::os::unix::fs::PermissionsExt;
    let (root, settings) = fixture();
    let mut prepared = Prepared::for_image(&settings, &image(), None, false).unwrap().unwrap();
    let script = root.path().join("slow-probe");
    std::fs::write(&script, "#!/bin/sh\nsleep 5\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let cancel = Cancellation::default();
    let trigger = cancel.clone();
    let cancellation = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        trigger.cancel();
    });
    let error = prepared
        .verify_image_with(&image(), &cancel, Command::new(&script))
        .unwrap_err();
    cancellation.join().unwrap();
    assert!(matches!(error, Error::Provider(horizon_cloud::CloudError::Cancelled)));
    assert!(!prepared.verified);
    assert!(prepared.journal.validation().is_none());
}

#[test]
fn partial_credential_replacement_cannot_save_the_unchanged_opposite_secret() {
    let (_root, mut settings) = fixture();
    let original = binding(&mut settings).clone();
    for replace_pull in [false, true] {
        let mut draft = draft::Draft::from_binding(&original);
        if replace_pull {
            draft.pull_secret = zeroize::Zeroizing::new("synthetic-push".into());
        } else {
            draft.publish_secret = zeroize::Zeroizing::new("synthetic-pull".into());
        }
        let mut writes = 0;
        assert!(
            draft
                .save(|_, _| {
                    writes += 1;
                    Err(Error::Invalid("must validate before writing"))
                })
                .is_err()
        );
        assert_eq!(writes, 0);
        if replace_pull {
            draft.pull_secret = zeroize::Zeroizing::new("replacement-pull".into());
        } else {
            draft.publish_secret = zeroize::Zeroizing::new("replacement-push".into());
        }
        assert!(draft.validate().is_ok());
    }
}
