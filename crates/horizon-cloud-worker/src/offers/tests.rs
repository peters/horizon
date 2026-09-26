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
    let agent = request("horizon:cloud-a", HOST, &serde_json::json!({"gpu": true, "hours": 2}));
    // Before any prices arrive, the agent is told where they come from.
    let missing = answer_from(&agent, &path, HOST, NOW);
    assert!(error(&missing).starts_with("cloud_offers_unavailable: no prices"));

    publish(snapshot(u64::try_from(NOW).unwrap() - 60_000).as_slice(), &path, NOW).unwrap();
    let answered = answer_from(&agent, &path, HOST, NOW);
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
    let stale = answer_from(&agent, &path, HOST, NOW);
    assert!(error(&stale).starts_with("cloud_offers_stale: the newest prices on this worker are 21 minutes old"));
    assert!(stale.offers.is_none());
    // Prices dated beyond the tolerated clock difference could never go stale.
    let ahead = u64::try_from(NOW).unwrap() + 6 * 60_000;
    assert!(publish(snapshot(ahead).as_slice(), &path, NOW).is_err());
    publish(snapshot(ahead).as_slice(), &path, NOW + 60_000).unwrap();
    let future = answer_from(&agent, &path, HOST, NOW);
    assert!(error(&future).ends_with("dated in the future"));
    // A few minutes of difference is tolerated.
    publish(
        snapshot(u64::try_from(NOW).unwrap() + 4 * 60_000).as_slice(),
        &path,
        NOW,
    )
    .unwrap();
    let skewed = answer_from(&agent, &path, HOST, NOW);
    assert_eq!(skewed.offers.unwrap()["observed_seconds_ago"], 0);
}

#[test]
fn only_this_workers_agents_with_valid_requirements_get_offers() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cloud-offers.json");
    publish(snapshot(u64::try_from(NOW).unwrap()).as_slice(), &path, NOW).unwrap();
    let valid = serde_json::json!({"min_vcpu": 4});
    for refused in [
        request("horizon:agent", HOST, &valid),
        request("horizon:cloud-a", "another-host", &valid),
    ] {
        assert_eq!(
            error(&answer_from(&refused, &path, HOST, NOW)),
            "cloud_offers_unavailable"
        );
    }
    let late = answer_from(&request("horizon:cloud-a", HOST, &valid), &path, HOST, NOW + 15_000);
    assert_eq!(error(&late), "cloud_offers_timed_out");
    for invalid in [serde_json::json!({"rent": true}), serde_json::json!({"gpu_type": "L4"})] {
        let refused = answer_from(&request("horizon:cloud-a", HOST, &invalid), &path, HOST, NOW);
        assert!(error(&refused).starts_with("cloud_offers_invalid_request"), "{invalid}");
    }
    let cpu = answer_from(&request("horizon:cloud-a", HOST, &valid), &path, HOST, NOW);
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
