//! The worker an agent's cloud rents: the offer its policy admits, and no dearer fallback.

use super::*;
use horizon_core::cloud_panel::WorkerChoice;
use horizon_core::cloud_runtime::{offers::workers, settings::AgentLifecycle};

/// The Hetzner catalog an agent's create ranks: a cheap 2 vCPU worker and a dearer 8 vCPU
/// one, both in `hel1`.
fn hetzner_catalog() -> prices::HetznerCatalog {
    let offer = |server_type: &str, cores, memory_gb, hourly_eur| {
        json!({"server_type": server_type, "location": "hel1", "cores": cores,
            "memory_gb": memory_gb, "disk_gb": 160, "dedicated": false, "hourly_eur": hourly_eur,
            "monthly_eur": hourly_eur * 600.0, "available": true, "recommended": false})
    };
    serde_json::from_value(json!({
        "offers": [offer("cx23", 2, 4.0, 0.0088), offer("ccx33", 8, 32.0, 0.06)],
        "volume_gb_month_eur": 0.0572, "ipv4_month_eur": {"hel1": 0.5},
        "ipv4_hour_eur": {"hel1": 0.0008}, "regions": {"hel1": "EUROPE"},
    }))
    .unwrap()
}

/// A cloud settings file whose Hetzner fallback server type is the dearer one, so a
/// deployment that is not pinned to the admitted offer rents that type instead.
fn hetzner_fallback(root: &Path) -> PathBuf {
    let path = root.join("settings.json");
    let absolute = |name: &str| root.join(name);
    std::fs::write(
        &path,
        json!({
            "runpod_key_file": absolute("key"), "ssh_identity_file": absolute("identity"),
            "docker_config": absolute("docker"), "registry_pull_auth_id": null,
            "cpu_flavors": ["cpu3c"], "gpu_types": [], "data_centers": ["US-MO-2"],
            "hetzner": {"token_file": absolute("hetzner-token"), "server_types": ["ccx33"],
                "locations": ["hel1"]},
        })
        .to_string(),
    )
    .unwrap();
    path
}

#[test]
fn the_deployment_requests_the_offer_its_policy_admits_and_not_a_dearer_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let path = hetzner_fallback(temp.path());
    let machine = Settings::for_cloud(&path, &Placement::default()).unwrap();
    assert_eq!(
        machine.hetzner.as_ref().unwrap().types_for(None).unwrap(),
        ["ccx33"],
        "the machine's fallback type, which a deployment without the pin would rent"
    );
    // The offer an agent's create admits: the cheapest CPU worker for the quick-start profile.
    let catalog = hetzner_catalog();
    let ranked = workers(&profile(), 1.0, None, Some(&catalog));
    let offer = ranked
        .offers
        .iter()
        .take(ranked.matching)
        .find(|offer| offer.id == "cx23")
        .expect("the cheap worker meets the quick-start requirements");
    let choice = WorkerChoice::from(offer).for_profile(&profile()).unwrap();
    let chosen = Chosen {
        provider: choice.provider,
        hourly: offer.hourly,
        size: choice.size,
        placement: choice.placement,
    };
    // The limit admits the cheap offer alone: the fallback type bills above it.
    let policy = AgentLifecycle {
        providers: vec!["hetzner".into()],
        max_hourly: 0.01,
        repositories: vec!["/home/person/shop".into()],
        allow_github_sign_in_skip: false,
    };
    let shop = Path::new("/home/person/shop");
    assert_eq!(policy.admits(shop, chosen.provider.id, Some(chosen.hourly)), Ok(()));
    assert_eq!(
        policy.admits(shop, "hetzner", Some(0.06)),
        Err(AgentLifecycleRefusal::Above)
    );
    // The request the deployment sends names that offer's worker, not the fallback one.
    let (profile, settings) = pinned_offer(&path, &profile(), &chosen).unwrap();
    let hetzner = settings.hetzner.as_ref().unwrap();
    let placement = settings.placement.as_ref();
    assert_eq!(
        hetzner.types_for(placement).unwrap(),
        ["cx23"],
        "the server type of the offer whose rate was admitted"
    );
    assert_eq!(hetzner.locations_for(placement).unwrap(), ["hel1"]);
    assert_eq!(
        (profile.provider.as_str(), profile.cpu, profile.memory_gb),
        ("hetzner", 2, 4),
        "the size of the offer whose rate was admitted"
    );
}
