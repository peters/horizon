//! Ranked compute offers for given requirements, from a provider's price list, so an
//! agent can choose the cheapest suitable worker before anything is rented. Read only:
//! nothing here allocates compute.
use horizon_cloud::prices::{Availability, DataCenter, PriceList};
use horizon_cloud::runpod::{
    flavors::{FLAVORS, VCPU_COUNTS},
    volumes::REQUEST_SIZE_GB,
};
use serde::{Deserialize, Serialize};

/// Hours in an average month, for prorating storage over the expected duration.
const MONTH_HOURS: f64 = 730.0;
const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 50;
const DEFAULT_STORAGE_GB: u16 = 20;
/// The longest expected duration priced: a year.
const MAX_HOURS: f64 = 24.0 * 366.0;

/// What the work needs. Every field is optional; an empty request lists the cheapest CPU
/// workers.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Requirements {
    #[serde(default)]
    pub min_vcpu: Option<u16>,
    #[serde(default)]
    pub min_memory_gb: Option<u16>,
    /// GPU workers instead of CPU workers.
    #[serde(default)]
    pub gpu: bool,
    #[serde(default)]
    pub min_gpu_memory_gb: Option<u16>,
    /// A provider GPU type ID or name, such as `NVIDIA RTX A5000` or `RTX A5000`.
    #[serde(default)]
    pub gpu_type: Option<String>,
    #[serde(default)]
    pub max_hourly: Option<f64>,
    /// Expected running time for the estimated total, at most a year. One hour when
    /// omitted.
    #[serde(default)]
    pub hours: Option<f64>,
    /// Workspace storage priced into the estimate: 10 to 4000 GB for CPU workers, at
    /// least 1 GB for GPU workers, and 20 GB when omitted.
    #[serde(default)]
    pub storage_gb: Option<u16>,
    /// A provider region, such as `EUROPE` or `North America`.
    #[serde(default)]
    pub region: Option<String>,
    /// Also list GPU types with no stock where the worker may go.
    #[serde(default)]
    pub include_unavailable: bool,
    /// At most this many offers, cheapest first: 1 to 50, and 10 when omitted.
    #[serde(default)]
    pub limit: Option<usize>,
}

impl Requirements {
    /// # Errors
    /// Rejects negative or non-finite amounts, more than a year of hours, requirements for
    /// the other kind of worker, an empty GPU type or region, storage Horizon could not
    /// create, and a limit outside 1 to 50.
    pub fn validate(&self) -> Result<(), &'static str> {
        let finite = |value: Option<f64>| value.is_none_or(|value| value.is_finite() && value >= 0.0);
        if !finite(self.max_hourly) || !finite(self.hours) {
            return Err("Prices and hours must be zero or more");
        }
        if self.hours.is_some_and(|hours| hours > MAX_HOURS) {
            return Err("Hours must be at most a year");
        }
        // A requirement the offers cannot meet or report is refused, never ignored.
        if self.gpu && (self.min_vcpu.is_some() || self.min_memory_gb.is_some()) {
            return Err("vCPU and memory minimums apply to CPU workers; GPU offers are chosen by GPU memory or type");
        }
        if !self.gpu && (self.min_gpu_memory_gb.is_some() || self.gpu_type.is_some() || self.include_unavailable) {
            return Err("GPU memory, GPU type and include_unavailable need gpu=true");
        }
        if self.gpu_type.as_deref().is_some_and(|value| value.trim().is_empty())
            || self.region.as_deref().is_some_and(|value| value.trim().is_empty())
        {
            return Err("GPU type and region must not be empty");
        }
        if self
            .storage_gb
            .is_some_and(|size| size == 0 || (!self.gpu && !REQUEST_SIZE_GB.contains(&u32::from(size))))
        {
            return Err("Workspace storage must be 10 to 4000 GB for CPU workers and at least 1 GB for GPU workers");
        }
        if self.limit.is_some_and(|limit| !(1..=MAX_LIMIT).contains(&limit)) {
            return Err("The limit must be 1 to 50");
        }
        Ok(())
    }
}

/// One worker the provider offers, priced for the requirements.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Offer {
    pub provider: &'static str,
    /// `cpu` or `gpu`.
    pub kind: &'static str,
    /// The CPU flavor or GPU type ID to request.
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vcpu: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_gb: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_memory_gb: Option<u16>,
    pub hourly: f64,
    /// Compute for the expected hours plus the workspace storage for that time.
    pub estimated_total: f64,
    /// `high`, `medium`, `low`, `none`, or `checked_at_creation` for CPU sizes, whose exact
    /// stock is confirmed when a cloud is created.
    pub availability: &'static str,
    /// Regions with this GPU type in stock.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub regions_in_stock: Vec<String>,
    /// Provider-operated hosts only; third-party hosts are not offered.
    pub host: &'static str,
    pub interruptible: bool,
    /// Horizon can deploy this today.
    pub rentable: bool,
}

/// Offers in `list` meeting `requirements`, cheapest estimated total first.
#[must_use]
pub fn offers(list: &PriceList, requirements: &Requirements) -> Vec<Offer> {
    let hours = requirements.hours.unwrap_or(1.0);
    let storage_gb = u32::from(requirements.storage_gb.unwrap_or(DEFAULT_STORAGE_GB));
    let within = region_centers(list, requirements.region.as_deref());
    let mut offers = if requirements.gpu {
        gpu_offers(list, requirements, &within, hours, storage_gb)
    } else {
        cpu_offers(list, requirements, &within, hours, storage_gb)
    };
    offers.retain(|offer| requirements.max_hourly.is_none_or(|max| offer.hourly <= max));
    offers.sort_by(|a, b| {
        a.estimated_total
            .total_cmp(&b.estimated_total)
            .then_with(|| a.name.cmp(&b.name))
    });
    offers.truncate(requirements.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT));
    offers
}

