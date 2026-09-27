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
