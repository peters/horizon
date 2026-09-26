//! Hetzner offers, ranked from its catalog. Hetzner bills compute per started hour up
//! to a monthly cap, in euros net of VAT, and its availability flag is advisory, so it
//! is reported but never used to hide an offer.
use super::{DEFAULT_LIMIT, DEFAULT_STORAGE_GB, MAX_LIMIT, MONTH_HOURS, Offer, Requirements, normalize};
use crate::hetzner::catalog::{self, Catalog};

/// Whether Horizon can create clouds on Hetzner yet. The deployment wiring for #972
/// turns this on; until then offers are informational only.
pub const DEPLOYABLE: bool = false;
const EUR: &str = "EUR";

/// Hetzner offers in `catalog` meeting `requirements`, cheapest estimated total first.
/// Hetzner has no hourly GPUs, so a GPU request gets none. `max_hourly` is read in
/// euros, the currency these offers are in.
#[must_use]
pub fn hetzner(catalog: &Catalog, requirements: &Requirements) -> Vec<Offer> {
    if requirements.gpu {
        return Vec::new();
    }
    let hours = requirements.hours.unwrap_or(1.0);
    // The workspace volume is kept while the cloud is stopped; the server and its
    // IPv4 address are deleted then.
    let volume_month = catalog.volume_gb_month_eur * f64::from(requirements.storage_gb.unwrap_or(DEFAULT_STORAGE_GB));
    let wanted = requirements.region.as_deref().map(normalize);
    let mut offers: Vec<Offer> = catalog
        .offers
        .iter()
        .filter(|offer| {
            wanted.as_ref().is_none_or(|wanted| {
                *wanted == normalize(&offer.location)
                    || catalog
                        .regions
                        .get(&offer.location)
                        .is_some_and(|region| normalize(region) == *wanted)
            })
        })
        .filter_map(|offer| priced(offer, catalog, requirements, (hours, volume_month)))
        .collect();
    offers.retain(|offer| requirements.max_hourly.is_none_or(|max| offer.hourly <= max));
    offers.sort_by(|a, b| {
        a.estimated_total
            .total_cmp(&b.estimated_total)
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.location.cmp(&b.location))
    });
    offers.truncate(requirements.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT));
    offers
}

fn priced(
    offer: &catalog::Offer,
    catalog: &Catalog,
    requirements: &Requirements,
    (hours, volume_month): (f64, f64),
) -> Option<Offer> {
    let vcpu = u16::try_from(offer.cores).ok()?;
    let memory_gb = whole_gb(offer.memory_gb)?;
    if vcpu < requirements.min_vcpu.unwrap_or(0) || memory_gb < requirements.min_memory_gb.unwrap_or(0) {
        return None;
    }
    let ipv4_month = catalog.ipv4_month_eur.get(&offer.location).copied().unwrap_or(0.0);
    let running_extras = (volume_month + ipv4_month) * hours / MONTH_HOURS;
    let cpu = if offer.dedicated { "dedicated" } else { "shared" };
    Some(Offer {
        provider: "Hetzner",
        currency: EUR,
        kind: "cpu",
        id: offer.server_type.clone(),
        name: format!("{} · {vcpu} vCPU · {memory_gb} GB · {cpu}", offer.server_type),
        vcpu: Some(vcpu),
        memory_gb: Some(memory_gb),
        gpu_memory_gb: None,
        hourly: offer.hourly_eur,
        flavors: Vec::new(),
        estimated_total: compute(offer.hourly_eur, offer.monthly_eur, hours) + running_extras,
        monthly: Some(offer.monthly_eur),
        stopped_monthly: volume_month,
        location: Some(offer.location.clone()),
        availability: if offer.available { "listed" } else { "unlisted" },
        regions_in_stock: Vec::new(),
        host: "provider_operated",
        interruptible: false,
        rentable: DEPLOYABLE,
    })
}

/// Compute billed per started hour, and at most the monthly price for each month.
fn compute(hourly: f64, monthly: f64, hours: f64) -> f64 {
    let months = (hours / MONTH_HOURS).floor();
    let rest = hours - months * MONTH_HOURS;
    months * monthly + (rest.ceil() * hourly).min(monthly)
}

/// Whole gigabytes of memory, rounded down.
fn whole_gb(gb: f64) -> Option<u16> {
    format!("{:.0}", gb.floor()).parse().ok()
}

#[cfg(test)]
mod tests;
