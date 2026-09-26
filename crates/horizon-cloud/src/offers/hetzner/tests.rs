use super::*;
use std::collections::BTreeMap;

fn offer(server_type: &str, location: &str, cores: u32, memory_gb: f64, hourly: f64, monthly: f64) -> catalog::Offer {
    catalog::Offer {
        server_type: server_type.into(),
        location: location.into(),
        cores,
        memory_gb,
        disk_gb: 80,
        dedicated: server_type.starts_with("ccx"),
        hourly_eur: hourly,
        monthly_eur: monthly,
        available: location != "fsn1",
        recommended: false,
    }
}

fn catalog() -> Catalog {
    Catalog {
        offers: vec![
            offer("cx23", "hel1", 2, 4.0, 0.0088, 5.49),
            offer("cx43", "hel1", 8, 16.0, 0.0256, 15.99),
            offer("cx43", "fsn1", 8, 16.0, 0.0256, 15.99),
            offer("cpx42", "ash", 8, 16.0, 0.1931, 120.49),
            offer("ccx33", "hel1", 8, 32.0, 0.2219, 138.49),
            // Hetzner priced no IPv4 address for this location.
            offer("cx43", "nbg1", 8, 16.0, 0.0256, 15.99),
        ],
        volume_gb_month_eur: 0.0572,
        ipv4_month_eur: BTreeMap::from([("hel1".into(), 0.5), ("fsn1".into(), 0.5), ("ash".into(), 0.5)]),
        regions: BTreeMap::from([
            ("hel1".into(), "EUROPE".into()),
            ("fsn1".into(), "EUROPE".into()),
            ("ash".into(), "NORTH_AMERICA".into()),
        ]),
    }
}

fn requirements(value: serde_json::Value) -> Requirements {
    serde_json::from_value(value).unwrap()
}

#[test]
fn offers_are_euro_priced_per_started_hour_with_running_storage_and_a_kept_volume() {
    let offers = hetzner(
        &catalog(),
        &requirements(serde_json::json!({"min_vcpu": 8, "hours": 2.5, "storage_gb": 100})),
    );
    let first = &offers[0];
    assert_eq!(
        (first.provider, first.currency, first.id.as_str()),
        ("Hetzner", "EUR", "cx43")
    );
    assert_eq!(first.name, "cx43 · 8 vCPU · 16 GB · shared");
    // Three started hours of compute, volume and IPv4 address.
    let expected = 3.0 * 0.0256 + (0.0572 * 100.0 + 0.5) * 3.0 / 730.0;
    assert!((first.estimated_total - expected).abs() < 1e-9);
    assert_eq!(first.monthly, Some(15.99));
    assert!(
        (first.stopped_monthly - 5.72).abs() < 1e-9,
        "a stopped cloud keeps only its volume"
    );
    assert!(!first.rentable, "not deployable until the wiring lands");
    let encoded = serde_json::to_value(first).unwrap();
    assert_eq!(encoded["currency"], "EUR");
    assert_eq!(encoded["location"], first.location.as_deref().unwrap());
    assert!(offers.iter().all(|offer| offer.vcpu >= Some(8)));
}

#[test]
fn advisory_availability_is_reported_but_never_hides_an_offer() {
    let offers = hetzner(
        &catalog(),
        &requirements(serde_json::json!({"min_vcpu": 8, "limit": 50})),
    );
    let fsn1 = offers
        .iter()
        .find(|offer| offer.location.as_deref() == Some("fsn1"))
        .unwrap();
    assert_eq!(fsn1.availability, "unlisted");
    let hel1 = offers
        .iter()
        .find(|offer| offer.id == "cx43" && offer.location.as_deref() == Some("hel1"))
        .unwrap();
    assert_eq!(hel1.availability, "listed");
}

#[test]
fn the_monthly_cap_limits_long_runs() {
    assert!((compute(0.0256, 15.99, 730.0) - 15.99).abs() < 1e-9);
    assert!((compute(0.0256, 15.99, 800.0) - (15.99 + 70.0 * 0.0256)).abs() < 1e-9);
    assert!((compute(0.0256, 15.99, 0.2) - 0.0256).abs() < 1e-9);
    assert!(compute(0.0256, 15.99, 0.0).abs() < 1e-9);
}

#[test]
fn region_location_price_and_gpu_requirements_filter_offers() {
    let catalog = catalog();
    let europe = hetzner(
        &catalog,
        &requirements(serde_json::json!({"region": "Europe", "limit": 50})),
    );
    assert!(!europe.is_empty());
    assert!(europe.iter().all(|offer| offer.location.as_deref() != Some("ash")));
    let ash = hetzner(&catalog, &requirements(serde_json::json!({"region": "ash"})));
    assert_eq!(ash.len(), 1);
    assert_eq!(ash[0].id, "cpx42");
    let cheap = hetzner(
        &catalog,
        &requirements(serde_json::json!({"max_hourly": 0.03, "limit": 50})),
    );
    assert!(cheap.iter().all(|offer| offer.hourly <= 0.03));
    assert!(!cheap.iter().any(|offer| offer.id == "ccx33"));
    let memory = hetzner(&catalog, &requirements(serde_json::json!({"min_memory_gb": 32})));
    assert_eq!(memory.len(), 1);
    assert_eq!(memory[0].name, "ccx33 · 8 vCPU · 32 GB · dedicated");
    assert!(hetzner(&catalog, &requirements(serde_json::json!({"gpu": true}))).is_empty());
    assert_eq!(
        hetzner(&catalog, &requirements(serde_json::json!({"limit": 2}))).len(),
        2
    );
}

#[test]
fn an_offer_whose_address_cannot_be_priced_is_left_out() {
    let offers = hetzner(&catalog(), &requirements(serde_json::json!({"limit": 50})));
    assert!(offers.iter().all(|offer| offer.location.as_deref() != Some("nbg1")));
}

#[test]
fn memory_is_counted_in_whole_gigabytes() {
    assert_eq!(whole_gb(16.0), Some(16));
    assert_eq!(whole_gb(0.5), Some(0));
    assert_eq!(whole_gb(-1.0), None);
    assert_eq!(whole_gb(f64::NAN), None);
    assert_eq!(whole_gb(1e9), None);
}
