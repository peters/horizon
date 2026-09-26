//! Prices for choosing a worker in New cloud. Only providers Horizon can deploy to
//! are asked; today that is `RunPod`.
use super::{Cancellation, Result, settings::Settings};
use horizon_cloud::runpod::RunPod;
pub use horizon_cloud::runpod::prices::STORAGE as RUNPOD_STORAGE;
pub use horizon_cloud::{
    Profile,
    prices::{Availability, CpuFlavorPrice, GpuPrice, PriceList, SizeAvailability, StoragePrices},
};

/// The preferences a deployment would use, so prices match what would be requested.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preferences {
    pub cpu_flavors: Vec<String>,
    pub gpu_types: Vec<String>,
}

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
