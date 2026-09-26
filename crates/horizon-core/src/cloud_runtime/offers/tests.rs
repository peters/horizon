use super::*;
use horizon_cloud::prices::{CpuFlavorPrice, GpuPrice};

fn list() -> PriceList {
    let center = |id: &str, region: &str, gpus: &[(&str, Availability)]| DataCenter {
        id: id.into(),
        region: region.into(),
        workspace_storage: true,
        gpus: gpus.iter().map(|&(gpu, level)| (gpu.into(), level)).collect(),
    };
    let gpu = |id: &str, name: &str, memory_gb, hourly| GpuPrice {
        id: id.into(),
        name: name.into(),
        memory_gb,
        hourly,
    };
    PriceList {
        provider: "RunPod",
        cpu: vec![
            CpuFlavorPrice {
                id: "cpu3c".into(),
                name: "Compute-Optimized".into(),
                per_vcpu_hour: 0.03,
            },
            CpuFlavorPrice {
                id: "cpu3g".into(),
                name: "General Purpose".into(),
                per_vcpu_hour: 0.04,
            },
        ],
        gpus: vec![
            gpu("NVIDIA RTX A5000", "RTX A5000", 24, 0.27),
            gpu("NVIDIA L4", "L4", 24, 0.49),
            gpu("NVIDIA A40", "A40", 48, 0.44),
        ],
        data_centers: vec![
            center("EU-RO-1", "EUROPE", &[("NVIDIA RTX A5000", Availability::High)]),
            center(
                "US-MO-2",
                "NORTH_AMERICA",
                &[("NVIDIA L4", Availability::Low), ("NVIDIA A40", Availability::None)],
            ),
        ],
        regions: std::collections::BTreeMap::new(),
        storage: horizon_cloud::runpod::prices::STORAGE,
    }
}

#[test]
fn cpu_offers_meet_the_size_and_rank_by_estimated_total() {
    let requirements = Requirements {
        min_vcpu: Some(4),
        min_memory_gb: Some(16),
        hours: Some(10.0),
        ..Requirements::default()
    };
    let offers = offers(&list(), &Preferences::default(), &requirements);
    let first = &offers[0];
    // 4 vCPU general purpose has 16 GB at $0.16/h, cheaper than 8 vCPU compute-optimized
    // (also 16 GB) at $0.24/h.
    assert_eq!(
        (first.id.as_str(), first.flavors.as_slice(), first.memory_gb),
        ("cpu-4-16", &["cpu3g".to_owned()][..], Some(16))
    );
    // Ten hours of compute plus ten hours of a 20 GB network volume.
    let storage = 20.0 * 0.07 * 10.0 / MONTH_HOURS;
    assert!((first.estimated_total - (0.16 * 10.0 + storage)).abs() < 1e-9);
    assert_eq!(first.availability, "checked_at_creation");
    assert!(
        offers
            .iter()
            .all(|offer| offer.vcpu >= Some(4) && offer.memory_gb >= Some(16))
    );
    assert!(
        offers
            .windows(2)
            .all(|pair| pair[0].estimated_total <= pair[1].estimated_total)
    );
    assert!(
        offers
            .iter()
            .all(|offer| offer.host == "provider_operated" && offer.rentable)
    );
}

#[test]
fn cpu_offers_request_the_preferred_flavors_and_quote_the_dearest() {
    let preferences = Preferences {
        cpu_flavors: vec!["cpu3c".into(), "cpu3g".into()],
        gpu_types: Vec::new(),
    };
    let requirements = Requirements {
        min_vcpu: Some(4),
        ..Requirements::default()
    };
    let offers = offers(&list(), &preferences, &requirements);
    // Both preferred flavors offer 4 vCPU and 8 GB, so a cloud of that size requests
    // both and the provider may allocate the dearer one.
    let small = offers.iter().find(|offer| offer.id == "cpu-4-8").unwrap();
    assert_eq!(small.flavors, ["cpu3c", "cpu3g"]);
    assert!((small.hourly - 0.04 * 4.0).abs() < 1e-9);
    assert_eq!(small.name, "Compute-Optimized or General Purpose · 4 vCPU · 8 GB");
    // Only General Purpose offers 16 GB at 4 vCPU.
    let larger = offers.iter().find(|offer| offer.id == "cpu-4-16").unwrap();
    assert_eq!(larger.flavors, ["cpu3g"]);
    // A size whose flavor has no price is left out rather than guessed.
    assert!(offers.iter().all(|offer| offer.id != "cpu-4-32"));
}

#[test]
fn cpu_offers_need_a_region_that_can_hold_the_workspace() {
    let cpu = |region: &str, list: &PriceList| {
        offers(
            list,
            &Preferences::default(),
            &Requirements {
                region: Some(region.into()),
                ..Requirements::default()
            },
        )
        .len()
    };
    let mut prices = list();
    assert_eq!(cpu("Europe", &prices), DEFAULT_LIMIT);
    assert_eq!(cpu("ASIA", &prices), 0);
    prices.data_centers[0].workspace_storage = false;
    assert_eq!(cpu("EUROPE", &prices), 0);
    assert_eq!(cpu("NORTH_AMERICA", &prices), DEFAULT_LIMIT);
}

