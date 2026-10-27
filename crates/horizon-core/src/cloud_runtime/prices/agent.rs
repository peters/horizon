//! The provider an agent's cloud runs on: the cheapest offer among the providers the
//! machine's agent policy allows. A person chooses the provider in New cloud; an agent's
//! create has no person, so this ranking chooses one for it. `RunPod` and Hetzner are the
//! catalogs it reads, so a policy that allows only a provider whose offers it cannot rank
//! gets no offer, and the create is refused rather than moved to another provider. The
//! choice carries that offer's worker, so the deployment requests the worker whose rate
//! the policy admits and not one of the machine's fallback settings.
use super::{Profile, hetzner_catalog, price_list};
use crate::cloud_panel::{ChosenWorker, Placement, WorkerChoice};
use crate::cloud_runtime::{
    Cancellation,
    offers::{Offer, Workers, comparison, exchange::Rates, workers},
    provider::{self, Description},
    settings::Settings,
};

/// How long an agent's cloud is priced to run when its offers are ranked: one hour, as a
/// `cloud_offers` request that names no duration.
const HOURS: f64 = 1.0;

/// The worker an agent's cloud rents, and what it bills per hour.
pub struct Chosen {
    /// The provider the cloud runs on, as the chosen offer names it.
    pub provider: &'static Description,
    /// That offer's compute rate per hour, in the provider's own currency: the rate the
    /// agent policy compares with its limit.
    pub hourly: f64,
    /// The size that offer names, where the provider bills a worker by its flavor.
    pub size: Option<(u16, u16)>,
    /// The placement that offer names: the server type and location of the worker, so the
    /// deployment asks for this offer and not for a fallback one.
    pub placement: Placement,
}

/// The worker an agent's cloud rents for `profile` among the providers `allowed` names:
/// the cheapest of their offers that meets the profile. `None` when none of them offers
/// such a worker, or when this machine cannot read their prices.
#[must_use]
pub fn choose(profile: &Profile, allowed: &[String], settings: &Settings, cancel: &Cancellation) -> Option<Chosen> {
    // A provider whose prices this machine cannot read contributes no offer, so a binding
    // that is missing for one allowed provider never refuses a create on another.
    let permits = |description: &Description| allowed.iter().any(|id| id == description.id);
    let runpod = (permits(&provider::RUNPOD) && settings.runpod_configured())
        .then(|| price_list(settings, cancel).ok())
        .flatten();
    let hetzner = permits(&provider::HETZNER)
        .then(|| hetzner_catalog(settings, cancel).ok().flatten())
        .flatten();
    if runpod.is_none() && hetzner.is_none() {
        return None;
    }
    let ranked = workers(profile, HOURS, runpod.as_ref(), hetzner.as_ref());
    let rates = needs_rates(&ranked)
        .then(Rates::fetch)
        .and_then(std::result::Result::ok);
    cheapest(profile, allowed, &ranked, rates.as_ref())
}

