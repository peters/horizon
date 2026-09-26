//! Hourly prices and availability a provider publishes, so a worker can be chosen
//! before any compute is requested. Each deployable provider supplies its own list.

/// Best first, so the derived order ranks offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Availability {
    High,
    Medium,
    Low,
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CpuFlavorPrice {
    pub id: String,
    pub name: String,
    pub per_vcpu_hour: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GpuPrice {
    pub id: String,
    pub name: String,
    pub memory_gb: u16,
    pub hourly: f64,
}

/// An allowed data center, with the GPU types it has in stock.
#[derive(Clone, Debug, PartialEq)]
pub struct DataCenter {
    pub id: String,
    /// The provider's region, such as `EUROPE`.
    pub region: String,
    /// Whether CPU clouds can keep their workspace volume here.
    pub workspace_storage: bool,
    /// Availability of each GPU type listed here.
    pub gpus: Vec<(String, Availability)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PriceList {
    /// Shown to people, such as `RunPod`.
    pub provider: &'static str,
    pub cpu: Vec<CpuFlavorPrice>,
    pub gpus: Vec<GpuPrice>,
    /// The allowed data centers.
    pub data_centers: Vec<DataCenter>,
    /// The region of every data center, allowed or not, for naming where a worker
    /// landed after the allowed set changes.
    pub regions: std::collections::BTreeMap<String, String>,
    pub storage: StoragePrices,
}

/// Storage prices per GB and month.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StoragePrices {
    /// Network volume rate for the first `network_tier_gb` GB, billed while the
    /// worker is stopped too.
    pub network: f64,
    pub network_tier_gb: u32,
    /// Network volume rate for the part beyond `network_tier_gb`.
    pub network_beyond: f64,
    /// Pod volume disk, where GPU workers keep their files: running, then stopped.
    pub pod_volume: (f64, f64),
    /// Container disk, billed only while the worker runs and cleared when it stops.
    pub container: f64,
    /// When these list prices were last checked, since providers may not publish them
    /// in a machine-readable form.
    pub confirmed: &'static str,
}

impl StoragePrices {
    /// Monthly price of a network volume of `gb` GB.
    #[must_use]
    pub fn network_month(&self, gb: u32) -> f64 {
        let first = gb.min(self.network_tier_gb);
        f64::from(first) * self.network + f64::from(gb - first) * self.network_beyond
    }

    /// Monthly price of a pod volume of `gb` GB while running and while stopped.
    #[must_use]
    pub fn pod_volume_month(&self, gb: u32) -> (f64, f64) {
        let (running, stopped) = self.pod_volume;
        (f64::from(gb) * running, f64::from(gb) * stopped)
    }
}

impl PriceList {
    /// Hourly price range of `vcpu` vCPUs across `flavors`, cheapest first. `None` unless
    /// every flavor has a price: an unpriced one may be the one allocated.
    #[must_use]
    pub fn cpu_hourly(&self, flavors: &[String], vcpu: u16) -> Option<(f64, f64)> {
        let prices: Vec<f64> = flavors
            .iter()
            .map(|id| {
                let flavor = self.cpu.iter().find(|flavor| &flavor.id == id)?;
                Some(flavor.per_vcpu_hour * f64::from(vcpu))
            })
            .collect::<Option<_>>()?;
        prices.into_iter().fold(None, |range, price| match range {
            None => Some((price, price)),
            Some((low, high)) => Some((f64::min(low, price), f64::max(high, price))),
        })
    }

    #[must_use]
    pub fn gpu(&self, id: &str) -> Option<&GpuPrice> {
        self.gpus.iter().find(|gpu| gpu.id == id)
    }

    /// Best availability of GPU type `id` in the data centers `within`, or in every
    /// allowed one when `within` is empty.
    #[must_use]
    pub fn gpu_availability(&self, id: &str, within: &[String]) -> Availability {
        self.data_centers
            .iter()
            .filter(|center| within.is_empty() || within.contains(&center.id))
            .flat_map(|center| &center.gpus)
            .filter(|(gpu, _)| gpu == id)
            .map(|&(_, level)| level)
            .min()
            .unwrap_or(Availability::None)
    }

    /// The cheapest GPU available right now in `within` (every allowed data center when
    /// empty), for a hint when preferences are sold out.
    #[must_use]
    pub fn cheapest_available_gpu(&self, within: &[String]) -> Option<&GpuPrice> {
        self.gpus
            .iter()
            .filter(|gpu| self.gpu_availability(&gpu.id, within) != Availability::None)
            .min_by(|a, b| a.hourly.total_cmp(&b.hourly))
    }
}

/// Stock of one CPU worker size in the allowed data centers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SizeAvailability {
    /// Data centers with this exact size in stock; the others have none.
    pub centers: Vec<(String, Availability)>,
}