#[test]
fn gpu_offers_follow_stock_type_memory_region_and_price() {
    let gpu = |requirements: Requirements| -> Vec<String> {
        offers(
            &list(),
            &Preferences::default(),
            &Requirements {
                gpu: true,
                ..requirements
            },
        )
        .into_iter()
        .map(|offer| offer.id)
        .collect()
    };
    // Sold-out types are left out unless asked for.
    assert_eq!(gpu(Requirements::default()), ["NVIDIA RTX A5000", "NVIDIA L4"]);
    assert_eq!(
        gpu(Requirements {
            include_unavailable: true,
            ..Requirements::default()
        }),
        ["NVIDIA RTX A5000", "NVIDIA A40", "NVIDIA L4"]
    );
    assert_eq!(
        gpu(Requirements {
            region: Some("North America".into()),
            ..Requirements::default()
        }),
        ["NVIDIA L4"]
    );
    // An unknown region has nothing to offer, even counting sold-out types.
    assert!(
        gpu(Requirements {
            region: Some("ASIA".into()),
            include_unavailable: true,
            ..Requirements::default()
        })
        .is_empty()
    );
    let with_sold_out = offers(
        &list(),
        &Preferences::default(),
        &Requirements {
            gpu: true,
            include_unavailable: true,
            ..Requirements::default()
        },
    );
    assert!(
        with_sold_out
            .iter()
            .all(|offer| offer.rentable == (offer.availability != "none"))
    );
    assert!(with_sold_out.iter().any(|offer| !offer.rentable));
    assert_eq!(
        gpu(Requirements {
            gpu_type: Some("l4".into()),
            ..Requirements::default()
        }),
        ["NVIDIA L4"]
    );
    assert_eq!(
        gpu(Requirements {
            max_hourly: Some(0.3),
            ..Requirements::default()
        }),
        ["NVIDIA RTX A5000"]
    );
    assert_eq!(
        gpu(Requirements {
            min_gpu_memory_gb: Some(40),
            include_unavailable: true,
            ..Requirements::default()
        }),
        ["NVIDIA A40"]
    );
    let a5000 = &offers(
        &list(),
        &Preferences::default(),
        &Requirements {
            gpu: true,
            ..Requirements::default()
        },
    )[0];
    assert_eq!(
        (a5000.availability, a5000.regions_in_stock.as_slice()),
        ("high", &["EUROPE".to_owned()][..])
    );
}

#[test]
fn limits_apply_and_bad_amounts_are_rejected() {
    let limited = offers(
        &list(),
        &Preferences::default(),
        &Requirements {
            limit: Some(2),
            ..Requirements::default()
        },
    );
    assert_eq!(limited.len(), 2);
    for limit in [0, MAX_LIMIT + 1] {
        let requirements = Requirements {
            limit: Some(limit),
            ..Requirements::default()
        };
        assert!(requirements.validate().is_err(), "limit {limit}");
    }
    assert!(
        Requirements {
            max_hourly: Some(-1.0),
            ..Requirements::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Requirements {
            hours: Some(f64::NAN),
            ..Requirements::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Requirements {
            region: Some(" ".into()),
            ..Requirements::default()
        }
        .validate()
        .is_err()
    );
    // Storage Horizon could not create is not priced as rentable.
    let storage = |gpu, storage_gb| {
        Requirements {
            gpu,
            storage_gb: Some(storage_gb),
            ..Requirements::default()
        }
        .validate()
        .is_ok()
    };
    assert!(!storage(false, 0) && !storage(false, 9) && !storage(false, 4001));
    assert!(storage(false, 10) && storage(false, 4000));
    assert!(!storage(true, 0) && storage(true, 5));
    // Requirements for the other kind of worker are refused rather than ignored.
    let rejected = [
        Requirements {
            gpu_type: Some("RTX A5000".into()),
            ..Requirements::default()
        },
        Requirements {
            min_gpu_memory_gb: Some(24),
            ..Requirements::default()
        },
        Requirements {
            include_unavailable: true,
            ..Requirements::default()
        },
        Requirements {
            gpu: true,
            min_vcpu: Some(32),
            ..Requirements::default()
        },
        Requirements {
            gpu: true,
            min_memory_gb: Some(64),
            ..Requirements::default()
        },
        Requirements {
            hours: Some(f64::MAX),
            ..Requirements::default()
        },
    ];
    for requirements in rejected {
        assert!(requirements.validate().is_err(), "{requirements:?}");
    }
    assert!(
        Requirements {
            hours: Some(MAX_HOURS),
            ..Requirements::default()
        }
        .validate()
        .is_ok()
    );
    assert!(Requirements::default().validate().is_ok());
}
