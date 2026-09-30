//! What a person chooses a worker from: where each offer can run, with its stock
//! there, and three starting points, so the cheapest or the most capable fit is one
//! click away. Read only, like the offers themselves.
use super::{Offer, Requirements};
use crate::Profile;
use crate::prices::{Availability, PriceList};
use crate::runpod::volumes::Tier;
use std::cmp::Ordering;

impl Requirements {
    /// Why a listed worker falls below this request's resource minimums.
    #[must_use]
    pub fn resource_reason(&self, offer: &Offer) -> Option<&'static str> {
        if self
            .min_vcpu
            .is_some_and(|minimum| offer.vcpu.is_none_or(|value| value < minimum))
        {
            Some("Below the profile's minimum CPU count")
        } else if self
            .min_memory_gb
            .is_some_and(|minimum| offer.memory_gb.is_none_or(|value| value < minimum))
        {
            Some("Below the profile's minimum memory")
        } else if self
            .min_gpu_memory_gb
            .is_some_and(|minimum| offer.gpu_memory_gb.is_none_or(|value| value < minimum))
        {
            Some("Below the profile's minimum GPU memory")
        } else {
            None
        }
    }

    /// What `profile` asks of a worker: its vCPU and memory are minimums for CPU
    /// workers, and its GPU memory floor applies to GPU workers. Its workspace storage
    /// is priced in, and every matching GPU type is listed, sold out or not.
    #[must_use]
    pub fn for_profile(profile: &Profile) -> Self {
        let mut requirements = Self {
            gpu: profile.gpu,
            storage_gb: Some(profile.storage.volume_gb.max(1)),
            ..Self::default()
        };
        if profile.gpu {
            requirements.min_gpu_memory_gb = profile.min_gpu_memory_gb;
            requirements.include_unavailable = true;
        } else {
            requirements.min_vcpu = Some(profile.cpu);
            requirements.min_memory_gb = Some(profile.memory_gb);
        }
        requirements
    }
}

/// An allowed data center an offer can run in, with the stock the catalog reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub id: String,
    /// The provider's region, such as `EUROPE`.
    pub region: String,
    pub availability: Availability,
}

/// Allowed data centers where `offer` can run, best stock first and then by name.
///
/// A GPU offer can run in every allowed data center, sold out where its type is not
/// listed. A CPU offer needs a data
/// center that holds a `tier` workspace volume and reports one of its flavor families,
/// and shows the best of their stock: the family, not the exact size, which is checked
/// on its own before a launch.
#[must_use]
pub fn places(list: &PriceList, offer: &Offer, tier: Tier) -> Vec<Place> {
    let mut places: Vec<Place> = list
        .data_centers
        .iter()
        .filter_map(|center| {
            // A data center reports only the GPU types it has capacity for, so one it
            // does not list is sold out there.
            let availability = if offer.kind == "gpu" {
                center
                    .gpus
                    .iter()
                    .find(|(id, _)| *id == offer.id)
                    .map_or(Availability::None, |&(_, level)| level)
            } else if center.holds(tier) {
                center
                    .cpus
                    .iter()
                    .filter(|(id, _)| offer.flavors.contains(id))
                    .map(|&(_, level)| level)
                    .min()?
            } else {
                return None;
            };
            Some(Place {
                id: center.id.clone(),
                region: center.region.clone(),
                availability,
            })
        })
        .collect();
    places.sort_by(|a, b| a.availability.cmp(&b.availability).then_with(|| a.id.cmp(&b.id)));
    places
}

/// Three starting points among some offers, by index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Picks {
    pub cheapest: Option<usize>,
    /// Priced between the other two, closest to their geometric mean.
    pub balanced: Option<usize>,
    pub powerful: Option<usize>,
}

/// The cheapest offer, the most capable one and one priced between them, choosing
/// only among the offers `in_stock` when any are. A CPU offer is more capable with more
/// vCPUs, then more memory; GPU offers rank by price, which follows their performance
/// more closely than their memory does. Ties go to the pricier offer, usually the newer
/// generation. Each offer is picked at most once.
#[must_use]
pub fn picks(offers: &[Offer], in_stock: impl Fn(&Offer) -> bool) -> Picks {
    picks_by(offers, in_stock, |offer| offer.hourly)
}

/// The same resource choices, ranked by a caller's comparable estimated cost.
#[must_use]
pub fn picks_by(offers: &[Offer], in_stock: impl Fn(&Offer) -> bool, price: impl Fn(&Offer) -> f64) -> Picks {
    picks_matching(offers, |_| true, in_stock, price)
}

