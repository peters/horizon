use super::*;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
#[cfg(unix)]
fn a_host_login_requires_verification_and_keeps_the_verified_pull_material() {
    let (root, mut settings) = fixture();
    binding(&mut settings).pull.expires_at = Some("2999-01-01T00:00:00Z".into());
    let mut prepared = Prepared::for_image(&settings, &image(), None, false).unwrap().unwrap();
    assert!(prepared.pull_expires_at().is_err());
    assert!(
        prepared.host_login().is_err(),
        "unverified material cannot reach a host"
    );
    assert_eq!(
        std::fs::metadata(prepared.docker_config(false))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let script = root.path().join("docker-fixture");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s' '{{\"digest\":\"sha256:{}\"}}'\n",
            "a".repeat(64)
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    prepared
        .verify_image_with(&image(), &Cancellation::default(), Command::new(&script))
        .unwrap();
    let expiry = prepared.pull_expires_at().unwrap();
    assert_eq!(expiry, Some(parse_expiry("2999-01-01T00:00:00Z").unwrap()));
    let path = binding(&mut settings).pull.secret_file.clone();
    std::fs::write(&path, "another-account-pull").unwrap();
    binding(&mut settings).pull.expires_at = Some("2000-01-01T00:00:00Z".into());
    assert_eq!(
        prepared.pull_expires_at().unwrap(),
        expiry,
        "expiry stays with the verified operation material"
    );
    let login = prepared.host_login().unwrap();
    assert_eq!(login.server, "registry.example");
    assert_eq!(login.username, "reader");
    let plan = horizon_cloud::host::Plan {
        image: image(),
        environment: std::collections::BTreeMap::new(),
        registry: Some(login),
        workspace_device: "/dev/disk/by-id/scsi-synthetic".into(),
        shm_gb: 1,
        exposure_ingress: false,
        platform: horizon_cloud::host::Platform::Prepared,
        purpose: horizon_cloud::host::Purpose::Worker,
    };
    let yaml = plan.cloud_config().unwrap();
    let document: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
    let entry = document["write_files"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|entry| entry["path"].as_str() == Some("/root/.docker/config.json"))
        .unwrap();
    let json = base64::engine::general_purpose::STANDARD
        .decode(entry["content"].as_str().unwrap())
        .unwrap();
    let config: serde_json::Value = serde_json::from_slice(&json).unwrap();
    let auth = base64::engine::general_purpose::STANDARD
        .decode(config["auths"]["registry.example"]["auth"].as_str().unwrap())
        .unwrap();
    assert_eq!(auth, b"reader:synthetic-pull");
    assert!(!String::from_utf8(json).unwrap().contains("synthetic-push"));
    assert!(root.path().join("registry").exists());
}

#[test]
fn an_unconfirmed_or_expired_binding_cannot_supply_a_host_login() {
    let (_root, mut settings) = fixture();
    binding(&mut settings).read_only_confirmed = false;
    assert!(binding(&mut settings).host_login().is_err());
    binding(&mut settings).read_only_confirmed = true;
    binding(&mut settings).pull.expires_at = Some("2000-01-01T00:00:00Z".into());
    assert!(binding(&mut settings).host_login().is_err());
}

#[test]
fn verified_expiry_retains_issuer_only_and_the_earlier_known_deadline() {
    for (declared, observed, expected) in [
        (None, Some("2030-01-01T00:00:00Z"), Some("2030-01-01T00:00:00Z")),
        (
            Some("2031-01-01T00:00:00Z"),
            Some("2030-01-01T00:00:00Z"),
            Some("2030-01-01T00:00:00Z"),
        ),
        (
            Some("2030-01-01T00:00:00Z"),
            Some("2031-01-01T00:00:00Z"),
            Some("2030-01-01T00:00:00Z"),
        ),
        (None, None, None),
    ] {
        let (_root, mut settings) = fixture();
        binding(&mut settings).pull.expires_at = declared.map(str::to_owned);
        let mut prepared = Prepared::for_image(&settings, &image(), None, false).unwrap().unwrap();
        // Supply the current issuer observation at the verification boundary without network access.
        prepared
            .journal
            .record_validation(Validation {
                image: image(),
                scope: "synthetic issuer-verified read-only".into(),
                expires_at: observed.map(str::to_owned),
                checked_at: time::OffsetDateTime::now_utc().unix_timestamp(),
            })
            .unwrap();
        assert!(
            prepared.pull_expires_at().is_err(),
            "old journal metadata alone is not current verification"
        );
        prepared.verified = true;
        std::fs::remove_file(&binding(&mut settings).pull.secret_file).unwrap();
        assert_eq!(
            prepared.pull_expires_at().unwrap(),
            expected.map(|value| parse_expiry(value).unwrap())
        );
        let expired = "2000-01-01T00:00:00Z";
        prepared
            .journal
            .record_validation(Validation {
                image: image(),
                scope: "synthetic issuer observation".into(),
                expires_at: Some(expired.into()),
                checked_at: 0,
            })
            .unwrap();
        assert_eq!(
            prepared.pull_expires_at().unwrap(),
            Some(parse_expiry(expired).unwrap()),
            "an elapsed issuer deadline must reach the allocation guard"
        );
    }
}

#[test]
fn host_logins_preserve_explicit_default_and_zero_padded_ports() {
    use base64::Engine as _;
    for authority in [
        "registry.example:443",
        "registry.example:0443",
        "registry.example:05000",
        "registry.example:5000",
        "localhost:05000",
    ] {
        let (_root, mut settings) = fixture();
        let repository = format!("{authority}/team/worker");
        binding(&mut settings).repository.clone_from(&repository);
        let image = format!("{repository}@sha256:{}", "a".repeat(64));
        let login = binding(&mut settings).host_login().unwrap();
        assert_eq!(login.server, authority);
        horizon_cloud::hetzner::cloud::check_login(Some(&login), true, &image).unwrap();
        let plan = horizon_cloud::host::Plan {
            image,
            environment: std::collections::BTreeMap::new(),
            registry: Some(login),
            workspace_device: "/dev/disk/by-id/scsi-synthetic".into(),
            shm_gb: 1,
            exposure_ingress: false,
            platform: horizon_cloud::host::Platform::Prepared,
            purpose: horizon_cloud::host::Purpose::Worker,
        };
        let document: serde_yaml::Value = serde_yaml::from_str(&plan.cloud_config().unwrap()).unwrap();
        let entry = document["write_files"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|entry| entry["path"].as_str() == Some("/root/.docker/config.json"))
            .unwrap();
        let json = base64::engine::general_purpose::STANDARD
            .decode(entry["content"].as_str().unwrap())
            .unwrap();
        let config: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert!(
            config["auths"].get(authority).is_some(),
            "the image's authority is the Docker login key"
        );
    }
}

#[test]
fn unsupported_host_authority_spellings_fail_before_provisioning() {
    let (_root, mut settings) = fixture();
    for repository in ["REGISTRY.EXAMPLE/team/worker", "registry.example./team/worker"] {
        binding(&mut settings).repository = repository.into();
        assert!(binding(&mut settings).host_login().is_err());
    }
}
