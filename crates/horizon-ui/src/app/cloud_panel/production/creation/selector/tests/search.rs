use super::{form, render};

#[test]
fn search_finds_flavor_ids_data_centers_regions_and_cpu_kind() {
    let mut form = form("cpu");
    form.launch.selector.in_stock_only = false;
    form.prices.hetzner.answered(Some(hetzner_catalog()));
    form.prices
        .exchange
        .answered(super::super::super::super::prices::Fetched {
            at: std::time::Instant::now(),
            value: horizon_core::cloud_runtime::offers::exchange::Rates {
                date: "2026-10-09".into(),
                usd_per_unit: std::collections::BTreeMap::from([("USD".into(), 1.0), ("EUR".into(), 1.2)]),
            },
        });
    let shown = |form: &mut crate::app::cloud_panel::production::Production, query: &str| {
        form.launch.selector.search = query.into();
        let labels = render(form);
        let line = labels.iter().find(|label| label.starts_with("Showing ")).unwrap();
        let count: usize = line.split_whitespace().nth(1).unwrap().parse().unwrap();
        let empty = labels.iter().any(|label| label.starts_with("No worker matches"));
        (count, empty)
    };
    let (all, _) = shown(&mut form, "");
    assert!(all > 2, "RunPod and Hetzner rows are both listed");
    let (flavors, empty) = shown(&mut form, "cpu3c");
    assert!(
        flavors > 0 && flavors < all && !empty,
        "flavor id matches RunPod rows only"
    );
    assert_eq!(shown(&mut form, "cpu5m"), (0, true));
    let (centers, empty) = shown(&mut form, "EU-1");
    assert!(centers > 0 && centers < all && !empty);
    let (regions, empty) = shown(&mut form, "north_america");
    assert!(regions > 0 && regions < all && !empty);
    assert_eq!(shown(&mut form, "dedicated").0, 1);
    assert_eq!(shown(&mut form, "shared").0, 1);
    form.launch.selector.search.clear();
    let labels = render(&mut form);
    assert!(!labels.iter().any(|label| label.starts_with("Cloud settings exclude")));
    form.prices.hetzner.answered_with_exclusions(
        Some(hetzner_catalog()),
        horizon_core::cloud_runtime::prices::HetznerExclusions {
            server_types: 4,
            locations: 2,
        },
    );
    let labels = render(&mut form);
    assert!(
        labels
            .iter()
            .any(|label| label == "Cloud settings exclude 4 Hetzner server types and 2 locations.")
    );
    form.launch.selector.provider_filter = Some("RunPod".into());
    let labels = render(&mut form);
    assert!(!labels.iter().any(|label| label.starts_with("Cloud settings exclude")));
}

fn hetzner_catalog() -> horizon_core::cloud_runtime::prices::HetznerCatalog {
    serde_json::from_value(serde_json::json!({
        "offers": [
            {"server_type": "cx33", "location": "hel1", "cores": 4, "memory_gb": 8.0, "disk_gb": 80,
             "dedicated": false, "hourly_eur": 0.0136, "monthly_eur": 8.49, "available": true, "recommended": false},
            {"server_type": "ccx33", "location": "fsn1", "cores": 8, "memory_gb": 32.0, "disk_gb": 240,
             "dedicated": true, "hourly_eur": 0.099, "monthly_eur": 64.0, "available": true, "recommended": false}
        ],
        "volume_gb_month_eur": 0.0572,
        "ipv4_month_eur": {"hel1": 0.5, "fsn1": 0.5},
        "ipv4_hour_eur": {"hel1": 0.0008, "fsn1": 0.0008},
        "regions": {"hel1": "EUROPE", "fsn1": "EUROPE"}
    }))
    .unwrap()
}
