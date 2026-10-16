use super::*;
use horizon_cloud::hetzner::catalog::Offer;
use std::collections::BTreeMap;

/// Settings without a Hetzner binding.
fn no_hetzner() -> Settings {
    serde_json::from_value(serde_json::json!({
        "runpod_key_file": "/unused/key", "ssh_identity_file": "/unused/identity",
        "docker_config": "/unused/docker", "registry_pull_auth_id": null,
        "cpu_flavors": ["cpu3c"], "gpu_types": [], "data_centers": ["US-MO-2"],
    }))
    .unwrap()
}

fn offer(server_type: &str, location: &str, cores: u32, memory_gb: f64, hourly_eur: f64, available: bool) -> Offer {
    Offer {
        server_type: server_type.into(),
        location: location.into(),
        cores,
        memory_gb,
        disk_gb: 240,
        dedicated: false,
        hourly_eur,
        monthly_eur: hourly_eur * 730.0,
        available,
        recommended: false,
    }
}

#[test]
fn a_builder_is_large_enough_in_an_allowed_place_and_orderable_ones_come_first() {
    let catalog = Catalog {
        offers: vec![
            offer("cx23", "hel1", 2, 4.0, 0.006, true),
            offer("cx43", "hel1", 8, 16.0, 0.016, false),
            offer("cx53", "hel1", 16, 32.0, 0.030, true),
            offer("cx43", "nbg1", 8, 16.0, 0.016, true),
            offer("cx43", "ash", 8, 16.0, 0.010, true),
        ],
        volume_gb_month_eur: 0.04,
        ipv4_month_eur: BTreeMap::new(),
        ipv4_hour_eur: BTreeMap::from([("hel1".into(), 0.001), ("nbg1".into(), 0.001), ("ash".into(), 0.001)]),
        regions: BTreeMap::new(),
    };
    let (placements, hourly) = placements(&catalog, &["hel1".into(), "nbg1".into()]);
    let names: Vec<(&str, &str)> = placements
        .iter()
        .map(|placement| (placement.server_type.as_str(), placement.location.as_str()))
        .collect();
    assert_eq!(names, [("cx43", "nbg1"), ("cx53", "hel1"), ("cx43", "hel1")]);
    assert!((hourly[0].1 - 0.017).abs() < 1e-9, "the server and its IPv4 address");
}

#[test]
fn without_a_journal_nothing_is_given_back_and_no_provider_is_needed() {
    let root = tempfile::tempdir().unwrap();
    let settings = no_hetzner();
    let owner = claim(root.path()).unwrap().unwrap();
    assert!(!left_behind(root.path()));
    assert!(!give_back(&settings, &owner, &Cancellation::default()).unwrap());
    assert!(!available(&settings));
}

#[test]
fn a_journal_names_the_builder_until_it_is_proven_gone_and_is_private() {
    let root = tempfile::tempdir().unwrap();
    write_journal(
        root.path(),
        "setup-builder-0123456789ab",
        &CreateState::Requested,
        Some(now()),
    )
    .unwrap();
    let owner = claim(root.path()).unwrap().unwrap();
    assert!(left_behind(root.path()));
    let text = std::fs::read_to_string(root.path().join(JOURNAL)).unwrap();
    let journal: Journal = serde_json::from_str(&text).unwrap();
    assert_eq!(journal.state, CreateState::Requested);
    assert!(journal.requested_at.is_some());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(root.path().join(JOURNAL))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // Without the Hetzner binding the builder cannot be deleted, so the journal stays.
    assert!(give_back(&no_hetzner(), &owner, &Cancellation::default()).is_err());
    assert!(left_behind(root.path()));
    assert!(matches!(
        rent(&no_hetzner(), &owner, &Cancellation::default(), |_| {}),
        Err(Error::Invalid(_))
    ));
}

#[cfg(unix)]
#[test]
fn the_keys_are_new_ed25519_pairs_in_a_private_folder() {
    use std::os::unix::fs::PermissionsExt;
    if Command::new("ssh-keygen").arg("-?").output().is_err() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join(FOLDER);
    let keys = Keys::make(&folder).unwrap();
    assert_eq!(std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777, 0o700);
    for path in [&keys.login, &keys.host] {
        let public = std::fs::read_to_string(path.with_extension("pub")).unwrap();
        assert!(public.starts_with("ssh-ed25519 "));
        assert!(
            std::fs::read_to_string(path)
                .unwrap()
                .contains("BEGIN OPENSSH PRIVATE KEY")
        );
    }
    let first = std::fs::read_to_string(keys.login.with_extension("pub")).unwrap();
    let again = Keys::make(&folder).unwrap();
    assert_ne!(
        first,
        std::fs::read_to_string(again.login.with_extension("pub")).unwrap(),
        "each builder gets new keys"
    );
}

#[test]
fn one_process_owns_the_builder_of_a_cloud_root_until_it_lets_go() {
    let root = tempfile::tempdir().unwrap();
    let owner = claim(root.path()).unwrap();
    assert!(owner.is_some());
    assert!(
        claim(root.path()).unwrap().is_none(),
        "a second claim waits for the first"
    );
    drop(owner);
    assert!(claim(root.path()).unwrap().is_some());
}
