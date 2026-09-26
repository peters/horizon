use super::*;
use horizon_cloud::prices::{Availability, CpuFlavorPrice, DataCenter, GpuPrice, Preferences, PriceList};

const HOST: &str = "worker-host";
const NOW: i64 = 10_000_000;

fn snapshot(observed_at_millis: u64) -> Vec<u8> {
    serde_json::to_vec(&Snapshot {
        version: horizon_cloud_protocol::offers::VERSION,
        observed_at_millis,
        list: PriceList {
            provider: "RunPod",
            cpu: vec![CpuFlavorPrice {
                id: "cpu3c".into(),
                name: "Compute-Optimized".into(),
                per_vcpu_hour: 0.03,
            }],
            gpus: vec![GpuPrice {
                id: "NVIDIA RTX A5000".into(),
                name: "RTX A5000".into(),
                memory_gb: 24,
                hourly: 0.27,
            }],
            data_centers: vec![DataCenter {
                id: "EU-RO-1".into(),
                region: "EUROPE".into(),
                workspace_storage: true,
                gpus: vec![("NVIDIA RTX A5000".into(), Availability::High)],
            }],
            regions: std::collections::BTreeMap::new(),
            storage: horizon_cloud::runpod::prices::STORAGE,
        },
        preferences: Preferences::default(),
    })
    .unwrap()
}

fn request(actor: &str, host: &str, requirements: &serde_json::Value) -> UsageRequest {
    serde_json::from_value(serde_json::json!({
        "request_id": "offers", "actor": actor, "host_instance": host,
        "deadline_at_millis": NOW + 15_000, "cloud_offers": requirements, "claimed": true
    }))
    .unwrap()
}

fn error(result: &UsageResult) -> &str {
    result.error.as_deref().unwrap_or_default()
}

#[test]
fn agents_rank_offers_from_the_prices_the_owning_horizon_sent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cloud-offers.json");
    let sessions = temp.path().join("sessions");
    std::fs::create_dir_all(sessions.join("a")).unwrap();
    let agent = request("horizon:cloud-a", HOST, &serde_json::json!({"gpu": true, "hours": 2}));
    // Before any prices arrive, the agent is told where they come from.
    let missing = answer_from(&agent, (&path, &sessions), HOST, NOW);
    assert!(error(&missing).starts_with("cloud_offers_unavailable: no prices"));

    publish(snapshot(u64::try_from(NOW).unwrap() - 60_000).as_slice(), &path, NOW).unwrap();
    let answered = answer_from(&agent, (&path, &sessions), HOST, NOW);
    let offers = answered.offers.unwrap();
    assert_eq!(offers["provider"], "RunPod");
    assert_eq!(offers["observed_seconds_ago"], 60);
    assert_eq!(offers["offers"][0]["id"], "NVIDIA RTX A5000");
    assert_eq!(offers["offers"][0]["regions_in_stock"][0], "EUROPE");

    // Prices the owning Horizon stopped refreshing are not offered as current.
    publish(
        snapshot(u64::try_from(NOW).unwrap() - 21 * 60_000).as_slice(),
        &path,
        NOW,
    )
    .unwrap();
    let stale = answer_from(&agent, (&path, &sessions), HOST, NOW);
    assert!(error(&stale).starts_with("cloud_offers_stale: the newest prices on this worker are 21 minutes old"));
    assert!(stale.offers.is_none());
    // Prices dated beyond the tolerated clock difference could never go stale.
    let ahead = u64::try_from(NOW).unwrap() + 6 * 60_000;
    assert!(publish(snapshot(ahead).as_slice(), &path, NOW).is_err());
    publish(snapshot(ahead).as_slice(), &path, NOW + 60_000).unwrap();
    let future = answer_from(&agent, (&path, &sessions), HOST, NOW);
    assert!(error(&future).ends_with("dated in the future"));
    // A few minutes of difference is tolerated.
    publish(
        snapshot(u64::try_from(NOW).unwrap() + 4 * 60_000).as_slice(),
        &path,
        NOW,
    )
    .unwrap();
    let skewed = answer_from(&agent, (&path, &sessions), HOST, NOW);
    assert_eq!(skewed.offers.unwrap()["observed_seconds_ago"], 0);
}

#[test]
fn only_this_workers_agents_with_valid_requirements_get_offers() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cloud-offers.json");
    let sessions = temp.path().join("sessions");
    std::fs::create_dir_all(sessions.join("a")).unwrap();
    publish(snapshot(u64::try_from(NOW).unwrap()).as_slice(), &path, NOW).unwrap();
    let valid = serde_json::json!({"min_vcpu": 4});
    // Another host, an actor that is not a worker agent, and a session that has ended
    // or never existed are refused.
    for refused in [
        request("horizon:agent", HOST, &valid),
        request("horizon:cloud-a", "another-host", &valid),
        request("horizon:cloud-ended", HOST, &valid),
        request("horizon:cloud-../a", HOST, &valid),
    ] {
        assert_eq!(
            error(&answer_from(&refused, (&path, &sessions), HOST, NOW)),
            "cloud_offers_unavailable"
        );
    }
    let late = answer_from(
        &request("horizon:cloud-a", HOST, &valid),
        (&path, &sessions),
        HOST,
        NOW + 15_000,
    );
    assert_eq!(error(&late), "cloud_offers_timed_out");
    for invalid in [serde_json::json!({"rent": true}), serde_json::json!({"gpu_type": "L4"})] {
        let refused = answer_from(
            &request("horizon:cloud-a", HOST, &invalid),
            (&path, &sessions),
            HOST,
            NOW,
        );
        assert!(error(&refused).starts_with("cloud_offers_invalid_request"), "{invalid}");
    }
    let cpu = answer_from(&request("horizon:cloud-a", HOST, &valid), (&path, &sessions), HOST, NOW);
    assert_eq!(cpu.offers.unwrap()["offers"][0]["vcpu"], 4);
}