impl SizeAvailability {
    /// Best availability in the data centers `within`, or in every allowed one when empty.
    #[must_use]
    pub fn best(&self, within: &[String]) -> Availability {
        self.in_scope(within)
            .map(|&(_, level)| level)
            .min()
            .unwrap_or(Availability::None)
    }

    /// How many data centers in `within` (every allowed one when empty) have this size.
    #[must_use]
    pub fn count(&self, within: &[String]) -> usize {
        self.in_scope(within).count()
    }

    fn in_scope<'a>(&'a self, within: &'a [String]) -> impl Iterator<Item = &'a (String, Availability)> {
        self.centers
            .iter()
            .filter(move |(id, _)| within.is_empty() || within.contains(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> PriceList {
        let flavor = |id: &str, per_vcpu_hour| CpuFlavorPrice {
            id: id.into(),
            name: id.into(),
            per_vcpu_hour,
        };
        let gpu = |id: &str, hourly| GpuPrice {
            id: id.into(),
            name: id.into(),
            memory_gb: 24,
            hourly,
        };
        let center = |id: &str, gpus: &[(&str, Availability)]| DataCenter {
            id: id.into(),
            region: "EUROPE".into(),
            workspace_storage: true,
            gpus: gpus.iter().map(|&(gpu, level)| (gpu.into(), level)).collect(),
        };
        PriceList {
            provider: "RunPod",
            cpu: vec![flavor("cpu3c", 0.03), flavor("cpu5c", 0.035), flavor("cpu3g", 0.04)],
            gpus: vec![gpu("a6000", 0.49), gpu("ada", 0.28), gpu("l4", 0.49)],
            data_centers: vec![
                center("EU-RO-1", &[("a6000", Availability::None), ("ada", Availability::Low)]),
                center("EU-SE-1", &[("l4", Availability::High)]),
            ],
            regions: std::collections::BTreeMap::new(),
            storage: crate::runpod::prices::STORAGE,
        }
    }

    #[test]
    fn a_cpu_size_is_priced_across_the_flavors_that_would_be_requested() {
        let list = list();
        let range = list.cpu_hourly(&["cpu3c".into(), "cpu5c".into()], 8).unwrap();
        assert!((range.0 - 0.24).abs() < 1e-9 && (range.1 - 0.28).abs() < 1e-9);
        assert!(list.cpu_hourly(&["unknown".into()], 8).is_none());
        // An unpriced flavor could be the one allocated, so no partial range is shown.
        assert!(list.cpu_hourly(&["cpu3c".into(), "cpu5m".into()], 8).is_none());
        assert!(list.cpu_hourly(&[], 8).is_none());
    }

    #[test]
    fn network_volumes_are_cheaper_only_beyond_the_first_tier() {
        let storage = list().storage;
        assert!((storage.network_month(100) - 7.0).abs() < 1e-9);
        assert!((storage.network_month(1000) - 70.0).abs() < 1e-9);
        assert!((storage.network_month(1500) - 95.0).abs() < 1e-9);
        let (running, stopped) = storage.pod_volume_month(200);
        assert!((running - 20.0).abs() < 1e-9 && (stopped - 40.0).abs() < 1e-9);
    }

    #[test]
    fn gpu_stock_and_the_cheapest_gpu_follow_the_chosen_data_centers() {
        let list = list();
        assert_eq!(list.cheapest_available_gpu(&[]).unwrap().id, "ada");
        assert_eq!(list.cheapest_available_gpu(&["EU-SE-1".into()]).unwrap().id, "l4");
        assert_eq!(list.gpu_availability("l4", &[]), Availability::High);
        assert_eq!(list.gpu_availability("l4", &["EU-RO-1".into()]), Availability::None);
        assert_eq!(list.gpu_availability("a6000", &[]), Availability::None);
        assert!(Availability::High < Availability::Low && Availability::Low < Availability::None);
    }

    #[test]
    fn size_stock_counts_only_the_chosen_data_centers() {
        let size = SizeAvailability {
            centers: vec![
                ("EU-RO-1".into(), Availability::Low),
                ("US-MO-2".into(), Availability::High),
            ],
        };
        assert_eq!((size.best(&[]), size.count(&[])), (Availability::High, 2));
        let europe = ["EU-RO-1".to_owned(), "EUR-IS-1".to_owned()];
        assert_eq!((size.best(&europe), size.count(&europe)), (Availability::Low, 1));
        assert_eq!(size.best(&["AP-JP-1".into()]), Availability::None);
    }
}
