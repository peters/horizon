//! The provider a new cloud runs on, read from each provider's description, and the
//! offers of providers that place workers in named locations. A provider choice is shown
//! only when more than one configured provider supports the profile; nothing moves a
//! cloud between providers on its own.
use super::super::{Production, machine_size::Size, prices::State};
use horizon_core::cloud_runtime::{
    flavors,
    prices::Profile,
    provider::{self, Description},
};

/// The providers this machine can use: `RunPod` unless a fetch found no API key for it,
/// and each other provider once a fetch finds its binding, including while its catalog
/// is refreshed or could not be fetched.
fn configured(prices: &State) -> Vec<&'static Description> {
    let mut configured = Vec::new();
    if prices.runpod_bound() {
        configured.push(&provider::RUNPOD);
    }
    if prices.hetzner.bound() {
        configured.push(&provider::HETZNER);
    }
    configured
}

/// The providers a new cloud of `profile` can choose from. A choice is shown only when
/// there is more than one.
pub(super) fn choices(prices: &State, profile: &Profile) -> Vec<&'static Description> {
    Description::choices(profile, &configured(prices))
}

/// The provider a new cloud of `profile` uses: the one chosen in the dialog, or else the
/// profile's own.
pub(in crate::app::cloud_panel) fn current(
    chosen: Option<&'static Description>,
    profile: &Profile,
) -> &'static Description {
    chosen
        .or_else(|| provider::by_id(&profile.provider))
        .unwrap_or(&provider::RUNPOD)
}

/// `profile` on `provider` at `size`, or at its own size, by that provider's rules:
/// providers that price flavors need a flavor with the size, and others only the
/// profile's own rules, since server types are checked when the cloud is placed.
pub(in crate::app::cloud_panel) fn sized(
    provider: &Description,
    profile: &Profile,
    size: Option<Size>,
) -> horizon_core::cloud_runtime::Result<Profile> {
    let profile = Profile {
        provider: provider.id.to_owned(),
        ..profile.clone()
    };
    if provider.pricing == provider::Pricing::Flavors {
        return Ok(match size {
            Some(size) => flavors::sized(&profile, size)?,
            // A GPU profile specifies minimum host resources; CPU sizes must be offered.
            None if profile.gpu => profile,
            None => flavors::sized(&profile, (profile.cpu, profile.memory_gb))?,
        });
    }
    let (cpu, memory_gb) = size.unwrap_or((profile.cpu, profile.memory_gb));
    let sized = Profile {
        cpu,
        memory_gb,
        ..profile
    };
    sized
        .validate(false)
        .map_err(|_| horizon_core::cloud_runtime::Error::Invalid("This profile cannot run on the chosen provider"))?;
    Ok(sized)
}

/// The provider's name for the dialog heading.
pub(super) fn label(form: &Production) -> &'static str {
    form.profiles
        .as_ref()
        .and_then(|config| config.profiles.get(&form.selected_profile))
        .map_or(provider::RUNPOD.label, |profile| current(form.provider, profile).label)
}

/// Euros with cents, or four decimals below a cent.
pub(super) fn euros(value: f64) -> String {
    if value < 0.1 {
        format!("€{value:.4}")
    } else {
        format!("€{value:.2}")
    }
}
