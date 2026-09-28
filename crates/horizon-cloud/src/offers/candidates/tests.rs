use super::*;
use crate::{
    hetzner::catalog,
    prices::{CpuFlavorPrice, DataCenter, GpuPrice},
    runpod::prices::STORAGE,
};

fn profile(provider: &str, (cpu, memory_gb): (u16, u16)) -> Profile {
    let mut profile = crate::CloudConfig::parse(crate::EXAMPLE).unwrap().profiles["image-only"].clone();
    profile.provider = provider.to_owned();
    profile.cpu = cpu;
    profile.memory_gb = memory_gb;
    profile
}

fn list() -> PriceList {
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
            GpuPrice {
                id: "NVIDIA RTX A5000".into(),
                name: "RTX A5000".into(),
                memory_gb: 24,
                hourly: 0.27,
            },
            GpuPrice {
                id: "NVIDIA A40".into(),
                name: "A40".into(),
                memory_gb: 48,
                hourly: 0.4,
            },
        ],
        data_centers: vec![DataCenter {
            id: "EU-RO-1".into(),
            region: "EUROPE".into(),
            workspace_storage: true,
            gpus: vec![
                ("NVIDIA RTX A5000".into(), Availability::None),
                ("NVIDIA A40".into(), Availability::High),
            ],
        }],
        regions: std::collections::BTreeMap::new(),
        storage: STORAGE,
    }
}

fn preferences() -> Preferences {
    Preferences {
        cpu_flavors: vec!["cpu3g".into()],
        gpu_types: Vec::new(),
    }
}

fn catalog() -> Catalog {
    let offer = |server_type: &str, location: &str, (cores, memory_gb): (u32, f64), hourly_eur| catalog::Offer {
        server_type: server_type.into(),
        location: location.into(),
        cores,
        memory_gb,
        disk_gb: 160,
        dedicated: false,
        hourly_eur,
        monthly_eur: hourly_eur * 600.0,
        available: true,
        recommended: false,
    };
    serde_json::from_value(serde_json::json!({
        "offers": [], "volume_gb_month_eur": 0.0572,
        "ipv4_month_eur": {"hel1": 0.5, "nbg1": 0.5}, "ipv4_hour_eur": {"hel1": 0.0008, "nbg1": 0.0008},
        "regions": {"hel1": "EUROPE", "nbg1": "EUROPE"},
    }))
    .map(|catalog: Catalog| Catalog {
        offers: vec![
            offer("cx43", "hel1", (8, 16.0), 0.0256),
            offer("cx53", "hel1", (16, 32.0), 0.0473),
            offer("cx53", "nbg1", (16, 32.0), 0.0473),
        ],
        ..catalog
    })
    .unwrap()
}

fn configured<'a>(list: &'a PriceList, preferences: &'a Preferences, hetzner: HetznerSource<'a>) -> Sources<'a> {
    Sources {
        runpod: Some((list, preferences)),
        hetzner: Some(hetzner),
    }
}

const RATE: ExchangeRate = ExchangeRate { usd_per_eur: 1.1 };

#[test]
fn a_rate_ranks_both_currencies_by_running_cost() {
    let (list, preferences, catalog) = (list(), preferences(), catalog());
    let types = ["cx43".to_owned(), "cx53".to_owned()];
    let sources = configured(
        &list,
        &preferences,
        HetznerSource {
            catalog: &catalog,
            server_types: &types,
            locations: &[],
        },
    );
    let ranked = candidates(&profile("runpod", (8, 32)), &sources, Some(RATE));
    let names: Vec<_> = ranked
        .iter()
        .map(|candidate| (candidate.provider.id, candidate.name.as_str()))
        .collect();
    // cx43 has too little memory, so cx53 is the type Hetzner gets first.
    assert_eq!(names, [("hetzner", "cx53"), ("runpod", "8 vCPU · 32 GB")]);
    let (hetzner, runpod) = (&ranked[0], &ranked[1]);
    assert_eq!((hetzner.currency, hetzner.location.as_deref()), ("EUR", Some("hel1")));
    assert!((hetzner.hourly - 0.0473).abs() < 1e-9, "compute alone is shown");
    // 20 GB volume and an IPv4 address are billed while it runs.
    let expected = 0.0473 + 0.0572 * 20.0 / 730.0 + 0.0008;
    assert!((hetzner.running_hourly - expected).abs() < 1e-9);
    assert!((runpod.hourly - 0.32).abs() < 1e-9);
    assert!(
        runpod.running_hourly > runpod.hourly,
        "the network volume is billed too"
    );
    // A rate that makes euros dearer than the dollar price puts RunPod first.
    let dear = ExchangeRate { usd_per_eur: 100.0 };
    let ranked = candidates(&profile("runpod", (8, 32)), &sources, Some(dear));
    assert_eq!(ranked[0].provider, &provider::RUNPOD);
}

