use super::*;
use crate::cloud_runtime::prices::{DataCenter, GpuPrice, RUNPOD_STORAGE};

fn selection(gpu: bool) -> Selection {
    let config = crate::cloud_panel::CloudConfig::parse(
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n",
    ).unwrap();
    let mut profile = config.profiles["dev"].clone();
    profile.gpu = gpu;
    Selection::new(
        profile,
        Placement {
            data_centers: vec!["EU-1".into()],
            gpu_types: if gpu { vec!["gpu-a".into()] } else { Vec::new() },
            ..Placement::default()
        },
    )
    .unwrap()
}

fn catalog() -> PriceList {
    PriceList {
        provider: "RunPod",
        cpu: Vec::new(),
        gpus: vec![GpuPrice {
            id: "gpu-a".into(),
            name: "GPU A".into(),
            memory_gb: 24,
            hourly: 0.5,
        }],
        data_centers: vec![DataCenter {
            id: "EU-1".into(),
            region: "EUROPE".into(),
            workspace_storage: true,
            high_performance_storage: false,
            cpus: Vec::new(),
            gpus: vec![("gpu-a".into(), Availability::Low)],
        }],
        regions: std::collections::BTreeMap::default(),
        storage: RUNPOD_STORAGE,
    }
}

#[test]
fn a_watch_requires_an_explicit_location_and_gpu_and_a_supported_provider() {
    let chosen = selection(true);
    let mut missing = chosen.placement.clone();
    missing.data_centers.clear();
    assert!(Selection::new(chosen.profile.clone(), missing).is_err());
    let mut missing = chosen.placement.clone();
    missing.gpu_types.clear();
    assert!(Selection::new(chosen.profile.clone(), missing).is_err());
    let mut foreign = chosen.profile;
    foreign.provider = "hetzner".into();
    assert!(Selection::new(foreign, chosen.placement).is_err());
}

#[test]
fn gpu_stock_must_match_the_type_and_allowed_location() {
    let chosen = selection(true);
    let mut list = catalog();
    assert!(chosen.available(&list, None));
    list.data_centers[0].gpus[0].1 = Availability::None;
    assert!(!chosen.available(&list, None));
    list.data_centers[0].gpus[0].1 = Availability::High;
    list.data_centers[0].id = "US-1".into();
    assert!(!chosen.available(&list, None));
    list.data_centers[0].id = "EU-1".into();
    list.gpus.clear();
    assert!(
        !chosen.available(&list, None),
        "removed GPU prices cannot authorize a launch"
    );
}

#[test]
fn cpu_watch_waits_for_exact_size_stock_and_compatible_storage() {
    let chosen = selection(false);
    let mut list = catalog();
    assert!(!chosen.available(&list, None));
    let mut size = SizeAvailability {
        centers: vec![("US-1".into(), Availability::High)],
    };
    assert!(!chosen.available(&list, Some(&size)));
    size.centers[0].0 = "EU-1".into();
    assert!(chosen.available(&list, Some(&size)));
    list.data_centers[0].workspace_storage = false;
    assert!(!chosen.available(&list, Some(&size)));
    list.data_centers.clear();
    assert!(!chosen.available(&list, Some(&size)));
}

#[test]
fn a_high_performance_watch_needs_a_data_center_that_holds_that_volume() {
    let mut chosen = selection(false);
    chosen.profile.storage.volume_tier = crate::cloud_runtime::prices::StorageTier::HighPerformance;
    let mut list = catalog();
    let size = SizeAvailability {
        centers: vec![("EU-1".into(), Availability::High)],
    };
    assert!(!chosen.available(&list, Some(&size)));
    list.data_centers[0].high_performance_storage = true;
    assert!(chosen.available(&list, Some(&size)));
}

#[test]
fn the_watched_price_is_the_gpu_price_or_the_dearest_requested_flavor() {
    let list = catalog();
    let preferences = Preferences::default();
    assert_eq!(selection(true).hourly(&list, &preferences), Some(0.5));
    let mut list = list;
    list.cpu = vec![
        crate::cloud_runtime::prices::CpuFlavorPrice {
            id: "cpu3c".into(),
            name: "Compute-Optimized".into(),
            per_vcpu_hour: 0.03,
        },
        crate::cloud_runtime::prices::CpuFlavorPrice {
            id: "cpu5c".into(),
            name: "Compute-Optimized".into(),
            per_vcpu_hour: 0.035,
        },
    ];
    let preferences = Preferences {
        cpu_flavors: vec!["cpu3c".into(), "cpu5c".into()],
        gpu_types: Vec::new(),
    };
    let hourly = selection(false).hourly(&list, &preferences).unwrap();
    assert!((hourly - 0.14).abs() < 1e-9, "{hourly}");
    list.cpu.clear();
    assert_eq!(selection(false).hourly(&list, &preferences), None);
}