/// Picks respecting a provider or location filter, retaining original catalog indices.
#[must_use]
pub fn picks_matching(
    offers: &[Offer],
    eligible: impl Fn(&Offer) -> bool,
    in_stock: impl Fn(&Offer) -> bool,
    price: impl Fn(&Offer) -> f64,
) -> Picks {
    let eligible: Vec<usize> = (0..offers.len()).filter(|&index| eligible(&offers[index])).collect();
    let stocked: Vec<usize> = eligible
        .iter()
        .copied()
        .filter(|&index| in_stock(&offers[index]))
        .collect();
    let pool = if stocked.is_empty() { eligible } else { stocked };
    let cheapest = pool
        .iter()
        .copied()
        .min_by(|&a, &b| price(&offers[a]).total_cmp(&price(&offers[b])));
    let powerful = pool
        .iter()
        .copied()
        .max_by(|&a, &b| capability(&offers[a], &offers[b], &price))
        .filter(|&index| Some(index) != cheapest);
    let balanced = cheapest.zip(powerful).and_then(|(low, high)| {
        let target = f64::midpoint(
            price(&offers[low]).max(f64::MIN_POSITIVE).ln(),
            price(&offers[high]).max(f64::MIN_POSITIVE).ln(),
        );
        pool.iter()
            .copied()
            .filter(|&index| index != low && index != high)
            .min_by(|&a, &b| {
                let distance = |index: usize| (price(&offers[index]).max(f64::MIN_POSITIVE).ln() - target).abs();
                distance(a).total_cmp(&distance(b))
            })
    });
    Picks {
        cheapest,
        balanced,
        powerful,
    }
}

