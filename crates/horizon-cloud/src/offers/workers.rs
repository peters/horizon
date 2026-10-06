//! Complete provider catalogs for a repository profile, before browsing filters.
use super::{Offer, Place, Requirements, catalog, hetzner, hetzner_catalog, places};
use crate::{
    Profile,
    hetzner::catalog::Catalog,
    prices::{Preferences, PriceList},
    provider,
};

pub struct Workers {
    pub offers: Vec<Offer>,
    pub matching: usize,
    pub places: Vec<Vec<Place>>,
}

/// Every policy-allowed worker; below-minimum workers remain available for inspection.
#[must_use]
pub fn workers(
    profile: &Profile,
    hours: f64,
    runpod: Option<&(PriceList, Preferences)>,
    hetzner: Option<&Catalog>,
) -> Workers {
    let requirements = Requirements {
        hours: Some(hours),
        ..Requirements::for_profile(profile)
    };
    let browsing = Requirements {
        min_vcpu: None,
        min_memory_gb: None,
        min_gpu_memory_gb: None,
        ..requirements.clone()
    };
    let mut all = Vec::new();
    if provider::RUNPOD.supports(profile)
        && let Some((list, preferences)) = runpod
    {
        all.extend(catalog(
            list,
            preferences,
            &browsing,
            (profile.storage.volume_tier, profile.storage.container_gb),
        ));
    }
    if provider::HETZNER.supports(profile)
        && let Some(list) = hetzner
    {
        all.extend(hetzner_catalog(list, &browsing).into_iter().filter(|offer| {
            list.offers.iter().any(|worker| {
                worker.server_type == offer.id
                    && Some(&worker.location) == offer.location.as_ref()
                    && worker.disk_gb >= u32::from(profile.storage.container_gb)
            })
        }));
    }
    let (mut offers, excluded): (Vec<_>, Vec<_>) = all
        .into_iter()
        .partition(|offer| requirements.resource_reason(offer).is_none());
    let matching = offers.len();
    offers.extend(excluded);
    let places = offers
        .iter()
        .map(|offer| {
            if let Some(location) = &offer.location {
                vec![Place {
                    id: location.clone(),
                    region: hetzner
                        .and_then(|catalog| catalog.regions.get(location))
                        .cloned()
                        .unwrap_or_default(),
                    availability: hetzner::advisory_stock(offer),
                }]
            } else {
                runpod.map_or_else(Vec::new, |(list, _)| places(list, offer, profile.storage.volume_tier))
            }
        })
        .collect();
    Workers {
        offers,
        matching,
        places,
    }
}
