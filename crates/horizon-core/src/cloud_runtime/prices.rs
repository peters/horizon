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

/// Hetzner's catalog for the locations this machine allows, or `None` without a Hetzner
/// binding. Every server type stays listed: Hetzner's availability flag is advisory.
/// # Errors
/// Fails without a readable Hetzner token and on provider errors.
pub fn hetzner_catalog(settings: &Settings, cancel: &Cancellation) -> Result<Option<HetznerCatalog>> {
    let Some(hetzner) = &settings.hetzner else {
        return Ok(None);
    };
    let mut catalog = Hetzner::new(hetzner.credential()?).catalog(cancel)?;
    restrict_locations(&mut catalog, &hetzner.locations);
    Ok(Some(catalog))
}

fn restrict_locations(catalog: &mut HetznerCatalog, locations: &[String]) {
    catalog.offers.retain(|offer| locations.contains(&offer.location));
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
    fn a_larger_catalog_worker_is_selectable_outside_legacy_fallback_preferences() {
        let binding: super::super::settings::Hetzner = serde_json::from_value(serde_json::json!({
            "token_file": "/unused", "server_types": ["cx43", "cx33", "cpx42"], "locations": ["hel1"]
        }))
        .unwrap();
        let worker = |server_type: &str, location: &str, cores, memory_gb, hourly| {
            serde_json::json!({"server_type": server_type, "location": location, "cores": cores,
                "memory_gb": memory_gb, "disk_gb": 160, "dedicated": false, "hourly_eur": hourly,
                "monthly_eur": hourly * 600.0, "available": true, "recommended": false})
        };
        let mut catalog: HetznerCatalog = serde_json::from_value(serde_json::json!({
            "offers": [worker("cx43", "hel1", 8, 16.0, 0.02), worker("cx53", "hel1", 16, 32.0, 0.04),
                worker("cx53", "ash", 16, 32.0, 0.03)],
            "volume_gb_month_eur": 0.0572, "ipv4_month_eur": {"hel1": 0.5, "ash": 0.5},
            "ipv4_hour_eur": {"hel1": 0.0008, "ash": 0.0008}, "regions": {"hel1": "EUROPE", "ash": "NORTH_AMERICA"}
        }))
        .unwrap();
        restrict_locations(&mut catalog, &binding.locations);
        let profile: Profile = serde_json::from_value(serde_json::json!({
            "provider": "runpod", "image": "example.invalid/worker", "min_cpu": 8, "min_memory_gb": 32
        }))
        .unwrap();
        let workers = horizon_cloud::offers::workers(&profile, 1.0, None, Some(&catalog));
        assert_eq!(workers.matching, 1, "the larger type meets repository minimums");
        let chosen = crate::cloud_panel::WorkerChoice::from(&workers.offers[0])
            .for_profile(&profile)
            .unwrap();
        assert_eq!(chosen.placement.cpu_types, ["cx53"]);
        assert_eq!(chosen.placement.data_centers, ["hel1"]);
        assert_eq!(binding.types_for(Some(&chosen.placement)).unwrap(), ["cx53"]);
        assert_eq!(binding.types_for(None).unwrap(), ["cx43", "cx33", "cpx42"]);
        assert!(
            workers
                .offers
                .iter()
                .all(|offer| offer.location.as_deref() == Some("hel1"))
        );
        assert_eq!(
            workers.offers[workers.matching].id, "cx43",
            "small types stay below requirements"
        );
    }

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
}
