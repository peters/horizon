use super::*;
use serde_json::json;
use std::collections::BTreeMap;

fn offer(size: &str, region: &str, vcpu: u16, memory_gb: u16, hourly_usd: f64) -> catalog::Offer {
    catalog::Offer {
        size: size.into(),
        region: region.into(),
        vcpu,
        memory_gb,
        hourly_usd,
    }
}

const P4: f64 = 5.28;
const P6: f64 = 10.21;
const P10: f64 = 19.71;
const ADDRESS: f64 = 0.005;

fn disks() -> BTreeMap<u32, f64> {
    BTreeMap::from([(32, P4), (64, P6), (128, P10)])
}

fn catalog() -> Catalog {
    Catalog {
        offers: vec![
            offer("Standard_F2s_v2", "northeurope", 2, 4, 0.096),
            offer("Standard_D4s_v3", "northeurope", 4, 16, 0.214),
            offer("Standard_D4s_v3", "eastus", 4, 16, 0.192),
            offer("Standard_D8s_v3", "northeurope", 8, 32, 0.428),
            // The price list confirmed no address price in this region.
            offer("Standard_D4s_v3", "westeurope", 4, 16, 0.1),
            // Nor any disk price in this one.
            offer("Standard_D4s_v3", "uksouth", 4, 16, 0.1),
        ],
        disk_month_usd: BTreeMap::from([
            ("northeurope".into(), disks()),
            ("eastus".into(), disks()),
            ("westeurope".into(), disks()),
        ]),
        static_ip_hour_usd: BTreeMap::from([
            ("northeurope".into(), ADDRESS),
            ("eastus".into(), ADDRESS),
            ("uksouth".into(), ADDRESS),
        ]),
        regions: BTreeMap::from([
            ("northeurope".into(), "EUROPE".into()),
            ("westeurope".into(), "EUROPE".into()),
            ("uksouth".into(), "EUROPE".into()),
            ("eastus".into(), "NORTH_AMERICA".into()),
        ]),
    }
}

fn requirements(value: serde_json::Value) -> Requirements {
    match serde_json::from_value(value) {
        Ok(requirements) => requirements,
        Err(error) => panic!("requirements: {error}"),
    }
}

#[test]
fn offers_are_dollar_priced_and_include_the_disks_and_static_address() {
    let offers = azure(
        &catalog(),
        &requirements(json!({"min_vcpu": 4, "hours": 2.5, "storage_gb": 100, "region": "northeurope"})),
    );
    let first = &offers[0];
    assert_eq!(
        (first.provider, first.currency, first.id.as_str(), first.kind),
        ("Azure", "USD", "Standard_D4s_v3", "cpu")
    );
    assert_eq!(first.name, "Standard_D4s_v3 · 4 vCPU · 16 GB");
    assert_eq!(first.location.as_deref(), Some("northeurope"));
    assert_eq!(first.monthly, None);
    assert_eq!(first.availability, "checked_at_creation");
    assert!((first.hourly - 0.214).abs() < 1e-12, "compute only");
    // A 30 GiB OS disk bills as P4, a 100 GiB workspace disk as P10.
    let expected = (0.214 + ADDRESS) * 2.5 + (P4 + P10) * 2.5 / 730.0;
    assert!(
        (first.estimated_total - expected).abs() < 1e-9,
        "{}",
        first.estimated_total
    );
}

#[test]
fn a_deallocated_cloud_keeps_its_disks_and_address_in_the_stopped_cost() {
    let offers = azure(&catalog(), &requirements(json!({"storage_gb": 50})));
    for offer in &offers {
        assert!(
            (offer.stopped_monthly - (P4 + P6 + ADDRESS * 730.0)).abs() < 1e-9,
            "{}",
            offer.id
        );
    }
    assert!(!offers.is_empty());
}

#[test]
fn an_offer_whose_disks_or_address_cannot_be_priced_is_left_out() {
    let offers = azure_catalog(&catalog(), &requirements(json!({})));
    let regions: Vec<_> = offers.iter().filter_map(|offer| offer.location.as_deref()).collect();
    assert!(
        !regions.contains(&"westeurope") && !regions.contains(&"uksouth"),
        "{regions:?}"
    );
    assert_eq!(offers.len(), 4);
    // No priced tier holds a 200 GiB workspace disk here.
    assert!(azure_catalog(&catalog(), &requirements(json!({"storage_gb": 200}))).is_empty());
    // Beyond the largest Azure tier, unvalidated requirements get nothing either.
    assert!(azure_catalog(&catalog(), &requirements(json!({"storage_gb": 40_000}))).is_empty());
}

#[test]
fn region_location_and_max_hourly_filters_and_gpu_requests_yield_no_rows() {
    let ids = |value| {
        azure_catalog(&catalog(), &requirements(value))
            .into_iter()
            .map(|offer| format!("{}@{}", offer.id, offer.location.unwrap_or_default()))
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(json!({"region": "north_america"})), ["Standard_D4s_v3@eastus"]);
    assert_eq!(ids(json!({"region": "eastus"})), ["Standard_D4s_v3@eastus"]);
    assert_eq!(
        ids(json!({"region": "EUROPE", "max_hourly": 0.25})),
        ["Standard_F2s_v2@northeurope", "Standard_D4s_v3@northeurope"]
    );
    assert_eq!(ids(json!({"min_memory_gb": 32})), ["Standard_D8s_v3@northeurope"]);
    assert!(ids(json!({"gpu": true})).is_empty());
    assert!(ids(json!({"region": "ASIA"})).is_empty());
}

#[test]
fn the_answer_section_names_azure_usd_and_is_deployable_but_no_offer_is_rentable() {
    let section = azure_section(&catalog(), &requirements(json!({"limit": 2})));
    assert_eq!(section["provider"], "Azure");
    assert_eq!(section["currency"], "USD");
    assert_eq!(section["deployable"], true);
    assert_eq!(section["offers"].as_array().map(Vec::len), Some(2));
    assert_eq!(section["offers"][0]["id"], "Standard_F2s_v2");
    assert_eq!(section["offers"][0]["rentable"], false);
    assert!(section["offers"][0].get("monthly").is_none());
    assert_eq!(section["provider"], crate::provider::AZURE.label);
}

#[test]
fn hours_are_priced_at_least_one_and_at_most_a_year() {
    let total = |hours: Option<f64>| {
        let mut requirements = requirements(json!({"min_vcpu": 8}));
        requirements.hours = hours;
        azure(&catalog(), &requirements)[0].estimated_total
    };
    let hour = (0.428 + ADDRESS) + (P4 + P4) / 730.0;
    assert!((total(None) - hour).abs() < 1e-9);
    assert!((total(Some(0.0)) - hour).abs() < 1e-9);
    assert!((total(Some(0.25)) - hour).abs() < 1e-9);
    let year = hour * MAX_HOURS;
    for unreadable in [f64::NAN, f64::INFINITY, 1e9] {
        assert!((total(Some(unreadable)) - year).abs() < 1e-6, "{unreadable}");
    }
}