/// The cheapest offer in `ranked` of a provider `allowed` names, in the order
/// `cloud_offers` ranks offers: a comparable estimated total, with an unquoted total last,
/// and then the provider name. Equal ranks keep the earlier offer, in the order the
/// provider's own catalog lists it, so one reading of the prices chooses one worker. An
/// offer below the profile's requirements, or one Horizon cannot create a cloud on, is
/// never chosen. The chosen offer's own worker comes with it: its size and its placement.
fn cheapest(profile: &Profile, allowed: &[String], ranked: &Workers, rates: Option<&Rates>) -> Option<Chosen> {
    let mut best: Option<(comparison::Rank<'_>, &Offer, ChosenWorker)> = None;
    for offer in ranked.offers.iter().take(ranked.matching) {
        let Ok(chosen) = WorkerChoice::from(offer).for_profile(profile) else {
            continue;
        };
        if !allowed.iter().any(|id| id == chosen.provider.id) {
            continue;
        }
        let rank = comparison::Rank::of(offer, dollars(offer, rates));
        if best
            .as_ref()
            .is_none_or(|(best_rank, _, _)| rank.order(best_rank).is_lt())
        {
            best = Some((rank, offer, chosen));
        }
    }
    best.map(|(_, offer, chosen)| Chosen {
        provider: chosen.provider,
        hourly: offer.hourly,
        size: chosen.size,
        placement: chosen.placement,
    })
}

/// An offer's estimated total in dollars, the currency the ranking compares in.
fn dollars(offer: &Offer, rates: Option<&Rates>) -> Option<f64> {
    comparison::dollars(offer.estimated_total, offer.currency, rates)
}

/// Whether ranking the offers needs a currency quote: one of them bills in another
/// currency than the dollars the ranking compares in.
fn needs_rates(ranked: &Workers) -> bool {
    ranked
        .offers
        .iter()
        .take(ranked.matching)
        .any(|offer| offer.currency != provider::RUNPOD.currency)
}

#[cfg(test)]
mod tests {
    use super::super::{HetznerCatalog, Preferences, PriceList, RUNPOD_STORAGE};
    use super::*;
    use crate::cloud_runtime::prices::{Availability, CpuFlavorPrice, DataCenter};
    use crate::cloud_runtime::repository::launch::quick_start;
    use crate::cloud_runtime::settings::{AgentLifecycle, AgentLifecycleRefusal};
    use std::{collections::BTreeMap, path::Path};
    use time::OffsetDateTime;

    /// The profile an agent's quick-start create deploys.
    fn profile() -> Profile {
        quick_start::builtin().unwrap().profiles[quick_start::PROFILE].clone()
    }

    /// A `RunPod` price list whose only CPU flavor costs 0.03 dollars per vCPU-hour.
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
            storage: RUNPOD_STORAGE,
        };
        (
            list,
            Preferences {
                cpu_flavors: vec!["cpu3c".into()],
                gpu_types: Vec::new(),
            },
        )
    }

    /// A Hetzner catalog whose 2 vCPU server is cheaper and 4 vCPU server dearer than
    /// `RunPod`'s, before the euro is converted.
    fn hetzner() -> HetznerCatalog {
        let offer = |server_type: &str, cores, memory_gb, hourly_eur| {
            serde_json::json!({"server_type": server_type, "location": "hel1", "cores": cores,
                "memory_gb": memory_gb, "disk_gb": 160, "dedicated": false, "hourly_eur": hourly_eur,
                "monthly_eur": hourly_eur * 600.0, "available": true, "recommended": false})
        };
        serde_json::from_value(serde_json::json!({
            "offers": [offer("cx23", 2, 4.0, 0.0088), offer("cx33", 4, 8.0, 0.0136)],
            "volume_gb_month_eur": 0.0572, "ipv4_month_eur": {"hel1": 0.5},
            "ipv4_hour_eur": {"hel1": 0.0008}, "regions": {"hel1": "EUROPE"},
        }))
        .unwrap()
    }

    /// Reference rates for a ranking that compares euros with dollars.
    fn rates(usd_per_eur: f64) -> Rates {
        Rates {
            date: OffsetDateTime::now_utc().date().to_string(),
            usd_per_unit: BTreeMap::from([("USD".into(), 1.0), ("EUR".into(), usd_per_eur)]),
        }
    }

    /// Both providers' offers for the quick-start profile, as an agent's create ranks them.
    fn ranked() -> Workers {
        let (list, preferences) = runpod();
        workers(&profile(), HOURS, Some(&(list, preferences)), Some(&hetzner()))
    }

    /// The providers and rates the ranking holds for offers that meet the profile.
    fn rated(ranked: &Workers) -> Vec<(&'static str, f64)> {
        ranked
            .offers
            .iter()
            .take(ranked.matching)
            .map(|offer| (offer.provider, offer.hourly))
            .collect()
    }

    fn policy(providers: &[&str]) -> AgentLifecycle {
        AgentLifecycle {
            providers: providers.iter().map(|id| (*id).to_owned()).collect(),
            max_hourly: 1.0,
            repositories: vec!["/home/person/shop".into()],
            allow_github_sign_in_skip: false,
        }
    }

    /// What the create answers after choosing: the policy admits the chosen provider at the
    /// chosen rate.
    fn admitted(chosen: &Chosen, providers: &[&str]) -> Result<(), AgentLifecycleRefusal> {
        policy(providers).admits(Path::new("/home/person/shop"), chosen.provider.id, Some(chosen.hourly))
    }

    #[test]
    fn a_policy_that_allows_only_hetzner_rents_on_hetzner() {
        let ranked = ranked();
        // The RunPod worker is the cheaper one at these rates, and the policy still chooses
        // the cheaper of the two workers Hetzner offers.
        let chosen = cheapest(&profile(), &["hetzner".into()], &ranked, Some(&rates(9.0))).unwrap();
        assert_eq!(chosen.provider.id, "hetzner");
        assert_eq!(chosen.hourly, 0.0088, "the cheaper of its two offered workers");
        assert_eq!(admitted(&chosen, &["hetzner"]), Ok(()));
        // A provider the policy does not list is still refused, as it was before: the
        // create chooses the listed provider instead of asking for one.
        assert_eq!(
            policy(&["hetzner"]).admits(Path::new("/home/person/shop"), "runpod", Some(0.06)),
            Err(AgentLifecycleRefusal::Provider)
        );
    }

    #[test]
    fn a_policy_that_allows_only_runpod_rents_on_runpod() {
        let ranked = ranked();
        // Hetzner's worker is the cheaper one at these rates.
        let chosen = cheapest(&profile(), &["runpod".into()], &ranked, Some(&rates(1.2))).unwrap();
        assert_eq!(chosen.provider.id, "runpod");
        assert_eq!(
            chosen.hourly, 0.06,
            "2 vCPU of its flavor, whatever the other provider asks"
        );
        assert_eq!(admitted(&chosen, &["runpod"]), Ok(()));
    }

    #[test]
    fn the_cheaper_allowed_offer_wins_and_an_unquoted_one_ranks_last() {
        let ranked = ranked();
        let allowed = ["runpod".to_owned(), "hetzner".to_owned()];
        // At 1.2 dollars per euro the euro offer is cheaper, and at 9 it is dearer, so the
        // policy that allows both follows the prices rather than the provider's name.
        let cheap_euro = cheapest(&profile(), &allowed, &ranked, Some(&rates(1.2))).unwrap();
        assert_eq!(cheap_euro.provider.id, "hetzner");
        let dear_euro = cheapest(&profile(), &allowed, &ranked, Some(&rates(9.0))).unwrap();
        assert_eq!(dear_euro.provider.id, "runpod");
        // Without a quote the euro offer has no comparable total and ranks last, as it does
        // in `cloud_offers`.
        let unquoted = cheapest(&profile(), &allowed, &ranked, None).unwrap();
        assert_eq!(unquoted.provider.id, "runpod");
        // One catalog alone needs no quote and keeps its own order.
        let alone = cheapest(&profile(), &["hetzner".into()], &ranked, None).unwrap();
        assert_eq!(alone.hourly, 0.0088);
    }

    #[test]
    fn a_policy_whose_providers_have_no_eligible_offer_chooses_nothing() {
        let ranked = ranked();
        // Neither provider's catalog holds a worker this policy may rent on.
        assert!(cheapest(&profile(), &["azure".into()], &ranked, None).is_none());
        assert!(cheapest(&profile(), &["docker-host".into()], &ranked, None).is_none());
        // A repository profile larger than every offered worker has no offer either.
        let larger = Profile {
            cpu: 32,
            memory_gb: 128,
            ..profile()
        };
        let allowed = ["runpod".to_owned(), "hetzner".to_owned()];
        assert!(cheapest(&larger, &allowed, &ranked, None).is_none());
    }

    #[test]
    fn the_admitted_rate_is_the_hourly_price_of_the_chosen_providers_offer() {
        let ranked = ranked();
        let offers = rated(&ranked);
        for allowed in [["runpod".to_owned()], ["hetzner".to_owned()]] {
            let chosen = cheapest(&profile(), &allowed, &ranked, Some(&rates(1.2))).unwrap();
            let label = chosen.provider.label;
            let own: Vec<f64> = offers
                .iter()
                .filter(|(provider, _)| *provider == label)
                .map(|(_, hourly)| *hourly)
                .collect();
            assert!(own.contains(&chosen.hourly), "{own:?} holds {}", chosen.hourly);
            let other: Vec<f64> = offers
                .iter()
                .filter(|(provider, _)| *provider != label)
                .map(|(_, hourly)| *hourly)
                .collect();
            assert!(
                !other.contains(&chosen.hourly),
                "{label} was admitted at the other provider's rate"
            );
        }
    }

    #[test]
    fn the_chosen_offer_carries_the_worker_its_rate_admits() {
        let ranked = ranked();
        // The worker is the deployment's request: its size and its placement are the
        // offer's own, so a fallback server type or location cannot replace it.
        let hetzner = cheapest(&profile(), &["hetzner".into()], &ranked, Some(&rates(9.0))).unwrap();
        assert_eq!(hetzner.hourly, 0.0088);
        assert_eq!(hetzner.size, Some((2, 4)), "cx23 has 2 vCPU and 4 GB");
        assert_eq!(hetzner.placement.cpu_types, ["cx23"]);
        assert_eq!(hetzner.placement.data_centers, ["hel1"]);
        // `RunPod` places a CPU worker by its size alone, which the deployment requests.
        let runpod = cheapest(&profile(), &["runpod".into()], &ranked, Some(&rates(1.2))).unwrap();
        assert_eq!(runpod.size, Some((2, 4)));
        assert!(runpod.placement.is_default());
    }

    #[test]
    fn a_machine_that_cannot_read_any_prices_offers_nothing() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        let absolute = |name: &str| root.path().join(name);
        std::fs::write(
            &path,
            serde_json::json!({
                "runpod_key_file": absolute("missing-key"), "ssh_identity_file": absolute("identity"),
                "docker_config": absolute("docker"), "registry_pull_auth_id": null,
                "cpu_flavors": ["cpu3c"], "gpu_types": [], "data_centers": ["EU-RO-1"],
            })
            .to_string(),
        )
        .unwrap();
        let settings = crate::cloud_runtime::settings::Settings::load(&path).unwrap();
        let cancel = Cancellation::default();
        // This machine binds no Hetzner token and its RunPod key file is missing, so an
        // agent may rent on neither provider.
        let allowed = ["runpod".to_owned(), "hetzner".to_owned()];
        assert!(choose(&profile(), &allowed, &settings, &cancel).is_none());
    }
}
