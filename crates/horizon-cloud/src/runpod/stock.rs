//! Exact-size CPU availability from the v2 catalog, scoped to pod deployments.
use super::{
    RunPod,
    flavors::{Flavor, VCPU_COUNTS},
};
use crate::{Cancellation, CloudError, valid_id};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

/// Most available first, so the derived order ranks placements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub(super) enum Level {
    High,
    Medium,
    Low,
}

impl RunPod {
    /// Best stock level per data center across `flavors` at `cpu` vCPUs.
    /// Family-level availability never substitutes for the exact-size query.
    pub(super) fn cpu_stock(
        &self,
        centers: &[String],
        flavors: &[&Flavor],
        cpu: u16,
        cancel: &Cancellation,
    ) -> Result<HashMap<String, Level>, CloudError> {
        if centers.is_empty() || flavors.is_empty() {
            return Ok(HashMap::new());
        }
        if !VCPU_COUNTS.contains(&cpu) || centers.iter().any(|id| !valid_id(id)) {
            return Err(CloudError::Invalid("Invalid CPU capacity query"));
        }
        let url = format!(
            "{}/cpus?include=AVAILABILITY&product=POD&vcpuCount={cpu}",
            self.catalog_endpoint
        );
        let response: Response = serde_json::from_value(self.request_url("GET", &url, None, cancel, None)?)
            .map_err(|_| CloudError::InvalidResponse)?;
        let mut stock: HashMap<String, Level> = HashMap::new();
        let mut seen = HashSet::new();
        for entry in response.cpus {
            if !seen.insert(entry.id.clone()) {
                return Err(CloudError::InvalidResponse);
            }
            let Some(flavor) = flavors.iter().find(|flavor| flavor.id == entry.id) else {
                continue;
            };
            // Do not certify a profile against a catalog whose sizing differs
            // from the policy used to construct that profile.
            if (entry.ram_gb_per_vcpu - f64::from(flavor.memory_per_vcpu)).abs() > f64::EPSILON
                || !(entry.vcpu.min..=entry.vcpu.max).contains(&cpu)
            {
                return Err(CloudError::InvalidResponse);
            }
            let mut seen_centers = HashSet::new();
            for center in entry.data_centers {
                if !valid_id(&center.id) || !seen_centers.insert(center.id.clone()) {
                    return Err(CloudError::InvalidResponse);
                }
                if centers.contains(&center.id)
                    && let Availability::Available(level) = center.availability
                {
                    let best = stock.entry(center.id).or_insert(level);
                    *best = (*best).min(level);
                }
            }
        }
        if flavors.iter().any(|flavor| !seen.contains(flavor.id)) {
            return Err(CloudError::InvalidResponse);
        }
        Ok(stock)
    }
}

#[derive(Deserialize)]
struct Response {
    cpus: Vec<Entry>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    id: String,
    vcpu: Bounds,
    ram_gb_per_vcpu: f64,
    // Omitted by the API when the flavor is unavailable everywhere.
    #[serde(default)]
    data_centers: Vec<Center>,
}
#[derive(Deserialize)]
struct Bounds {
    min: u16,
    max: u16,
}
#[derive(Deserialize)]
struct Center {
    id: String,
    availability: Availability,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Availability {
    Available(Level),
    Unavailable(Unavailable),
}
#[derive(Deserialize)]
enum Unavailable {
    #[serde(rename = "NONE")]
    None,
}
