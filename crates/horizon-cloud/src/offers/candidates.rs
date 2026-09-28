//! Where a new cloud of a profile can be created first on each provider this machine is
//! set up for, in one ranked list, so New cloud can rent from whichever provider is
//! cheapest for the chosen size. Read only: nothing here allocates compute.
use super::MONTH_HOURS;
use crate::{
    Profile,
    hetzner::catalog::Catalog,
    prices::{Availability, Preferences, PriceList},
    provider::{self, Description},
    runpod::{flavors, volumes::REQUEST_SIZE_GB},
};

/// US dollars per euro, from a reference rate, so prices billed in either currency
/// can be compared. Amounts are only converted to rank them, never shown converted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExchangeRate {
    pub usd_per_eur: f64,
}

/// What each configured provider currently offers. A provider without a source is not
/// configured on this machine, or its prices have not arrived yet.
#[derive(Clone, Copy, Default)]
pub struct Sources<'a> {
    pub runpod: Option<(&'a PriceList, &'a Preferences)>,
    pub hetzner: Option<HetznerSource<'a>>,
}

/// Hetzner's catalog with the server types and locations this machine's settings try,
/// in order. Empty locations allow every location in the catalog.
#[derive(Clone, Copy)]
pub struct HetznerSource<'a> {
    pub catalog: &'a Catalog,
    pub server_types: &'a [String],
    pub locations: &'a [String],
}

/// The worker a provider is asked for first for a profile.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub provider: &'static Description,
    /// What is requested: a `RunPod` CPU size such as `8 vCPU · 32 GB` or GPU type name,
    /// or a Hetzner server type such as `cx53`.
    pub name: String,
    /// The location tried first, for providers that place workers in named locations.
    pub location: Option<String>,
    /// Compute per hour in `currency`; for a `RunPod` CPU size, the highest price among
    /// the flavors requested, since the provider picks one.
    pub hourly: f64,
    /// Compute plus the storage and address billed while it runs, per hour, which is
    /// what candidates are ranked by.
    pub running_hourly: f64,
    /// The provider's billing currency, as an ISO 4217 code.
    pub currency: &'static str,
}

/// The first worker each configured provider would be asked for to run `profile` at
/// its size, cheapest first. Prices in different currencies are compared at `rate`;
/// without one they cannot be compared, so the profile's own provider comes first and
/// the others follow in the order providers are listed. Providers that cannot run the
/// profile, or whose prices have no worker of this size, are left out.
#[must_use]
pub fn candidates(profile: &Profile, sources: &Sources<'_>, rate: Option<ExchangeRate>) -> Vec<Candidate> {
    let own = Description::of(profile);
    let mut candidates: Vec<Candidate> = provider::ALL
        .into_iter()
        .filter(|provider| provider.creatable && provider.supports(profile))
        .filter_map(|provider| match provider.kind {
            provider::Kind::RunPod => runpod(profile, sources.runpod?),
            provider::Kind::Hetzner => hetzner(profile, sources.hetzner?),
        })
        .collect();
    let own_first = |candidate: &Candidate| candidate.provider != own;
    match rate {
        Some(rate) => candidates.sort_by(|a, b| {
            in_usd(a, rate)
                .total_cmp(&in_usd(b, rate))
                .then_with(|| own_first(a).cmp(&own_first(b)))
        }),
        None => candidates.sort_by_key(own_first),
    }
    candidates
}

fn in_usd(candidate: &Candidate, rate: ExchangeRate) -> f64 {
    if candidate.currency == provider::HETZNER.currency {
        candidate.running_hourly * rate.usd_per_eur
    } else {
        candidate.running_hourly
    }
}

fn runpod(profile: &Profile, (list, preferences): (&PriceList, &Preferences)) -> Option<Candidate> {
    let candidate = |name: String, hourly: f64, storage_month: f64| Candidate {
        provider: &provider::RUNPOD,
        name,
        location: None,
        hourly,
        running_hourly: hourly + storage_month / MONTH_HOURS,
        currency: provider::RUNPOD.currency,
    };
    let volume_gb = u32::from(profile.storage.volume_gb);
    if profile.gpu {
        // The first preferred GPU type in stock anywhere allowed, as creation requests
        // them in preference order, or the cheapest in stock without preferences; its
        // files live on the pod volume.
        let (running, _) = list.storage.pod_volume_month(volume_gb);
        let in_stock = |gpu: &&crate::prices::GpuPrice| list.gpu_availability(&gpu.id, &[]) != Availability::None;
        let gpu = if preferences.gpu_types.is_empty() {
            list.gpus
                .iter()
                .filter(in_stock)
                .min_by(|a, b| a.hourly.total_cmp(&b.hourly))?
        } else {
            preferences
                .gpu_types
                .iter()
                .filter_map(|id| list.gpu(id))
                .find(in_stock)?
        };
        return Some(candidate(gpu.name.clone(), gpu.hourly, running));
    }
    // A CPU worker keeps its workspace on a network volume, so an allowed data center
    // must be able to hold one of this size, as creation requires. The price list
    // records that for the standard tier only; other tiers are checked at creation.
    let hosts_workspace = REQUEST_SIZE_GB.contains(&volume_gb)
        && (!profile.storage.standard_tier() || list.data_centers.iter().any(|center| center.workspace_storage));
    if !hosts_workspace {
        return None;
    }
    let requested = flavors::for_profile(profile, &preferences.cpu_flavors).ok()?;
    let (_, highest) = list.cpu_hourly(&requested, profile.cpu)?;
    Some(candidate(
        format!("{} vCPU · {} GB", profile.cpu, profile.memory_gb),
        highest,
        list.storage.network_month(volume_gb),
    ))
}

/// The server type a cloud placed in any allowed location gets first: the first
/// configured type that fits, in the first allowed location that has one, as creation
/// tries them.
fn hetzner(profile: &Profile, source: HetznerSource<'_>) -> Option<Candidate> {
    let catalog = source.catalog;
    let mut locations: Vec<&str> = if source.locations.is_empty() {
        let mut all: Vec<&str> = catalog.offers.iter().map(|offer| offer.location.as_str()).collect();
        all.sort_unstable();
        all
    } else {
        source.locations.iter().map(String::as_str).collect()
    };
    let mut seen = std::collections::BTreeSet::new();
    locations.retain(|location| seen.insert(*location));
    let volume_month = catalog.volume_gb_month_eur * f64::from(profile.storage.volume_gb);
    locations.into_iter().find_map(|location| {
        let offer = source.server_types.iter().find_map(|server_type| {
            catalog.offers.iter().find(|offer| {
                &offer.server_type == server_type
                    && offer.location == location
                    && offer.cores >= u32::from(profile.cpu)
                    && offer.memory_gb >= f64::from(profile.memory_gb)
                    && offer.disk_gb >= u32::from(profile.storage.container_gb)
            })
        })?;
        // Every server has an IPv4 address; a location that cannot price it is skipped
        // rather than ranked as cheaper than it is.
        let ipv4_hour = catalog.ipv4_hour_eur.get(location)?;
        Some(Candidate {
            provider: &provider::HETZNER,
            name: offer.server_type.clone(),
            location: Some(location.to_owned()),
            hourly: offer.hourly_eur,
            running_hourly: offer.hourly_eur + volume_month / MONTH_HOURS + ipv4_hour,
            currency: provider::HETZNER.currency,
        })
    })
}

#[cfg(test)]
mod tests;
