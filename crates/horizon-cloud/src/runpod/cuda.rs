//! CUDA host constraints for GPU workers. `POST /pods` in the v1 REST API (0.1.0) takes
//! only an exact `allowedCudaVersions` list, so a profile's `min_cuda_version` floor is
//! expanded from the v2 GPU catalog (2.0.0), which reports the CUDA versions that hosts
//! of each GPU type run. The v2 pod create takes the floor itself as `gpu.minCudaVersion`.
use super::RunPod;
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
    /// types run, newest first. Empty when the profile sets no floor.
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
        let catalog = self.cuda_catalog(floor, cancel)?;
        let mut versions = Vec::new();
        for gpu in catalog.gpus.iter().filter(|gpu| spec.gpu_types.contains(&gpu.id)) {
            for offered in &gpu.cuda_versions {
                let number = cuda_version(&offered.version).ok_or(CloudError::InvalidResponse)?;
                if number >= minimum && POD_CREATE_VERSIONS.contains(&offered.version.as_str()) {
                    versions.push((number, offered.version.clone()));
                }
            }
        }
        versions.sort_unstable_by(|a, b| b.cmp(a));
        versions.dedup();
        if versions.is_empty() {
            return Err(CloudError::CudaUnavailable(floor.to_owned()));
        }
        Ok(versions.into_iter().map(|(_, version)| version).collect())
    }

    /// GPU types with the CUDA versions their Secure Cloud pod hosts run, at `floor` or newer.
    fn cuda_catalog(&self, floor: &str, cancel: &Cancellation) -> Result<Catalog, CloudError> {
        let url = format!(
            "{}/gpus?include=AVAILABILITY&product=POD&cloud=SECURE&minCudaVersion={floor}",
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
/// A missing `cudaVersions` means none: the catalog omits it for a GPU type whose hosts
/// report no CUDA version.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Gpu {
    id: String,
    #[serde(default)]
    cuda_versions: Vec<CudaVersion>,
}
#[derive(Deserialize)]
struct CudaVersion {
    version: String,
}
