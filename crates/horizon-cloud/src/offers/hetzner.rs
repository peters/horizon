//! Hetzner offers, ranked from its catalog. Hetzner bills compute per started hour up
//! to a monthly cap, in euros net of VAT, and its availability flag is advisory, so it
//! is reported but never used to hide an offer.
use super::{DEFAULT_LIMIT, DEFAULT_STORAGE_GB, MAX_LIMIT, MONTH_HOURS, Offer, Requirements, normalize};
use crate::hetzner::catalog::{self, Catalog};
use time::{Date, Month, OffsetDateTime};

/// Whether Horizon can create clouds on Hetzner yet. The deployment wiring for #972
/// turns this on; until then offers are informational only.
pub const DEPLOYABLE: bool = false;
const EUR: &str = "EUR";

/// Hetzner offers in `catalog` meeting `requirements`, cheapest estimated total first,
/// each priced for a run that starts now. Hetzner has no hourly GPUs, so a GPU request
/// gets none. `max_hourly` is read in euros, the currency these offers are in.
#[must_use]
pub fn hetzner(catalog: &Catalog, requirements: &Requirements) -> Vec<Offer> {
    hetzner_at(catalog, requirements, OffsetDateTime::now_utc())
}

/// As [`hetzner`], for a run that starts at `start`. Hetzner caps each price per
/// calendar month (UTC), so the estimate follows the month boundaries after `start`.
#[must_use]
pub fn hetzner_at(catalog: &Catalog, requirements: &Requirements, start: OffsetDateTime) -> Vec<Offer> {
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
        .filter_map(|offer| priced(offer, catalog, requirements, (hours, volume_month, start)))
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
    (hours, volume_month, start): (f64, f64, OffsetDateTime),
) -> Option<Offer> {
    let vcpu = u16::try_from(offer.cores).ok()?;
    let memory_gb = whole_gb(offer.memory_gb)?;
    if vcpu < requirements.min_vcpu.unwrap_or(0) || memory_gb < requirements.min_memory_gb.unwrap_or(0) {
        return None;
    }
    // Every server has a primary IPv4 address; an offer that cannot price it is left out
    // rather than shown as cheaper than it is.
    let ipv4_month = catalog.ipv4_month_eur.get(&offer.location).copied()?;
    let ipv4_hour = catalog.ipv4_hour_eur.get(&offer.location).copied()?;
    // The volume and address are billed per started hour too, up to their monthly price.
    let running_extras =
        compute(volume_month / MONTH_HOURS, volume_month, hours, start) + compute(ipv4_hour, ipv4_month, hours, start);
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
        estimated_total: compute(offer.hourly_eur, offer.monthly_eur, hours, start) + running_extras,
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

/// An amount billed per started hour, and at most `monthly` in each calendar month
/// (UTC), for a run of `hours` from `start`. Each started hour counts in the month it
/// starts in, and a created server bills at least one hour.
fn compute(hourly: f64, monthly: f64, hours: f64, start: OffsetDateTime) -> f64 {
    let mut remaining = hours.ceil().max(1.0);
    let mut at = start;
    let mut total = 0.0;
    // A run of at most a year touches at most fourteen calendar months.
    for _ in 0..16 {
        if remaining <= 0.0 {
            return total;
        }
        let Some(end) = next_month(at) else { break };
        let starts_this_month = ((end - at).as_seconds_f64() / 3600.0).ceil().max(1.0);
        let hours_here = remaining.min(starts_this_month);
        total += (hours_here * hourly).min(monthly);
        remaining -= hours_here;
        at += time::Duration::seconds_f64(hours_here * 3600.0);
    }
    // Past the supported range, bill the rest hourly: never understated.
    total + remaining.max(0.0) * hourly
}

/// The first instant of the calendar month after the one `at` is in.
fn next_month(at: OffsetDateTime) -> Option<OffsetDateTime> {
    let date = at.date();
    let (year, month) = match date.month() {
        Month::December => (date.year().checked_add(1)?, Month::January),
        month => (date.year(), month.next()),
    };
    Some(Date::from_calendar_date(year, month, 1).ok()?.midnight().assume_utc())
}

/// Whole gigabytes of memory, rounded down.
fn whole_gb(gb: f64) -> Option<u16> {
    format!("{:.0}", gb.floor()).parse().ok()
}

#[cfg(test)]
mod tests;
