//! Read-only, exact-ID provider observations; never volume ownership or attachment authority.

use super::{RunPodClient, RunPodError, valid_provider_id};
use serde::Deserialize;

const MIN_VOLUME_GB: u32 = 10;
const MAX_VOLUME_GB: u32 = 4_096;

/// Caller-selected constraints for one provider network-volume observation.
/// These values do not establish ownership, exclusive attachment or a durable binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunPodNetworkVolumeExpectation {
    pub volume_id: String,
    pub data_center_id: String,
    /// Provider-reported allocated GB, not measured free space.
    pub minimum_size_gb: u32,
}

/// Point-in-time metadata for a matching High-Performance network volume.
///
/// This is not proof of ownership, exclusivity, mount permissions, filesystem
/// durability or available capacity. It does not authorize attach, Stop or Delete.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunPodNetworkVolume {
    pub volume_id: String,
    pub data_center_id: String,
    pub size_gb: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ApiNetworkVolume {
    id: String,
    data_center: String,
    size: u32,
    #[serde(rename = "type")]
    storage_type: String,
}

impl RunPodClient {
    /// Observe one exact network volume without listing, creating or modifying resources.
    ///
    /// `None` means the provider returned 404. A present response must match the
    /// requested ID, data center and minimum size, with tier `HIGH_PERFORMANCE`.
    /// A successful observation is not a persisted storage-identity binding.
    ///
    /// # Errors
    /// Rejects invalid expectations before I/O, mismatched metadata, malformed or
    /// oversized responses, transport failures and non-404 unsuccessful statuses.
    pub fn inspect_high_performance_volume(
        &self,
        expected: &RunPodNetworkVolumeExpectation,
    ) -> Result<Option<RunPodNetworkVolume>, RunPodError> {
        if !valid_provider_id(&expected.volume_id)
            || !valid_provider_id(&expected.data_center_id)
            || !(MIN_VOLUME_GB..=MAX_VOLUME_GB).contains(&expected.minimum_size_gb)
        {
            return Err(RunPodError::InvalidTarget);
        }
        let Some(volume) = self.transport.network_volume(&expected.volume_id)? else {
            return Ok(None);
        };
        if volume.id != expected.volume_id
            || volume.data_center != expected.data_center_id
            || volume.storage_type != "HIGH_PERFORMANCE"
            || !(expected.minimum_size_gb..=MAX_VOLUME_GB).contains(&volume.size)
        {
            return Err(RunPodError::ResourceIdentityMismatch);
        }
        Ok(Some(RunPodNetworkVolume {
            volume_id: volume.id,
            data_center_id: volume.data_center,
            size_gb: volume.size,
        }))
    }
}

#[cfg(test)]
mod tests;