fn capability(a: &Offer, b: &Offer, price: &impl Fn(&Offer) -> f64) -> Ordering {
    let size = |offer: &Offer| (offer.vcpu.unwrap_or(0), offer.memory_gb.unwrap_or(0));
    let by_size = if a.kind == "gpu" {
        Ordering::Equal
    } else {
        size(a).cmp(&size(b))
    };
    by_size.then_with(|| price(a).total_cmp(&price(b)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prices::DataCenter;

    fn offer(kind: &'static str, id: &str, hourly: f64, size: (u16, u16)) -> Offer {
        Offer {
            provider: "RunPod",
            currency: "USD",
            kind,
            id: id.into(),
            name: id.into(),
            vcpu: (kind == "cpu").then_some(size.0),
            memory_gb: (kind == "cpu").then_some(size.1),
            gpu_memory_gb: (kind == "gpu").then_some(size.1),
            hourly,
            flavors: if kind == "cpu" {
                vec!["cpu3c".into()]
            } else {
                Vec::new()
            },
            estimated_total: hourly,
            monthly: None,
            stopped_monthly: 0.0,
            location: None,
            availability: "high",
            regions_in_stock: Vec::new(),
            host: "provider_operated",
            interruptible: false,
            rentable: true,
        }
    }

    #[test]
    fn picks_span_cheap_to_capable_and_prefer_stock() {
        let offers = [
            offer("cpu", "cpu-2-4", 0.06, (2, 4)),
            offer("cpu", "cpu-4-8", 0.12, (4, 8)),
            offer("cpu", "cpu-8-16", 0.24, (8, 16)),
            offer("cpu", "cpu-16-32", 0.48, (16, 32)),
            offer("cpu", "cpu-32-64", 0.96, (32, 64)),
        ];
        let all = picks(&offers, |_| true);
        assert_eq!(
            all,
            Picks {
                cheapest: Some(0),
                balanced: Some(2),
                powerful: Some(4)
            }
        );
        // Sold-out offers are skipped while any offer is in stock.
        let stocked = picks(&offers, |offer| offer.id != "cpu-2-4" && offer.id != "cpu-32-64");
        assert_eq!(
            stocked,
            Picks {
                cheapest: Some(1),
                balanced: Some(2),
                powerful: Some(3)
            }
        );
        // With nothing in stock, every offer can still be a starting point to watch.
        assert_eq!(picks(&offers, |_| false), all);
    }

    #[test]
    fn one_or_two_offers_are_never_picked_twice() {
        let one = [offer("gpu", "a", 0.3, (0, 24))];
        assert_eq!(
            picks(&one, |_| true),
            Picks {
                cheapest: Some(0),
                ..Picks::default()
            }
        );
        let two = [offer("gpu", "a", 0.3, (0, 24)), offer("gpu", "b", 2.0, (0, 80))];
        assert_eq!(
            picks(&two, |_| true),
            Picks {
                cheapest: Some(0),
                balanced: None,
                powerful: Some(1)
            }
        );
        assert_eq!(picks(&[], |_| true), Picks::default());
    }

    #[test]
    fn excluded_workers_explain_which_resource_is_below_the_minimum() {
        let requirements = Requirements {
            min_vcpu: Some(4),
            min_memory_gb: Some(16),
            ..Requirements::default()
        };
        assert_eq!(
            requirements.resource_reason(&offer("cpu", "small", 0.1, (2, 4))),
            Some("Below the profile's minimum CPU count")
        );
        assert_eq!(
            requirements.resource_reason(&offer("cpu", "low-memory", 0.2, (4, 8))),
            Some("Below the profile's minimum memory")
        );
        assert_eq!(requirements.resource_reason(&offer("cpu", "fits", 0.3, (4, 16))), None);
        let gpu = Requirements {
            gpu: true,
            min_gpu_memory_gb: Some(24),
            ..Requirements::default()
        };
        assert_eq!(
            gpu.resource_reason(&offer("gpu", "small", 0.2, (0, 16))),
            Some("Below the profile's minimum GPU memory")
        );
        assert_eq!(gpu.resource_reason(&offer("gpu", "fits", 0.4, (0, 24))), None);
    }

    #[test]
    fn gpus_rank_by_price_and_cpu_ties_go_to_the_pricier_flavor() {
        let gpus = [
            offer("gpu", "big-memory", 2.39, (0, 192)),
            offer("gpu", "fast", 5.99, (0, 180)),
            offer("gpu", "small", 0.2, (0, 16)),
        ];
        assert_eq!(picks(&gpus, |_| true).powerful, Some(1));
        let cpus = [
            offer("cpu", "cpu3c", 0.24, (8, 16)),
            offer("cpu", "cpu5c", 0.28, (8, 16)),
        ];
        assert_eq!(picks(&cpus, |_| true).powerful, Some(1));
    }

    #[test]
    fn places_follow_storage_tier_and_offer_kind() {
        let center =
            |id: &str, standard, fast, cpus: &[(&str, Availability)], gpus: &[(&str, Availability)]| DataCenter {
                id: id.into(),
                region: "EUROPE".into(),
                workspace_storage: standard,
                high_performance_storage: fast,
                gpus: gpus.iter().map(|&(id, level)| (id.into(), level)).collect(),
                cpus: cpus.iter().map(|&(id, level)| (id.into(), level)).collect(),
            };
        let list = PriceList {
            provider: "RunPod",
            cpu: Vec::new(),
            gpus: Vec::new(),
            data_centers: vec![
                center(
                    "EU-RO-1",
                    true,
                    false,
                    &[("cpu3c", Availability::Low)],
                    &[("l4", Availability::None)],
                ),
                center("EU-SE-1", true, true, &[("cpu3c", Availability::High)], &[]),
                center(
                    "US-KS-2",
                    false,
                    true,
                    &[("cpu3g", Availability::High)],
                    &[("l4", Availability::High)],
                ),
                center("CA-MTL-3", true, false, &[], &[]),
            ],
            regions: std::collections::BTreeMap::new(),
            storage: crate::runpod::prices::STORAGE,
        };
        let ids = |places: Vec<Place>| {
            places
                .into_iter()
                .map(|place| (place.id, place.availability))
                .collect::<Vec<_>>()
        };
        let cpu = offer("cpu", "cpu-8-16", 0.24, (8, 16));
        assert_eq!(
            ids(places(&list, &cpu, Tier::Standard)),
            [
                ("EU-SE-1".to_owned(), Availability::High),
                ("EU-RO-1".to_owned(), Availability::Low)
            ]
        );
        assert_eq!(
            ids(places(&list, &cpu, Tier::HighPerformance)),
            [("EU-SE-1".to_owned(), Availability::High)]
        );
        let gpu = offer("gpu", "l4", 0.4, (0, 24));
        assert_eq!(
            ids(places(&list, &gpu, Tier::Standard)),
            [
                ("US-KS-2".to_owned(), Availability::High),
                ("CA-MTL-3".to_owned(), Availability::None),
                ("EU-RO-1".to_owned(), Availability::None),
                ("EU-SE-1".to_owned(), Availability::None)
            ]
        );
    }

    #[test]
    fn a_profile_sets_minimums_for_its_kind_of_worker() {
        let config = crate::CloudConfig::parse(crate::EXAMPLE).unwrap();
        let cpu = Requirements::for_profile(&config.profiles["development"]);
        assert_eq!((cpu.gpu, cpu.min_vcpu, cpu.min_memory_gb), (false, Some(4), Some(8)));
        assert!(cpu.validate().is_ok());
        let mut profile = config.profiles["gpu"].clone();
        profile.min_gpu_memory_gb = Some(48);
        let gpu = Requirements::for_profile(&profile);
        assert_eq!((gpu.gpu, gpu.min_vcpu, gpu.min_gpu_memory_gb), (true, None, Some(48)));
        assert!(gpu.include_unavailable && gpu.validate().is_ok());
    }
}
