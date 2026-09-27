//! Prices for choosing a worker. [`price_list`] asks `RunPod`, and [`hetzner_catalog`]
//! asks Hetzner when this machine has a Hetzner binding.
use super::{Cancellation, Result, settings::Settings};
pub use horizon_cloud::hetzner::catalog::Catalog as HetznerCatalog;
pub use horizon_cloud::runpod::prices::STORAGE as RUNPOD_STORAGE;
pub use horizon_cloud::{
    Profile,
    prices::{
        Availability, CpuFlavorPrice, DataCenter, GpuPrice, Preferences, PriceList, SizeAvailability, StoragePrices,
    },
};
use horizon_cloud::{hetzner::Hetzner, runpod::RunPod};

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
    catalog
        .offers
        .retain(|offer| hetzner.locations.contains(&offer.location));
    Ok(Some(catalog))
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
}
