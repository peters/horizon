//! Ranked compute offers for given requirements, from a provider's price list, so an
//! agent can choose the cheapest suitable worker before anything is rented. Read only:
//! nothing here allocates compute.
use crate::prices::{Availability, DataCenter, Preferences, PriceList};
use crate::runpod::{
    flavors::{self, Flavor, VCPU_COUNTS},
    volumes::REQUEST_SIZE_GB,
};
use serde::{Deserialize, Serialize};

/// Hours in an average month, for prorating storage over the expected duration.
const MONTH_HOURS: f64 = 730.0;
/// `RunPod` bills in US dollars.
const USD: &str = "USD";
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
    /// The currency of every amount in this offer, as the provider bills it: `USD` for
    /// `RunPod` and `EUR` (net of VAT) for Hetzner. Amounts are never converted.
    pub currency: &'static str,
    /// `cpu` or `gpu`.
    pub kind: &'static str,
    /// The CPU size as `cpu-<vCPU>-<GB>`, or the GPU type ID to request.
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vcpu: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_gb: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_memory_gb: Option<u16>,
    /// For a CPU size, the highest price among `flavors`, since the provider picks one.
    pub hourly: f64,
    /// The CPU flavors a cloud of this size requests, from the preferences in cloud
    /// settings; the provider allocates one of them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flavors: Vec<String>,
    /// Compute for the expected hours plus the workspace storage for that time.
    pub estimated_total: f64,
    /// The most the worker's compute is billed in a month, where the provider caps it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monthly: Option<f64>,
    /// What a stopped cloud of this kind costs per month: its kept workspace storage.
    pub stopped_monthly: f64,
    /// The location the worker is created in, where the offer is for one location.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// `high`, `medium`, `low` or `none` for GPU types, and `checked_at_creation` for
    /// `RunPod` CPU sizes, whose exact stock is confirmed when a cloud is created. Hetzner
    /// offers report `listed` or `unlisted`: Hetzner's own flag, which is advisory, so an
    /// unlisted type can still be created and creation confirms either way.
    pub availability: &'static str,
    /// Regions with this GPU type in stock.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub regions_in_stock: Vec<String>,
    /// Provider-operated hosts only; third-party hosts are not offered.
    pub host: &'static str,
    pub interruptible: bool,
    /// Horizon can deploy this now: false for a GPU type out of stock wherever the worker
    /// may go.
    pub rentable: bool,
}

/// Offers in `list` meeting `requirements`, cheapest estimated total first. CPU sizes
/// request the flavors `preferences` choose, as a deployment would.
#[must_use]
pub fn offers(list: &PriceList, preferences: &Preferences, requirements: &Requirements) -> Vec<Offer> {
    let hours = requirements.hours.unwrap_or(1.0);
    let storage_gb = u32::from(requirements.storage_gb.unwrap_or(DEFAULT_STORAGE_GB));
    // Data centers in the requested region, or none (meaning every allowed one) without
    // a region. A region with no allowed data center has nothing to offer.
    let within = match requirements.region.as_deref() {
        Some(region) => region_centers(list, region),
        None => Vec::new(),
    };
    if requirements.region.is_some() && within.is_empty() {
        return Vec::new();
    }
    let mut offers = if requirements.gpu {
        gpu_offers(list, requirements, &within, hours, storage_gb)
    } else {
        cpu_offers(
            list,
            &preferences.cpu_flavors,
            requirements,
            &within,
            (hours, storage_gb),
        )
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
    preferred: &[String],
    requirements: &Requirements,
    within: &[String],
    (hours, storage_gb): (f64, u32),
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
    let mut offers: Vec<Offer> = Vec::new();
    for vcpu in VCPU_COUNTS {
        for (size_memory, _) in flavors::memory_options(vcpu, 0) {
            // The flavors a deployment of this size requests, never one on its own.
            let Ok(requested) = flavors::for_size((vcpu, size_memory), 0, preferred) else {
                continue;
            };
            let prices: Option<Vec<_>> = requested
                .iter()
                .map(|id| list.cpu.iter().find(|price| &price.id == id))
                .collect();
            let Some(prices) = prices else {
                continue;
            };
            // The memory every one of them guarantees.
            let memory_gb = requested
                .iter()
                .filter_map(|id| Flavor::get(id))
                .map(|flavor| vcpu * flavor.memory_per_vcpu)
                .min()
                .unwrap_or(size_memory);
            let duplicate = offers
                .iter()
                .any(|offer| offer.vcpu == Some(vcpu) && offer.flavors == requested);
            if duplicate
                || vcpu < requirements.min_vcpu.unwrap_or(0)
                || memory_gb < requirements.min_memory_gb.unwrap_or(0)
            {
                continue;
            }
            let hourly = prices.iter().map(|price| price.per_vcpu_hour).fold(0.0, f64::max) * f64::from(vcpu);
            let mut names: Vec<&str> = prices.iter().map(|price| price.name.as_str()).collect();
            names.dedup();
            offers.push(Offer {
                provider: list.provider,
                currency: USD,
                kind: "cpu",
                id: format!("cpu-{vcpu}-{memory_gb}"),
                name: format!("{} · {vcpu} vCPU · {memory_gb} GB", names.join(" or ")),
                vcpu: Some(vcpu),
                memory_gb: Some(memory_gb),
                gpu_memory_gb: None,
                hourly,
                flavors: requested,
                estimated_total: hourly * hours + storage,
                monthly: None,
                stopped_monthly: list.storage.network_month(storage_gb),
                location: None,
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
    let (running, stopped) = list.storage.pod_volume_month(storage_gb);
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
                currency: USD,
                kind: "gpu",
                id: gpu.id.clone(),
                name: gpu.name.clone(),
                vcpu: None,
                memory_gb: None,
                gpu_memory_gb: Some(gpu.memory_gb),
                hourly: gpu.hourly,
                flavors: Vec::new(),
                estimated_total: gpu.hourly * hours + storage,
                monthly: None,
                stopped_monthly: stopped,
                location: None,
                availability: level(availability),
                regions_in_stock: regions_in_stock(list, &gpu.id, within),
                host: "provider_operated",
                interruptible: false,
                rentable: availability != Availability::None,
            })
        })
        .collect()
}

/// Allowed data centers in `region`.
fn region_centers(list: &PriceList, region: &str) -> Vec<String> {
    let wanted = normalize(region);
    list.data_centers
        .iter()
        .filter(|center| normalize(&center.region) == wanted)
        .map(|center| center.id.clone())
        .collect()
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

mod hetzner;
pub use hetzner::hetzner;

#[cfg(test)]
mod tests;
