//! `RunPod` Secure Cloud prices and live availability, for choosing a worker before
//! requesting one. Community Cloud hosts are third parties and are not offered here.
use super::{
    RunPod,
    flavors::{self, Flavor},
    stock::Level,
    volumes::{Catalog, candidates},
};
use crate::{
    Cancellation, CloudError, Profile,
    prices::{Availability, CpuFlavorPrice, GpuPrice, PriceList, SizeAvailability},
};
use serde::{Deserialize, de::DeserializeOwned};

/// `RunPod`'s list price for standard network storage (September 2026). The catalog
/// does not publish it.
pub const STORAGE_GB_MONTH: f64 = 0.07;

impl RunPod {
    /// Secure Cloud prices for CPU flavors and GPU types, with each GPU's best
    /// availability in `data_centers` (every data center when empty).
    /// # Errors
    /// Fails on provider errors and on malformed catalogs.
    pub fn price_list(&self, data_centers: &[String], cancel: &Cancellation) -> Result<PriceList, CloudError> {
        let cpus: CpuCatalog = self.catalog("/cpus", cancel)?;
        let gpus: GpuCatalog = self.catalog("/gpus", cancel)?;
        let centers: Catalog = self.catalog("/datacenters?include=GPU_AVAILABILITY", cancel)?;
        let mut best: std::collections::HashMap<String, Availability> = std::collections::HashMap::new();
        for center in centers
            .data_centers
            .into_iter()
            .filter(|center| data_centers.is_empty() || data_centers.contains(&center.id))
        {
            for gpu in center.gpu_availability {
                let level = availability(&gpu.availability)?;
                let entry = best.entry(gpu.id).or_insert(level);
                *entry = (*entry).min(level);
            }
        }
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
                    availability: best.get(&gpu.id).copied().unwrap_or(Availability::None),
                    id: gpu.id,
                    name: gpu.name,
                    memory_gb: gpu.memory,
                    hourly: gpu.price.secure,
                })
                .collect(),
            storage_gb_month: STORAGE_GB_MONTH,
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
            "/datacenters?include=CPU_AVAILABILITY&networkVolumeTypes=STANDARD",
            cancel,
        )?;
        let centers: Vec<String> = candidates(catalog, data_centers, &requested)
            .into_iter()
            .map(|(_, id)| id)
            .collect();
        let flavors: Vec<&Flavor> = requested.iter().filter_map(|id| Flavor::get(id)).collect();
        let stock = self.cpu_stock(&centers, &flavors, profile.cpu, cancel)?;
        Ok(SizeAvailability {
            best: stock.values().min().map_or(Availability::None, |&level| level.into()),
            centers: stock.len(),
        })
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
