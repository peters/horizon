//! Complete provider catalogs for a repository profile, before browsing filters.
use super::{Offer, Place, Requirements, catalog, comparison::Rank, hetzner, hetzner_catalog, places};
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

impl Workers {
    /// Orders the matching workers, and then the rest on their own, by a comparable
    /// estimated `total`, in the order the `cloud_offers` comparison uses. Each worker
    /// keeps its places.
    pub fn order_by(&mut self, total: impl Fn(&Offer) -> Option<f64>) {
        let mut rows: Vec<_> = std::mem::take(&mut self.offers)
            .into_iter()
            .zip(std::mem::take(&mut self.places))
            .map(|(offer, places)| (total(&offer), offer, places))
            .collect();
        let matching = self.matching.min(rows.len());
        let (matching, others) = rows.split_at_mut(matching);
        for section in [matching, others] {
            section.sort_by(|(a_total, a, _), (b_total, b, _)| Rank::of(a, *a_total).order(&Rank::of(b, *b_total)));
        }
        (self.offers, self.places) = rows.into_iter().map(|(_, offer, places)| (offer, places)).unzip();
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offers::{comparison, exchange::Rates};
    use crate::prices::{Availability, CpuFlavorPrice, DataCenter};
    use std::collections::BTreeMap;

    fn runpod() -> (PriceList, Preferences) {
        let list = PriceList {
            provider: "RunPod",
            cpu: vec![CpuFlavorPrice {
                id: "cpu3c".into(),
                name: "Compute-Optimized".into(),
                per_vcpu_hour: 0.03,
            }],
            gpus: Vec::new(),
            data_centers: vec![DataCenter {
                id: "EU-RO-1".into(),
                region: "EUROPE".into(),
                workspace_storage: true,
                high_performance_storage: false,
                gpus: Vec::new(),
                cpus: vec![("cpu3c".into(), Availability::High)],
            }],
            regions: BTreeMap::new(),
            storage: crate::runpod::prices::STORAGE,
        };
        let preferences = Preferences {
            cpu_flavors: vec!["cpu3c".into()],
            gpu_types: Vec::new(),
        };
        (list, preferences)
    }

    fn hetzner() -> Catalog {
        let offer = |server_type: &str, cores, memory_gb, hourly| {
            serde_json::json!({"server_type": server_type, "location": "hel1", "cores": cores,
                "memory_gb": memory_gb, "disk_gb": 160, "dedicated": false, "hourly_eur": hourly,
                "monthly_eur": hourly * 600.0, "available": true, "recommended": false})
        };
        serde_json::from_value(serde_json::json!({
            "offers": [offer("cx23", 2, 4.0, 0.0088), offer("cx33", 4, 8.0, 0.0136), offer("cpx52", 16, 32.0, 0.25)],
            "volume_gb_month_eur": 0.0572, "ipv4_month_eur": {"hel1": 0.5},
            "ipv4_hour_eur": {"hel1": 0.0008}, "regions": {"hel1": "EUROPE"},
        }))
        .unwrap()
    }

    fn identity(offer: &Offer) -> (String, String, Option<String>) {
        (offer.provider.to_owned(), offer.id.clone(), offer.location.clone())
    }

    #[test]
    fn workers_follow_the_cloud_offers_comparison_order_across_providers() {
        let config = crate::CloudConfig::parse(crate::EXAMPLE).unwrap();
        let profile = &config.profiles["development"];
        let rates = Rates {
            date: time::OffsetDateTime::now_utc().date().to_string(),
            usd_per_unit: BTreeMap::from([("USD".into(), 1.0), ("EUR".into(), 1.2)]),
        };
        let mut workers = workers(profile, 1.0, Some(&runpod()), Some(&hetzner()));
        workers.order_by(|offer| comparison::dollars(offer.estimated_total, offer.currency, Some(&rates)));
        let matching = &workers.offers[..workers.matching];
        assert_eq!(identity(&matching[0]).1, "cx33", "the cheapest worker comes first");
        assert_ne!(
            matching.iter().map(|offer| offer.provider).collect::<Vec<_>>(),
            {
                let mut grouped: Vec<_> = matching.iter().map(|offer| offer.provider).collect();
                grouped.sort_unstable();
                grouped
            },
            "providers interleave by total rather than one after the other"
        );
        // A cloud_offers request for the same profile ranks the same workers in the same order.
        let request = crate::offers::Requirements {
            min_vcpu: Some(profile.cpu),
            min_memory_gb: Some(profile.memory_gb),
            limit: Some(50),
            ..crate::offers::Requirements::default()
        };
        let (list, preferences) = runpod();
        let mut answer = serde_json::json!({
            "offers": crate::offers::offers(&list, &preferences, &request),
            "other_providers": [crate::offers::hetzner_section(&hetzner(), &request)],
        });
        comparison::append(&mut answer, Some(&rates));
        let compared: Vec<_> = answer["comparison"]["offers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|offer| {
                (
                    offer["provider"].as_str().unwrap().to_owned(),
                    offer["id"].as_str().unwrap().to_owned(),
                    offer["location"].as_str().map(str::to_owned),
                )
            })
            .collect();
        assert_eq!(compared, matching.iter().map(identity).collect::<Vec<_>>());
        // Workers below the profile follow on their own, in the same order.
        let below = &workers.offers[workers.matching..];
        assert!(!below.is_empty() && below.iter().any(|offer| offer.id == "cx23"));
        let totals: Vec<_> = below
            .iter()
            .map(|offer| comparison::dollars(offer.estimated_total, offer.currency, Some(&rates)).unwrap())
            .collect();
        assert!(totals.is_sorted_by(|a, b| a <= b), "{totals:?}");
        // Each worker keeps its own places.
        for (offer, places) in workers.offers.iter().zip(&workers.places) {
            assert_eq!(offer.location.is_some(), places.iter().any(|place| place.id == "hel1"));
        }
    }

    #[test]
    fn an_unknown_total_ranks_last() {
        let mut workers = workers(
            &crate::CloudConfig::parse(crate::EXAMPLE).unwrap().profiles["development"],
            1.0,
            Some(&runpod()),
            Some(&hetzner()),
        );
        workers.order_by(|offer| (offer.currency == "USD").then_some(offer.estimated_total));
        let matching = &workers.offers[..workers.matching];
        let first_hetzner = matching.iter().position(|offer| offer.provider == "Hetzner").unwrap();
        assert!(
            matching[first_hetzner..]
                .iter()
                .all(|offer| offer.provider == "Hetzner")
        );
    }
}
