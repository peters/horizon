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
    pub memory_per_vcpu: u16,
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
    /// Network storage per GB and month, billed while the worker is stopped too.
    pub storage_gb_month: f64,
}

impl PriceList {
    /// Hourly price range of `vcpu` vCPUs across `flavors`, cheapest first.
    #[must_use]
    pub fn cpu_hourly(&self, flavors: &[String], vcpu: u16) -> Option<(f64, f64)> {
        let prices = self
            .cpu
            .iter()
            .filter(|flavor| flavors.contains(&flavor.id))
            .map(|flavor| flavor.per_vcpu_hour * f64::from(vcpu));
        prices.fold(None, |range, price| match range {
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
        let flavor = |id: &str, per_vcpu_hour, memory_per_vcpu| CpuFlavorPrice {
            id: id.into(),
            name: id.into(),
            per_vcpu_hour,
            memory_per_vcpu,
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
            cpu: vec![
                flavor("cpu3c", 0.03, 2),
                flavor("cpu5c", 0.035, 2),
                flavor("cpu3g", 0.04, 4),
            ],
            gpus: vec![
                gpu("a6000", 0.49, Availability::None),
                gpu("ada", 0.28, Availability::Low),
                gpu("l4", 0.49, Availability::High),
            ],
            storage_gb_month: 0.07,
        }
    }

    #[test]
    fn a_cpu_size_is_priced_across_the_flavors_that_would_be_requested() {
        let list = list();
        let range = list.cpu_hourly(&["cpu3c".into(), "cpu5c".into()], 8).unwrap();
        assert!((range.0 - 0.24).abs() < 1e-9 && (range.1 - 0.28).abs() < 1e-9);
        assert!(list.cpu_hourly(&["unknown".into()], 8).is_none());
    }

    #[test]
    fn the_cheapest_available_gpu_skips_sold_out_types() {
        let list = list();
        assert_eq!(list.cheapest_available_gpu().unwrap().id, "ada");
        assert_eq!(list.gpu("l4").unwrap().availability, Availability::High);
        assert!(Availability::High < Availability::Low && Availability::Low < Availability::None);
    }
}
