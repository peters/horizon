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
    /// Best availability across the allowed data centers.
    pub availability: Availability,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PriceList {
    /// Shown to people, such as `RunPod`.
    pub provider: &'static str,
    pub cpu: Vec<CpuFlavorPrice>,
    pub gpus: Vec<GpuPrice>,
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

    /// The cheapest GPU available right now, for a hint when preferences are sold out.
    #[must_use]
    pub fn cheapest_available_gpu(&self) -> Option<&GpuPrice> {
        self.gpus
            .iter()
            .filter(|gpu| gpu.availability != Availability::None)
            .min_by(|a, b| a.hourly.total_cmp(&b.hourly))
    }
}

/// Availability of one CPU worker size across the allowed data centers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SizeAvailability {
    pub best: Availability,
    /// Data centers with this exact size in stock.
    pub centers: usize,
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
        let gpu = |id: &str, hourly, availability| GpuPrice {
            id: id.into(),
            name: id.into(),
            memory_gb: 24,
            hourly,
            availability,
        };
        PriceList {
            provider: "RunPod",
            cpu: vec![flavor("cpu3c", 0.03), flavor("cpu5c", 0.035), flavor("cpu3g", 0.04)],
            gpus: vec![
                gpu("a6000", 0.49, Availability::None),
                gpu("ada", 0.28, Availability::Low),
                gpu("l4", 0.49, Availability::High),
            ],
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
    fn the_cheapest_available_gpu_skips_sold_out_types() {
        let list = list();
        assert_eq!(list.cheapest_available_gpu().unwrap().id, "ada");
        assert_eq!(list.gpu("l4").unwrap().availability, Availability::High);
        assert!(Availability::High < Availability::Low && Availability::Low < Availability::None);
    }
}