#[test]
fn without_a_rate_the_profiles_own_provider_comes_first() {
    let (list, preferences, catalog) = (list(), preferences(), catalog());
    let types = ["cx53".to_owned()];
    let locations = ["nbg1".to_owned(), "hel1".to_owned()];
    let sources = configured(
        &list,
        &preferences,
        HetznerSource {
            catalog: &catalog,
            server_types: &types,
            locations: &locations,
        },
    );
    let ranked = candidates(&profile("runpod", (8, 32)), &sources, None);
    assert_eq!(ranked[0].provider, &provider::RUNPOD);
    assert_eq!(
        ranked[1].location.as_deref(),
        Some("nbg1"),
        "allowed locations keep their order"
    );
    let ranked = candidates(&profile("hetzner", (8, 32)), &sources, None);
    assert_eq!(ranked[0].provider, &provider::HETZNER);
    assert_eq!(ranked.len(), 2);
}

#[test]
fn a_provider_without_a_worker_of_this_size_is_left_out() {
    let (list, preferences, catalog) = (list(), preferences(), catalog());
    let types = ["cx43".to_owned()];
    let sources = configured(
        &list,
        &preferences,
        HetznerSource {
            catalog: &catalog,
            server_types: &types,
            locations: &[],
        },
    );
    // No configured Hetzner type has 32 GB.
    let ranked = candidates(&profile("hetzner", (8, 32)), &sources, Some(RATE));
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].provider, &provider::RUNPOD);
    // RunPod has no 3 vCPU worker; Hetzner's cx43 has at least that.
    let ranked = candidates(&profile("runpod", (3, 8)), &sources, Some(RATE));
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].provider, &provider::HETZNER);
    // Prices that have not arrived offer nothing.
    assert!(candidates(&profile("runpod", (8, 32)), &Sources::default(), Some(RATE)).is_empty());
}

#[test]
fn runpod_is_left_out_when_no_allowed_data_center_holds_the_workspace() {
    let (mut list, preferences, catalog) = (list(), preferences(), catalog());
    list.data_centers[0].workspace_storage = false;
    let types = ["cx53".to_owned()];
    let sources = configured(
        &list,
        &preferences,
        HetznerSource {
            catalog: &catalog,
            server_types: &types,
            locations: &[],
        },
    );
    let ranked = candidates(&profile("runpod", (8, 32)), &sources, None);
    assert_eq!(
        ranked.iter().map(|candidate| candidate.provider).collect::<Vec<_>>(),
        [&provider::HETZNER]
    );
    // The list records standard-tier support only, so a premium profile is not held back.
    let mut premium = profile("runpod", (8, 32));
    premium.storage.volume_tier = crate::runpod::volumes::Tier::HighPerformance;
    assert_eq!(candidates(&premium, &sources, None)[0].provider, &provider::RUNPOD);
}

#[test]
fn a_profile_hetzner_cannot_run_is_ranked_on_runpod_alone() {
    let (list, preferences, catalog) = (list(), preferences(), catalog());
    let types = ["cx53".to_owned()];
    let sources = configured(
        &list,
        &preferences,
        HetznerSource {
            catalog: &catalog,
            server_types: &types,
            locations: &[],
        },
    );
    // A premium network volume is a RunPod-only choice.
    let mut premium = profile("runpod", (8, 32));
    premium.storage.volume_tier = crate::runpod::volumes::Tier::HighPerformance;
    assert!(!provider::HETZNER.supports(&premium));
    let ranked = candidates(&premium, &sources, Some(RATE));
    assert_eq!(
        ranked.iter().map(|candidate| candidate.provider).collect::<Vec<_>>(),
        [&provider::RUNPOD]
    );
    // Hetzner has no GPUs; a GPU profile gets RunPod's cheapest type in stock.
    let mut gpu = profile("runpod", (8, 32));
    gpu.gpu = true;
    let ranked = candidates(&gpu, &sources, Some(RATE));
    assert_eq!(ranked.len(), 1);
    assert_eq!(
        (ranked[0].provider, ranked[0].name.as_str()),
        (&provider::RUNPOD, "A40")
    );
}

#[test]
fn preferred_gpu_types_are_asked_for_in_order() {
    let mut list = list();
    list.gpus.push(GpuPrice {
        id: "NVIDIA RTX 4090".into(),
        name: "RTX 4090".into(),
        memory_gb: 24,
        hourly: 0.2,
    });
    list.data_centers[0].gpus = vec![
        ("NVIDIA RTX A5000".into(), Availability::Low),
        ("NVIDIA A40".into(), Availability::High),
        ("NVIDIA RTX 4090".into(), Availability::High),
    ];
    let preferences = Preferences {
        cpu_flavors: Vec::new(),
        gpu_types: vec!["NVIDIA A40".into(), "NVIDIA RTX 4090".into()],
    };
    let sources = Sources {
        runpod: Some((&list, &preferences)),
        hetzner: None,
    };
    let mut gpu = profile("runpod", (8, 32));
    gpu.gpu = true;
    // The first preference in stock, although the second is cheaper.
    assert_eq!(candidates(&gpu, &sources, None)[0].name, "A40");
    // Without preferences, the cheapest in stock.
    let any = Preferences::default();
    let sources = Sources {
        runpod: Some((&list, &any)),
        hetzner: None,
    };
    assert_eq!(candidates(&gpu, &sources, None)[0].name, "RTX 4090");
}
