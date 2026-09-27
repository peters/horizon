//! Prices the owning Horizon hands to its workers, so agents there can rank cloud offers
//! without the provider account. A snapshot informs estimates; it never reserves compute
//! or grants access.
use horizon_cloud::{
    hetzner::catalog::Catalog,
    prices::{Preferences, PriceList},
};
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

/// Hetzner's catalog, sent as its own snapshot beside the price list so workers that do
/// not know Hetzner keep taking the price list unchanged.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HetznerSnapshot {
    pub version: u32,
    /// When the owning Horizon fetched the catalog, in milliseconds since the Unix epoch.
    pub observed_at_millis: u64,
    pub catalog: Catalog,
}

impl HetznerSnapshot {
    /// # Errors
    /// Rejects another version and catalogs far larger than Hetzner publishes.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != VERSION {
            return Err("Unsupported Hetzner offer snapshot");
        }
        let catalog = &self.catalog;
        if catalog.offers.len() > 1024
            || catalog.regions.len() > 64
            || catalog.ipv4_month_eur.len() > 64
            || catalog.ipv4_hour_eur.len() > 64
        {
            return Err("Hetzner offer snapshot is too large");
        }
        let amount = |value: f64| value.is_finite() && value >= 0.0;
        let offers_valid = catalog
            .offers
            .iter()
            .all(|offer| amount(offer.hourly_eur) && amount(offer.monthly_eur) && amount(offer.memory_gb));
        let rates_valid = amount(catalog.volume_gb_month_eur)
            && catalog
                .ipv4_month_eur
                .values()
                .chain(catalog.ipv4_hour_eur.values())
                .all(|value| amount(*value));
        if !offers_valid || !rates_valid {
            return Err("Hetzner offer snapshot has an invalid amount");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hetzner_snapshots_carry_one_version_and_bounded_catalogs() {
        let snapshot = HetznerSnapshot {
            version: VERSION,
            observed_at_millis: 1,
            catalog: Catalog {
                offers: Vec::new(),
                volume_gb_month_eur: 0.0572,
                ipv4_month_eur: std::collections::BTreeMap::new(),
                ipv4_hour_eur: std::collections::BTreeMap::new(),
                regions: std::collections::BTreeMap::from([("hel1".into(), "EUROPE".into())]),
            },
        };
        assert!(snapshot.validate().is_ok());
        let decoded: HetznerSnapshot = serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
        assert_eq!(decoded, snapshot);
        assert!(
            HetznerSnapshot {
                version: VERSION + 1,
                ..snapshot.clone()
            }
            .validate()
            .is_err()
        );
        let mut negative = snapshot.clone();
        negative.catalog.volume_gb_month_eur = -0.01;
        assert!(negative.validate().is_err());
        let mut unpriced = snapshot.clone();
        unpriced.catalog.ipv4_hour_eur.insert("hel1".into(), f64::NAN);
        assert!(unpriced.validate().is_err());
        let mut crowded = snapshot;
        crowded.catalog.regions = (0..65)
            .map(|index| (format!("l{index}"), "EUROPE".to_owned()))
            .collect();
        assert!(crowded.validate().is_err());
    }

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
