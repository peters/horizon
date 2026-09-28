//! What a new cloud costs: a month running, a month stopped, and every kind of storage
//! it is billed for.
use horizon_core::cloud_runtime::prices::{PriceList, Profile};

/// Hours in an average month, for monthly totals.
const MONTH_HOURS: f64 = 730.0;

/// One kind of storage a cloud is billed for, per month.
#[derive(Debug, PartialEq)]
pub(super) struct Storage {
    pub kind: &'static str,
    pub gb: u16,
    pub running: f64,
    pub quoted: bool,
    /// `None` when it is not billed while the cloud is stopped.
    pub stopped: Option<f64>,
    pub note: &'static str,
}

pub(super) fn storage(list: &PriceList, profile: &Profile) -> Vec<Storage> {
    let rates = list.storage;
    let volume = profile.storage.volume_gb;
    let mut items = Vec::new();
    if profile.gpu {
        let (running, stopped) = rates.pod_volume_month(u32::from(volume));
        items.push(Storage {
            kind: "Pod volume",
            gb: volume,
            running,
            quoted: true,
            stopped: Some(stopped),
            note: "The pod volume holds your files and costs twice as much while stopped.",
        });
    } else {
        let quoted = profile.storage.standard_tier();
        let month = if quoted {
            rates.network_month(u32::from(volume))
        } else {
            0.0
        };
        items.push(Storage {
            kind: if quoted {
                "Network volume"
            } else {
                "High-performance network volume"
            },
            gb: volume,
            running: month,
            quoted,
            stopped: Some(month),
            note: if quoted {
                "The network volume holds your files and stays in its data center."
            } else {
                "High-performance storage pricing varies by data center; confirm the provider quote before starting."
            },
        });
    }
    let container = profile.storage.container_gb;
    items.push(Storage {
        kind: "Container disk",
        gb: container,
        running: f64::from(container) * rates.container,
        quoted: true,
        stopped: None,
        note: "The container disk is cleared when the cloud stops.",
    });
    items.retain(|item| item.gb > 0);
    items
}

/// Monthly totals running all month and stopped all month, as low and high bounds.
pub(super) fn monthly(hourly: (f64, f64), storage: &[Storage]) -> Option<((f64, f64), f64)> {
    if storage.iter().any(|item| !item.quoted) {
        return None;
    }
    let running: f64 = storage.iter().map(|item| item.running).sum();
    let stopped = storage.iter().filter_map(|item| item.stopped).sum();
    let (low, high) = hourly;
    Some(((low * MONTH_HOURS + running, high * MONTH_HOURS + running), stopped))
}

pub(super) fn money(value: f64) -> String {
    if value >= 100.0 {
        format!("${value:.0}")
    } else {
        format!("${value:.2}")
    }
}

/// One amount when both ends read the same once rounded, otherwise a range.
pub(super) fn range(low: f64, high: f64) -> String {
    let (low, high) = (money(low), money(high));
    if low == high {
        low
    } else {
        format!("{low}–{}", high.trim_start_matches('$'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::cloud_runtime::prices::RUNPOD_STORAGE;

    fn list() -> PriceList {
        PriceList {
            provider: "RunPod",
            cpu: Vec::new(),
            gpus: Vec::new(),
            data_centers: Vec::new(),
            regions: std::collections::BTreeMap::new(),
            storage: RUNPOD_STORAGE,
        }
    }

    fn profile(gpu: bool, container_gb: u16, volume_gb: u16) -> Profile {
        let mut profile = horizon_core::cloud_panel::CloudConfig::parse(
            "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker:latest\n    cpu: 8\n    memory_gb: 16\n",
        )
        .unwrap()
        .profiles["dev"]
        .clone();
        profile.gpu = gpu;
        profile.storage.container_gb = container_gb;
        profile.storage.volume_gb = volume_gb;
        profile
    }

    #[test]
    fn premium_storage_never_uses_standard_prices_or_false_totals() {
        let mut profile = profile(false, 20, 100);
        profile.storage.volume_tier = serde_json::from_value(serde_json::json!("HIGH_PERFORMANCE")).unwrap();
        let items = storage(&list(), &profile);
        assert!(!items[0].quoted);
        assert!(items[1].quoted);
        assert!(monthly((0.24, 0.28), &items).is_none());
    }

    #[test]
    fn prices_read_naturally() {
        assert_eq!(money(0.24), "$0.24");
        assert_eq!(money(5.76), "$5.76");
        assert_eq!(money(242.0), "$242");
        assert_eq!(range(0.24, 0.24), "$0.24");
        assert_eq!(range(0.24, 0.28), "$0.24–0.28");
        assert_eq!(range(0.244, 0.248), "$0.24–0.25");
        assert_eq!(range(0.241, 0.244), "$0.24");
        assert_eq!(range(1.92, 2.24), "$1.92–2.24");
    }

    #[test]
    fn a_cpu_cloud_keeps_its_network_volume_while_stopped() {
        let items = storage(&list(), &profile(false, 40, 100));
        let kinds: Vec<(&str, u16)> = items.iter().map(|item| (item.kind, item.gb)).collect();
        assert_eq!(kinds, [("Network volume", 100), ("Container disk", 40)]);
        let ((low, high), stopped) = monthly((0.24, 0.28), &items).unwrap();
        // 730 hours of compute, plus $7 of network volume and $4 of container disk.
        assert!((low - 186.2).abs() < 1e-9 && (high - 215.4).abs() < 1e-9);
        assert!((stopped - 7.0).abs() < 1e-9);
    }

    #[test]
    fn a_stopped_gpu_cloud_pays_twice_for_its_pod_volume() {
        let items = storage(&list(), &profile(true, 50, 200));
        assert_eq!(items[0].kind, "Pod volume");
        assert!((items[0].running - 20.0).abs() < 1e-9);
        assert_eq!(items[0].stopped.map(|stopped| (stopped * 100.0).round()), Some(4000.0));
        assert_eq!(items[1].stopped, None);
        let (_, stopped) = monthly((0.49, 0.49), &items).unwrap();
        assert!((stopped - 40.0).abs() < 1e-9);
        assert!(storage(&list(), &profile(true, 0, 0)).is_empty());
    }
}
