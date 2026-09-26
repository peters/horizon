//! Prices the owning Horizon hands to its workers, so agents there can rank cloud offers
//! without the provider account. A snapshot informs estimates; it never reserves compute
//! or grants access.
use horizon_cloud::prices::{Preferences, PriceList};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
/// The largest encoded snapshot a worker accepts.
pub const MAX_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub version: u32,
    /// When the owning Horizon fetched the prices, in milliseconds since the Unix epoch.
    pub observed_at_millis: u64,
    pub list: PriceList,
    /// The CPU flavors and GPU types a deployment requests, so offers match them.
    pub preferences: Preferences,
}

impl Snapshot {
    /// # Errors
    /// Rejects another version and lists far larger than any provider publishes.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != VERSION {
            return Err("Unsupported cloud offer snapshot");
        }
        let list = &self.list;
        if list.cpu.len() > 64
            || list.gpus.len() > 256
            || list.data_centers.len() > 256
            || list.regions.len() > 256
            || self.preferences.cpu_flavors.len() > 64
            || self.preferences.gpu_types.len() > 256
        {
            return Err("Cloud offer snapshot is too large");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_carry_one_version_and_bounded_lists() {
        let snapshot = Snapshot {
            version: VERSION,
            observed_at_millis: 1,
            list: PriceList {
                provider: "RunPod",
                cpu: Vec::new(),
                gpus: Vec::new(),
                data_centers: Vec::new(),
                regions: std::collections::BTreeMap::new(),
                storage: horizon_cloud::runpod::prices::STORAGE,
            },
            preferences: Preferences::default(),
        };
        assert!(snapshot.validate().is_ok());
        let encoded = serde_json::to_vec(&snapshot).unwrap();
        let decoded: Snapshot = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.list.provider, "RunPod");
        assert!(
            Snapshot {
                version: VERSION + 1,
                ..snapshot.clone()
            }
            .validate()
            .is_err()
        );
        let mut crowded = snapshot;
        crowded.preferences.cpu_flavors = vec!["cpu3c".into(); 65];
        assert!(crowded.validate().is_err());
    }
}