fn cpu_offers(
    list: &PriceList,
    requirements: &Requirements,
    within: &[String],
    hours: f64,
    storage_gb: u32,
) -> Vec<Offer> {
    // A CPU worker keeps its workspace on a network volume, so the region needs a data
    // center that can hold one. Exact CPU stock is checked when the cloud is created.
    let hosts_workspace = list
        .data_centers
        .iter()
        .any(|center| center.workspace_storage && (within.is_empty() || within.contains(&center.id)));
    if !hosts_workspace {
        return Vec::new();
    }
    // The network volume is billed whether the worker runs or not.
    let storage = list.storage.network_month(storage_gb) * hours / MONTH_HOURS;
    let mut offers = Vec::new();
    for price in &list.cpu {
        let Some(flavor) = FLAVORS.iter().find(|flavor| flavor.id == price.id) else {
            continue;
        };
        for vcpu in VCPU_COUNTS {
            let memory_gb = vcpu * flavor.memory_per_vcpu;
            if vcpu < requirements.min_vcpu.unwrap_or(0) || memory_gb < requirements.min_memory_gb.unwrap_or(0) {
                continue;
            }
            let hourly = price.per_vcpu_hour * f64::from(vcpu);
            offers.push(Offer {
                provider: list.provider,
                kind: "cpu",
                id: price.id.clone(),
                name: format!("{} · {vcpu} vCPU · {memory_gb} GB", price.name),
                vcpu: Some(vcpu),
                memory_gb: Some(memory_gb),
                gpu_memory_gb: None,
                hourly,
                estimated_total: hourly * hours + storage,
                availability: "checked_at_creation",
                regions_in_stock: Vec::new(),
                host: "provider_operated",
                interruptible: false,
                rentable: true,
            });
        }
    }
    offers
}

fn gpu_offers(
    list: &PriceList,
    requirements: &Requirements,
    within: &[String],
    hours: f64,
    storage_gb: u32,
) -> Vec<Offer> {
    // A GPU worker keeps its files on its pod volume, billed at the running rate here.
    let (running, _) = list.storage.pod_volume_month(storage_gb);
    let storage = running * hours / MONTH_HOURS;
    let wanted = requirements.gpu_type.as_deref().map(str::trim);
    list.gpus
        .iter()
        .filter(|gpu| gpu.memory_gb >= requirements.min_gpu_memory_gb.unwrap_or(0))
        .filter(|gpu| {
            wanted.is_none_or(|wanted| gpu.id.eq_ignore_ascii_case(wanted) || gpu.name.eq_ignore_ascii_case(wanted))
        })
        .filter_map(|gpu| {
            let availability = list.gpu_availability(&gpu.id, within);
            if availability == Availability::None && !requirements.include_unavailable {
                return None;
            }
            Some(Offer {
                provider: list.provider,
                kind: "gpu",
                id: gpu.id.clone(),
                name: gpu.name.clone(),
                vcpu: None,
                memory_gb: None,
                gpu_memory_gb: Some(gpu.memory_gb),
                hourly: gpu.hourly,
                estimated_total: gpu.hourly * hours + storage,
                availability: level(availability),
                regions_in_stock: regions_in_stock(list, &gpu.id, within),
                host: "provider_operated",
                interruptible: false,
                rentable: true,
            })
        })
        .collect()
}

/// Data centers in `region`, or none (meaning every allowed one) without a region.
fn region_centers(list: &PriceList, region: Option<&str>) -> Vec<String> {
    let Some(region) = region else {
        return Vec::new();
    };
    let wanted = normalize(region);
    let centers: Vec<String> = list
        .data_centers
        .iter()
        .filter(|center| normalize(&center.region) == wanted)
        .map(|center| center.id.clone())
        .collect();
    // A region with no allowed data center matches nothing rather than everything.
    if centers.is_empty() {
        vec![String::new()]
    } else {
        centers
    }
}

fn regions_in_stock(list: &PriceList, gpu: &str, within: &[String]) -> Vec<String> {
    let mut regions: Vec<String> = list
        .data_centers
        .iter()
        .filter(|center| within.is_empty() || within.contains(&center.id))
        .filter(|center| stocked(center, gpu))
        .map(|center| center.region.clone())
        .filter(|region| !region.is_empty())
        .collect();
    regions.sort();
    regions.dedup();
    regions
}

fn stocked(center: &DataCenter, gpu: &str) -> bool {
    center
        .gpus
        .iter()
        .any(|(id, level)| id == gpu && *level != Availability::None)
}

fn normalize(region: &str) -> String {
    region.trim().to_ascii_uppercase().replace([' ', '-'], "_")
}

fn level(availability: Availability) -> &'static str {
    match availability {
        Availability::High => "high",
        Availability::Medium => "medium",
        Availability::Low => "low",
        Availability::None => "none",
    }
}

#[cfg(test)]
mod tests {
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
        let offers = offers(&list(), &requirements);
        let first = &offers[0];
        // 4 vCPU general purpose has 16 GB at $0.16/h, cheaper than 8 vCPU compute-optimized
        // (also 16 GB) at $0.24/h.
        assert_eq!(
            (first.id.as_str(), first.vcpu, first.memory_gb),
            ("cpu3g", Some(4), Some(16))
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
    fn cpu_offers_need_a_region_that_can_hold_the_workspace() {
        let cpu = |region: &str, list: &PriceList| {
            offers(
                list,
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
        assert!(
            gpu(Requirements {
                region: Some("ASIA".into()),
                ..Requirements::default()
            })
            .is_empty()
        );
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
}
