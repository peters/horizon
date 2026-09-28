//! `RunPod` Secure Cloud prices and live availability, for choosing a worker before
//! requesting one. Community Cloud hosts are third parties and are not offered here.
use super::{
    RunPod,
    flavors::{self, Flavor},
    stock::Level,
    volumes::{Capacity, Catalog, Tier, candidates},
};
use crate::{
    Cancellation, CloudError, Profile,
    prices::{Availability, CpuFlavorPrice, DataCenter, GpuPrice, PriceList, SizeAvailability, StoragePrices},
    valid_id,
};
use serde::{Deserialize, de::DeserializeOwned};

/// `RunPod`'s storage list prices: standard network volumes at $0.07 per GB-month for
/// the first TB and $0.05 beyond it, pod volumes at $0.10 while running and $0.20 while
/// stopped, and container disks at $0.10 while running. The catalog does not publish them.
pub const STORAGE: StoragePrices = StoragePrices {
    network: 0.07,
    network_tier_gb: 1000,
    network_beyond: 0.05,
    pod_volume: (0.10, 0.20),
    container: 0.10,
    confirmed: "2026-09-26",
};

impl RunPod {
    /// Secure Cloud prices for CPU flavors and GPU types, and the data centers in
    /// `data_centers` (every one when empty) with their region and GPU stock.
    /// # Errors
    /// Fails on provider errors and on malformed catalogs.
    pub fn price_list(&self, data_centers: &[String], cancel: &Cancellation) -> Result<PriceList, CloudError> {
        let cpus: CpuCatalog = self.catalog("/cpus", cancel)?;
        let gpus: GpuCatalog = self.catalog("/gpus", cancel)?;
        let centers: Catalog = self.catalog("/datacenters?include=GPU_AVAILABILITY,CPU_AVAILABILITY", cancel)?;
        let regions = centers
            .data_centers
            .iter()
            .filter(|center| valid_id(&center.id) && !center.region.is_empty())
            .map(|center| (center.id.clone(), center.region.clone()))
            .collect();
        let centers = centers
            .data_centers
            .into_iter()
            .filter(|center| valid_id(&center.id) && (data_centers.is_empty() || data_centers.contains(&center.id)))
            .map(|center| {
                let levels = |entries: Vec<Capacity>| {
                    entries
                        .into_iter()
                        .map(|entry| Ok((entry.id, availability(&entry.availability)?)))
                        .collect::<Result<Vec<_>, CloudError>>()
                };
                let holds = |tier: Tier| center.network_volume_types.iter().any(|value| value == tier.api_value());
                Ok(DataCenter {
                    workspace_storage: holds(Tier::Standard),
                    high_performance_storage: holds(Tier::HighPerformance),
                    gpus: levels(center.gpu_availability)?,
                    cpus: levels(center.cpu_availability)?,
                    id: center.id,
                    region: center.region,
                })
            })
            .collect::<Result<Vec<_>, CloudError>>()?;
        Ok(PriceList {
            provider: "RunPod",
            cpu: cpus
                .cpus
                .into_iter()
                .filter(|flavor| flavor.price.secure_per_vcpu > 0.0)
                .map(|flavor| CpuFlavorPrice {
                    id: flavor.id,
                    name: flavor.name,
                    per_vcpu_hour: flavor.price.secure_per_vcpu,
                })
                .collect(),
            gpus: gpus
                .gpus
                .into_iter()
                .filter(|gpu| gpu.secure && gpu.price.secure > 0.0)
                .map(|gpu| GpuPrice {
                    id: gpu.id,
                    name: gpu.name,
                    memory_gb: gpu.memory,
                    hourly: gpu.price.secure,
                })
                .collect(),
            data_centers: centers,
            regions,
            storage: STORAGE,
        })
    }

    /// Stock of the exact CPU size `profile` asks for, with the flavors a deployment
    /// would request from `preferred`, in `data_centers` (every one when empty).
    /// # Errors
    /// Rejects sizes no flavor offers and fails on provider errors.
    pub fn cpu_size_availability(
        &self,
        profile: &Profile,
        preferred: &[String],
        data_centers: &[String],
        cancel: &Cancellation,
    ) -> Result<SizeAvailability, CloudError> {
        let requested = flavors::for_profile(profile, preferred)?;
        let catalog: Catalog = self.catalog(
            &format!(
                "/datacenters?include=CPU_AVAILABILITY&networkVolumeTypes={}",
                profile.storage.volume_tier.api_value()
            ),
            cancel,
        )?;
        // Preserve compatible sold-out locations separately from the stock query.
        let compatible: Vec<String> = catalog
            .data_centers
            .iter()
            .filter(|center| {
                valid_id(&center.id)
                    && (data_centers.is_empty() || data_centers.contains(&center.id))
                    && center
                        .network_volume_types
                        .iter()
                        .any(|tier| tier == profile.storage.volume_tier.api_value())
            })
            .map(|center| center.id.clone())
            .collect();
        let centers: Vec<String> = candidates(catalog, data_centers, &requested, profile.storage.volume_tier)
            .into_iter()
            .map(|(_, id)| id)
            .collect();
        let flavors: Vec<&Flavor> = requested.iter().filter_map(|id| Flavor::get(id)).collect();
        let stock = self.cpu_stock(&centers, &flavors, profile.cpu, cancel)?;
        let mut centers: Vec<(String, Availability)> = compatible
            .into_iter()
            .map(|center| {
                let level = stock.get(&center).copied().map_or(Availability::None, Into::into);
                (center, level)
            })
            .collect();
        centers.sort();
        Ok(SizeAvailability { centers })
    }

    fn catalog<T: DeserializeOwned>(&self, path: &str, cancel: &Cancellation) -> Result<T, CloudError> {
        let url = format!("{}{path}", self.catalog_endpoint);
        serde_json::from_value(self.request_url("GET", &url, None, cancel, None)?)
            .map_err(|_| CloudError::InvalidResponse)
    }
}

impl From<Level> for Availability {
    fn from(level: Level) -> Self {
        match level {
            Level::High => Self::High,
            Level::Medium => Self::Medium,
            Level::Low => Self::Low,
        }
    }
}

/// Unknown values are malformed answers, never shown as out of stock.
fn availability(value: &str) -> Result<Availability, CloudError> {
    match value.to_ascii_uppercase().as_str() {
        "HIGH" => Ok(Availability::High),
        "MEDIUM" => Ok(Availability::Medium),
        "LOW" => Ok(Availability::Low),
        "NONE" => Ok(Availability::None),
        _ => Err(CloudError::InvalidResponse),
    }
}

#[derive(Deserialize)]
struct CpuCatalog {
    cpus: Vec<CpuFlavor>,
}
#[derive(Deserialize)]
struct CpuFlavor {
    id: String,
    name: String,
    price: CpuPrice,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CpuPrice {
    #[serde(default)]
    secure_per_vcpu: f64,
}
#[derive(Deserialize)]
struct GpuCatalog {
    gpus: Vec<GpuType>,
}
#[derive(Deserialize)]
struct GpuType {
    id: String,
    name: String,
    memory: u16,
    #[serde(default)]
    secure: bool,
    price: GpuTypePrice,
}
#[derive(Deserialize)]
struct GpuTypePrice {
    #[serde(default)]
    secure: f64,
}