#[test]
fn only_a_valid_snapshot_replaces_the_prices() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cloud-offers.json");
    publish(snapshot(1).as_slice(), &path, NOW).unwrap();
    let mut other_version: serde_json::Value = serde_json::from_slice(&snapshot(2)).unwrap();
    other_version["version"] = 2.into();
    let rejected = [
        b"not json".to_vec(),
        serde_json::to_vec(&other_version).unwrap(),
        vec![b' '; MAX_BYTES + 1],
    ];
    for bytes in rejected {
        assert!(publish(bytes.as_slice(), &path, NOW).is_err());
    }
    let kept = decode(std::fs::File::open(&path).unwrap()).unwrap();
    assert_eq!(kept.observed_at_millis, 1);
}

fn hetzner_snapshot(observed_at_millis: u64) -> Vec<u8> {
    use horizon_cloud::hetzner::catalog::{Catalog, Offer};
    serde_json::to_vec(&HetznerSnapshot {
        version: horizon_cloud_protocol::offers::VERSION,
        observed_at_millis,
        catalog: Catalog {
            offers: vec![Offer {
                server_type: "cx43".into(),
                location: "hel1".into(),
                cores: 8,
                memory_gb: 16.0,
                disk_gb: 160,
                dedicated: false,
                hourly_eur: 0.0256,
                monthly_eur: 15.99,
                available: false,
                recommended: false,
            }],
            volume_gb_month_eur: 0.0572,
            ipv4_month_eur: std::collections::BTreeMap::from([("hel1".into(), 0.5)]),
            ipv4_hour_eur: std::collections::BTreeMap::from([("hel1".into(), 0.0008)]),
            regions: std::collections::BTreeMap::from([("hel1".into(), "EUROPE".into())]),
        },
    })
    .unwrap()
}

#[test]
fn hetzner_offers_come_beside_the_price_list_in_their_own_currency() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cloud-offers.json");
    let hetzner = hetzner_path(&path);
    let sessions = temp.path().join("sessions");
    std::fs::create_dir_all(sessions.join("a")).unwrap();
    let agent = request("horizon:cloud-a", HOST, &serde_json::json!({"min_vcpu": 8}));
    let minute_ago = u64::try_from(NOW).unwrap() - 60_000;
    publish(snapshot(minute_ago).as_slice(), &path, NOW).unwrap();
    // Without a Hetzner catalog the answer is as before, with no other providers.
    let plain = answer_from(&agent, (&path, &sessions), HOST, NOW).offers.unwrap();
    assert_eq!(plain["other_providers"], serde_json::json!([]));
    assert_eq!(plain["offers"][0]["currency"], "USD");

    publish_hetzner(hetzner_snapshot(minute_ago).as_slice(), &hetzner, NOW).unwrap();
    let answered = answer_from(&agent, (&path, &sessions), HOST, NOW).offers.unwrap();
    let section = &answered["other_providers"][0];
    assert_eq!(
        (section["provider"].as_str(), section["currency"].as_str()),
        (Some("Hetzner"), Some("EUR"))
    );
    assert_eq!(section["observed_seconds_ago"], 60);
    assert_eq!(section["offers"][0]["id"], "cx43");
    assert_eq!(
        section["offers"][0]["availability"], "unlisted",
        "advisory availability never hides the offer"
    );

    // A stale or future-dated catalog is reported, never offered as current.
    publish_hetzner(
        hetzner_snapshot(u64::try_from(NOW).unwrap() - 21 * 60_000).as_slice(),
        &hetzner,
        NOW,
    )
    .unwrap();
    let stale = answer_from(&agent, (&path, &sessions), HOST, NOW).offers.unwrap();
    assert!(
        stale["other_providers"][0]["error"]
            .as_str()
            .unwrap()
            .starts_with("cloud_offers_stale")
    );
    assert!(stale["other_providers"][0].get("offers").is_none());
    let ahead = u64::try_from(NOW).unwrap() + 6 * 60_000;
    assert!(publish_hetzner(hetzner_snapshot(ahead).as_slice(), &hetzner, NOW).is_err());
    // An unreadable catalog does not take the price list down with it.
    std::fs::write(&hetzner, b"{").unwrap();
    let broken = answer_from(&agent, (&path, &sessions), HOST, NOW).offers.unwrap();
    assert_eq!(broken["offers"][0]["currency"], "USD");
    assert!(
        broken["other_providers"][0]["error"]
            .as_str()
            .unwrap()
            .contains("cannot be read")
    );
    // Invalid input is refused and leaves the stored catalog alone.
    assert!(publish_hetzner(&b"{\"version\":1}"[..], &hetzner, NOW).is_err());
    assert_eq!(std::fs::read(&hetzner).unwrap(), b"{");
}
