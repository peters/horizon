//! Azure offers, ranked from its retail price catalog in US dollars. An estimate adds
//! the managed OS disk, the workspace data disk and the Standard static public address
//! to the compute. A deallocated cloud keeps both disks and the address, so they make
//! its stopped cost. Horizon cannot create an Azure cloud yet: the offers are
//! informational and never rentable.
use super::{
    AZURE_DEPLOYABLE, DEFAULT_LIMIT, DEFAULT_STORAGE_GB, MAX_HOURS, MAX_LIMIT, MONTH_HOURS, Offer, Requirements,
    normalize,
};
use crate::azure::catalog::{self, Catalog, OS_DISK_GB};

const USD: &str = crate::provider::AZURE.currency;
const LABEL: &str = crate::provider::AZURE.label;

/// Azure offers in `catalog` meeting `requirements`, cheapest estimated total first.
/// Azure has no GPU offers here, so a GPU request gets none. `max_hourly` is read in
/// US dollars.
#[must_use]
pub fn azure(catalog: &Catalog, requirements: &Requirements) -> Vec<Offer> {
    let mut offers = azure_catalog(catalog, requirements);
    offers.truncate(requirements.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT));
    offers
}

/// The Azure part of a `cloud_offers` answer, in US dollars and not deployable.
#[must_use]
pub fn azure_section(catalog: &Catalog, requirements: &Requirements) -> serde_json::Value {
    serde_json::json!({
        "provider": LABEL,
        "currency": USD,
        "deployable": AZURE_DEPLOYABLE,
        "offers": azure(catalog, requirements),
    })
}

/// Every matching Azure offer, before presentation limits.
#[must_use]
pub fn azure_catalog(catalog: &Catalog, requirements: &Requirements) -> Vec<Offer> {
    if requirements.gpu {
        return Vec::new();
    }
    // As for Hetzner: one hour when none is given, and a longer or unreadable
    // duration priced as a year. A created VM and its address bill at least an hour.
    let hours = match requirements.hours {
        None => 1.0,
        Some(hours) if hours.is_nan() => MAX_HOURS,
        Some(hours) => hours.clamp(0.0, MAX_HOURS),
    }
    .max(1.0);
    let disks = Disks {
        os_gb: OS_DISK_GB.max(u32::from(crate::Storage::default().container_gb)),
        workspace_gb: u32::from(requirements.storage_gb.unwrap_or(DEFAULT_STORAGE_GB)),
    };
    let wanted = requirements.region.as_deref().map(normalize);
    let mut offers: Vec<Offer> = catalog
        .offers
        .iter()
        .filter(|offer| {
            wanted.as_ref().is_none_or(|wanted| {
                *wanted == normalize(&offer.region)
                    || catalog
                        .regions
                        .get(&offer.region)
                        .is_some_and(|label| normalize(label) == *wanted)
            })
        })
        .filter_map(|offer| priced(offer, catalog, requirements, disks, hours))
        .collect();
    offers.retain(|offer| requirements.max_hourly.is_none_or(|max| offer.hourly <= max));
    offers.sort_by(|a, b| {
        a.estimated_total
            .total_cmp(&b.estimated_total)
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.location.cmp(&b.location))
    });
    offers
}

/// The managed disks of one worker, in GiB.
#[derive(Clone, Copy)]
struct Disks {
    /// Holds the system and the container images.
    os_gb: u32,
    /// Holds `/workspace` and survives a rebuild on a new VM.
    workspace_gb: u32,
}

fn priced(
    offer: &catalog::Offer,
    catalog: &Catalog,
    requirements: &Requirements,
    disks: Disks,
    hours: f64,
) -> Option<Offer> {
    let (vcpu, memory_gb) = (offer.vcpu, offer.memory_gb);
    if vcpu < requirements.min_vcpu.unwrap_or(0) || memory_gb < requirements.min_memory_gb.unwrap_or(0) {
        return None;
    }
    // Every worker has both disks and a static address; an offer that cannot price one
    // of them is left out rather than shown as cheaper than it is.
    let disks_month =
        catalog.disk_month(&offer.region, disks.os_gb)? + catalog.disk_month(&offer.region, disks.workspace_gb)?;
    let address_hour = catalog.static_ip_hour_usd.get(&offer.region).copied()?;
    Some(Offer {
        provider: LABEL,
        currency: USD,
        kind: "cpu",
        id: offer.size.clone(),
        name: format!("{} · {vcpu} vCPU · {memory_gb} GB", offer.size),
        vcpu: Some(vcpu),
        memory_gb: Some(memory_gb),
        gpu_memory_gb: None,
        hourly: offer.hourly_usd,
        flavors: Vec::new(),
        estimated_total: (offer.hourly_usd + address_hour) * hours + disks_month * hours / MONTH_HOURS,
        monthly: None,
        stopped_monthly: disks_month + address_hour * MONTH_HOURS,
        location: Some(offer.region.clone()),
        availability: "checked_at_creation",
        regions_in_stock: Vec::new(),
        host: "provider_operated",
        interruptible: false,
        rentable: AZURE_DEPLOYABLE,
    })
}

#[cfg(test)]
mod tests;
