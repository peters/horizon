//! Prices for choosing a worker. [`price_list`] asks `RunPod`, and [`hetzner_catalog`]
//! asks Hetzner when this machine has a Hetzner binding.
use super::{Cancellation, Result, settings::Settings};
pub use horizon_cloud::hetzner::catalog::Catalog as HetznerCatalog;
pub use horizon_cloud::runpod::prices::STORAGE as RUNPOD_STORAGE;
pub use horizon_cloud::{
    Profile, Storage,
    prices::{
        Availability, CpuFlavorPrice, DataCenter, GpuPrice, Preferences, PriceList, SizeAvailability, StoragePrices,
    },
};
use horizon_cloud::{hetzner::Hetzner, runpod::RunPod};

pub use horizon_cloud::runpod::volumes::Tier as StorageTier;

pub mod freshness;
pub mod watch;

/// Current prices and the preferences they apply to.
/// # Errors
/// Fails without a provider credential and on provider errors.
pub fn price_list(settings: &Settings, cancel: &Cancellation) -> Result<(PriceList, Preferences)> {
    let list = RunPod::new(settings.credential()?).price_list(&settings.data_centers, cancel)?;
    Ok((
        list,
        Preferences {
            cpu_flavors: settings.cpu_flavors.clone(),
            gpu_types: settings.gpu_types.clone(),
        },
    ))
}

/// Server types and locations present in a Hetzner catalog and absent from this
/// machine's settings. The catalog that follows lists only the allowed pairs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HetznerExclusions {
    pub server_types: usize,
    pub locations: usize,
}

/// A Hetzner catalog narrowed to this machine's settings, with what those settings left out.
pub struct HetznerCatalogReport {
    pub catalog: HetznerCatalog,
    pub exclusions: HetznerExclusions,
}

/// Hetzner's catalog for the locations this machine allows, or `None` without a Hetzner
/// binding. Every server type stays listed: Hetzner's availability flag is advisory.
/// # Errors
/// Fails without a readable Hetzner token and on provider errors.
pub fn hetzner_catalog(settings: &Settings, cancel: &Cancellation) -> Result<Option<HetznerCatalog>> {
    Ok(hetzner_catalog_report(settings, cancel)?.map(|report| report.catalog))
}

/// As [`hetzner_catalog`], also counting the server types and locations the settings leave out.
/// # Errors
/// Fails without a readable Hetzner token and on provider errors.
pub fn hetzner_catalog_report(settings: &Settings, cancel: &Cancellation) -> Result<Option<HetznerCatalogReport>> {
    let Some(hetzner) = &settings.hetzner else {
        return Ok(None);
    };
    let mut catalog = Hetzner::new(hetzner.credential()?).catalog(cancel)?;
    let exclusions = retain_allowed(&mut catalog, &hetzner.server_types, &hetzner.locations);
    Ok(Some(HetznerCatalogReport { catalog, exclusions }))
}

/// Drops offers the settings do not allow and counts the distinct types and locations removed.
fn retain_allowed(catalog: &mut HetznerCatalog, server_types: &[String], locations: &[String]) -> HetznerExclusions {
    let type_allowed = |name: &str| server_types.iter().any(|item| item == name);
    let location_allowed = |name: &str| locations.iter().any(|item| item == name);
    let mut excluded_types = std::collections::BTreeSet::new();
    let mut excluded_locations = std::collections::BTreeSet::new();
    for offer in &catalog.offers {
        if !type_allowed(&offer.server_type) {
            excluded_types.insert(offer.server_type.clone());
        }
        if !location_allowed(&offer.location) {
            excluded_locations.insert(offer.location.clone());
        }
    }
    catalog
        .offers
        .retain(|offer| type_allowed(&offer.server_type) && location_allowed(&offer.location));
    HetznerExclusions {
        server_types: excluded_types.len(),
        locations: excluded_locations.len(),
    }
}

/// Live stock of the CPU size `profile` asks for.
/// # Errors
/// Fails without a provider credential, for sizes no flavor offers and on provider errors.
pub fn size_availability(settings: &Settings, profile: &Profile, cancel: &Cancellation) -> Result<SizeAvailability> {
    Ok(RunPod::new(settings.credential()?).cpu_size_availability(
        profile,
        &settings.cpu_flavors,
        &settings.data_centers,
        cancel,
    )?)
}

/// The CPU flavors a deployment of `profile` would request, empty when none offers it.
#[must_use]
pub fn requested_flavors(profile: &Profile, preferences: &Preferences) -> Vec<String> {
    horizon_cloud::runpod::flavors::for_profile(profile, &preferences.cpu_flavors).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hetzner_is_asked_only_with_a_binding_and_a_readable_token() {
        let root = tempfile::tempdir().unwrap();
        let absolute = |name: &str| root.path().join(name);
        let base = serde_json::json!({
            "runpod_key_file": absolute("key"), "ssh_identity_file": absolute("identity"),
            "docker_config": absolute("docker"), "registry_pull_auth_id": null,
            "cpu_flavors": ["cpu3c"], "gpu_types": [],
        });
        let cancel = Cancellation::default();
        let without: Settings = serde_json::from_value(base.clone()).unwrap();
        assert!(hetzner_catalog(&without, &cancel).unwrap().is_none());
        // A binding whose token file is missing fails before any provider request.
        let mut with = base;
        with["hetzner"] = serde_json::json!({
            "token_file": absolute("missing-token"), "server_types": ["cx43"], "locations": ["hel1"],
        });
        let with: Settings = serde_json::from_value(with).unwrap();
        assert!(hetzner_catalog(&with, &cancel).is_err());
    }

    #[test]
    fn settings_leave_out_distinct_server_types_and_locations() {
        let mut catalog = HetznerCatalog {
            offers: vec![
                offer("cx23", "hel1"),
                offer("cx23", "fsn1"),
                offer("cx33", "hel1"),
                offer("cpx42", "nbg1"),
            ],
            volume_gb_month_eur: 0.05,
            ipv4_month_eur: std::collections::BTreeMap::new(),
            ipv4_hour_eur: std::collections::BTreeMap::new(),
            regions: std::collections::BTreeMap::new(),
        };
        let exclusions = retain_allowed(&mut catalog, &["cx33".into(), "cx23".into()], &["hel1".into()]);
        assert_eq!(
            exclusions,
            HetznerExclusions {
                server_types: 1,
                locations: 2
            }
        );
        assert_eq!(
            catalog
                .offers
                .iter()
                .map(|offer| (offer.server_type.as_str(), offer.location.as_str()))
                .collect::<Vec<_>>(),
            [("cx23", "hel1"), ("cx33", "hel1")]
        );
    }

    fn offer(server_type: &str, location: &str) -> horizon_cloud::hetzner::catalog::Offer {
        horizon_cloud::hetzner::catalog::Offer {
            server_type: server_type.into(),
            location: location.into(),
            cores: 2,
            memory_gb: 4.0,
            disk_gb: 40,
            dedicated: false,
            hourly_eur: 0.01,
            monthly_eur: 4.0,
            available: true,
            recommended: false,
        }
    }
}
