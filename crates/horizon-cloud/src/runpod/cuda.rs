//! CUDA host constraints for GPU workers. `POST /pods` in the v1 REST API (0.1.0) takes
//! only an exact `allowedCudaVersions` list, so a profile's `min_cuda_version` floor is
//! expanded from the v2 GPU catalog (2.0.0), which reports the CUDA versions that hosts
//! of each GPU type run. The v2 pod create takes the floor itself as `gpu.minCudaVersion`.
use super::{RunPod, volumes::Capacity};
use crate::{Cancellation, CloudError, WorkerSpec, profile::cuda_version};
use serde::Deserialize;

/// The `allowedCudaVersions` values the v1 pod create accepts, as its schema lists them
/// (checked 2026-09-26). Newer versions the catalog reports are left out rather than
/// risking a refused request; they only widen what a floor already allows.
const POD_CREATE_VERSIONS: [&str; 12] = [
    "13.0", "12.9", "12.8", "12.7", "12.6", "12.5", "12.4", "12.3", "12.2", "12.1", "12.0", "11.8",
];

impl RunPod {
    /// The CUDA versions at or above the profile's floor that hosts of the requested GPU
    /// types run with free capacity now, in the worker's data centers when it names any,
    /// newest first. Empty when the profile sets no floor.
    /// # Errors
    /// Reports a floor no requested GPU type meets, provider errors and malformed catalogs.
    pub(super) fn allowed_cuda_versions(
        &self,
        spec: &WorkerSpec,
        cancel: &Cancellation,
    ) -> Result<Vec<String>, CloudError> {
        let Some(floor) = spec.profile.min_cuda_version.as_deref().filter(|_| spec.profile.gpu) else {
            return Ok(Vec::new());
        };
        let minimum = cuda_version(floor).ok_or(CloudError::Invalid("Invalid worker profile"))?;
        let catalog = self.cuda_catalog(&format!("minCudaVersion={floor}"), cancel)?;
        let mut versions = Vec::new();
        for gpu in catalog.requested(spec) {
            for offered in &gpu.cuda_versions {
                let number = cuda_version(&offered.version).ok_or(CloudError::InvalidResponse)?;
                // A version whose hosts are full would only fail the create on capacity.
                if offered.available && number >= minimum && POD_CREATE_VERSIONS.contains(&offered.version.as_str()) {
                    versions.push((number, offered.version.clone()));
                }
            }
        }
        versions.sort_unstable_by(|a, b| b.cmp(a));
        versions.dedup();
        let mut allowed = Vec::new();
        for (_, version) in versions {
            if spec.data_centers.is_empty() || self.free_in_data_centers(spec, &version, cancel)? {
                allowed.push(version);
            }
        }
        if allowed.is_empty() {
            return Err(CloudError::CudaUnavailable(floor.to_owned()));
        }
        Ok(allowed)
    }

    /// Whether a requested GPU type has free capacity on `version` in one of the worker's
    /// data centers. The floor query's version flags span every data center, while an
    /// exact `cudaVersions` filter scopes each type's per-data-center availability to it.
    fn free_in_data_centers(
        &self,
        spec: &WorkerSpec,
        version: &str,
        cancel: &Cancellation,
    ) -> Result<bool, CloudError> {
        let catalog = self.cuda_catalog(&format!("cudaVersions={version}"), cancel)?;
        Ok(catalog.requested(spec).any(|gpu| {
            gpu.cuda_versions
                .iter()
                .any(|offered| offered.available && offered.version == version)
                && gpu.data_centers.iter().any(|center| {
                    spec.data_centers.contains(&center.id)
                        && matches!(center.availability.as_str(), "HIGH" | "MEDIUM" | "LOW")
                })
        }))
    }

    /// GPU types with the CUDA versions their Secure Cloud pod hosts run, scoped by `filter`.
    fn cuda_catalog(&self, filter: &str, cancel: &Cancellation) -> Result<Catalog, CloudError> {
        let url = format!(
            "{}/gpus?include=AVAILABILITY&product=POD&cloud=SECURE&{filter}",
            self.catalog_endpoint
        );
        serde_json::from_value(self.request_url("GET", &url, None, cancel, None)?)
            .map_err(|_| CloudError::InvalidResponse)
    }
}

#[derive(Deserialize)]
struct Catalog {
    gpus: Vec<Gpu>,
}
impl Catalog {
    fn requested<'a>(&'a self, spec: &'a WorkerSpec) -> impl Iterator<Item = &'a Gpu> {
        self.gpus.iter().filter(|gpu| spec.gpu_types.contains(&gpu.id))
    }
}
/// A missing `cudaVersions` means none: the catalog omits it for a GPU type whose hosts
/// report no CUDA version. A missing `dataCenters` means the type is unavailable everywhere.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Gpu {
    id: String,
    #[serde(default)]
    cuda_versions: Vec<CudaVersion>,
    #[serde(default)]
    data_centers: Vec<Capacity>,
}
#[derive(Deserialize)]
struct CudaVersion {
    version: String,
    available: bool,
}
