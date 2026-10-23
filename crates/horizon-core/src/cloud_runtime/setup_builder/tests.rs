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
    let (placements, hourly) = placements(&catalog, &[], &["hel1".into(), "nbg1".into()]);
    let names: Vec<(&str, &str)> = placements
        .iter()
        .map(|placement| (placement.server_type.as_str(), placement.location.as_str()))
        .collect();
    assert_eq!(names, [("cx43", "nbg1"), ("cx53", "hel1"), ("cx43", "hel1")]);
    assert!((hourly[0].1 - 0.017).abs() < 1e-9, "the server and its IPv4 address");
}

#[test]
fn the_configured_server_types_decide_which_machines_are_rented() {
    let catalog = Catalog {
        offers: vec![
            offer("cpx42", "fsn1", 8, 16.0, 0.112, true),
            offer("ccx33", "fsn1", 8, 32.0, 0.190, true),
            offer("cpx52", "fsn1", 12, 24.0, 0.170, true),
        ],
        volume_gb_month_eur: 0.04,
        ipv4_month_eur: BTreeMap::new(),
        ipv4_hour_eur: BTreeMap::from([("fsn1".into(), 0.001)]),
        regions: BTreeMap::new(),
    };
    let chosen = |server_types: &[&str]| {
        placements(
            &catalog,
            &server_types.iter().map(|name| (*name).to_owned()).collect::<Vec<_>>(),
            &["fsn1".into()],
        )
        .0
        .iter()
        .map(|placement| placement.server_type.clone())
        .collect::<Vec<_>>()
    };
    assert_eq!(
        chosen(&[]),
        ["cpx42", "cpx52", "ccx33"],
        "every type when none is chosen"
    );
    assert_eq!(
        chosen(&["ccx33"]),
        ["ccx33"],
        "a project that cannot order the cheapest type rents the one the person chose"
    );
    assert_eq!(
        chosen(&["ccx33", "cpx52"]),
        ["cpx52", "ccx33"],
        "cheapest first among the chosen ones"
    );
    assert!(
        chosen(&["cpx22"]).is_empty(),
        "a type too small for a builder is not rented"
    );
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
        ("setup-builder-0123456789ab", "0011223344556677"),
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

/// Settings with a Hetzner binding whose token file in `folder` holds `token`.
#[cfg(unix)]
fn with_hetzner(folder: &Path, token: &str) -> Settings {
    use std::os::unix::fs::OpenOptionsExt as _;
    let token_file = folder.join("hetzner-token");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&token_file)
        .unwrap();
    std::io::Write::write_all(&mut file, token.as_bytes()).unwrap();
    let mut settings = no_hetzner();
    settings.hetzner = Some(
        serde_json::from_value(serde_json::json!({
            "token_file": token_file, "server_types": ["cx43"], "locations": ["hel1"],
        }))
        .unwrap(),
    );
    settings
}

/// A journal of a builder rented with the token whose fingerprint is `binding`.
#[cfg(unix)]
fn journal_of(root: &Path, binding: &str) -> Journal {
    write_journal(
        root,
        ("setup-builder-0123456789ab", binding),
        &CreateState::Bound { worker_id: "42".into() },
        Some(now()),
    )
    .unwrap();
    serde_json::from_str(&std::fs::read_to_string(root.join(JOURNAL)).unwrap()).unwrap()
}

#[cfg(unix)]
fn listing(key: &str, items: &serde_json::Value) -> String {
    let mut page = serde_json::json!({"meta": {"pagination": {"next_page": null}}});
    page[key] = items.clone();
    page.to_string()
}

#[cfg(unix)]
fn server(id: u64) -> serde_json::Value {
    serde_json::json!({"id": id, "name": "horizon-setup-builder", "status": "running",
        "public_net": {"ipv4": {"ip": "192.0.2.10"}, "ipv6": null},
        "server_type": {"name": "cpx42", "cores": 8, "memory": 16.0, "disk": 320},
        "location": {"name": "fsn1"},
        "labels": {"horizon-operation": "setup-builder-0123456789ab"}, "volumes": []})
}

#[cfg(unix)]
#[test]
fn the_token_fingerprint_never_holds_the_token() {
    let root = tempfile::tempdir().unwrap();
    let settings = with_hetzner(root.path(), "token-of-project-a");
    let binding = settings
        .hetzner
        .as_ref()
        .unwrap()
        .credential_with_fingerprint()
        .unwrap()
        .1;
    assert_eq!(binding.len(), 16);
    assert!(binding.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let other = with_hetzner(root.path(), "token-of-project-b");
    assert_ne!(
        other.hetzner.as_ref().unwrap().credential_with_fingerprint().unwrap().1,
        binding
    );
}

#[cfg(unix)]
#[test]
fn a_token_that_does_not_list_the_builder_keeps_the_journal() {
    use crate::cloud_runtime::deployment::hetzner::tests::provider::serve;
    let root = tempfile::tempdir().unwrap();
    let journal = journal_of(root.path(), "0011223344556677");
    // Another project's token lists neither the server nor its key: that is no proof
    // that the builder is gone.
    let (address, requests, task) = serve(vec![
        (200, listing("servers", &serde_json::json!([]))),
        (200, listing("ssh_keys", &serde_json::json!([]))),
    ]);
    let client = Hetzner::loopback(horizon_cloud::Credential::new("other".into()).unwrap(), address).unwrap();
    let refused = return_with(
        root.path(),
        journal,
        (&client, "8899aabbccddeeff"),
        &Cancellation::default(),
    );
    task.join().unwrap();
    assert!(
        matches!(&refused, Err(Error::Invalid(reason)) if reason.contains("does not list")),
        "{refused:?}"
    );
    assert_eq!(requests.lock().unwrap().len(), 2, "it only looked");
    let kept: Journal = serde_json::from_str(&std::fs::read_to_string(root.path().join(JOURNAL)).unwrap()).unwrap();
    assert_eq!(kept.binding, "0011223344556677", "the journal stays as it was");
}

#[cfg(unix)]
#[test]
fn a_rotated_token_that_lists_the_builder_deletes_it() {
    use crate::cloud_runtime::deployment::hetzner::tests::provider::serve;
    let root = tempfile::tempdir().unwrap();
    let journal = journal_of(root.path(), "0011223344556677");
    let (address, requests, task) = serve(vec![
        (200, listing("servers", &serde_json::json!([server(42)]))),
        (
            404,
            serde_json::json!({"error": {"code": "not_found", "message": "server not found", "details": {}}})
                .to_string(),
        ),
        (200, listing("ssh_keys", &serde_json::json!([]))),
        (200, listing("ssh_keys", &serde_json::json!([]))),
    ]);
    let client = Hetzner::loopback(horizon_cloud::Credential::new("rotated".into()).unwrap(), address).unwrap();
    let returned = return_with(
        root.path(),
        journal,
        (&client, "8899aabbccddeeff"),
        &Cancellation::default(),
    );
    task.join().unwrap();
    assert!(matches!(returned, Ok(true)), "{returned:?}");
    assert!(!left_behind(root.path()), "the builder is proven gone");
    let requests = requests.lock().unwrap();
    assert!(requests[0].starts_with("GET /servers?label_selector="));
    assert!(requests[1].starts_with("GET /servers/42 "));
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
