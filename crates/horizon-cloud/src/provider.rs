//! What each provider offers a new cloud, so interfaces show exactly the choices that
//! apply to it and never branch on provider names. Descriptive only: nothing here
//! talks to a provider.
use crate::Profile;

/// How a provider says where a worker runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Data centers grouped into regions, with live stock per size or GPU type.
    DataCenters,
    /// Named locations, each with its own server types and prices.
    Locations,
}

/// What happens to the worker when a cloud stops. Either way only the workspace
/// storage stays billed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoppedCost {
    /// The stopped worker is kept, so it resumes where it was; a GPU cloud's pod volume
    /// is its workspace and is billed at the stopped rate.
    WorkerKept,
    /// Stopping deletes the server; the workspace volume is kept and a new server is
    /// created on start.
    ServerDeleted,
}

/// How a provider prices a worker of a given size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pricing {
    /// Per vCPU-hour across CPU flavor families, such as compute-optimized; the provider
    /// allocates one of the requested flavors.
    Flavors,
    /// Per server type and location, with a monthly cap.
    ServerTypes,
}

/// A choice a provider supports beyond size and placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// A GPU type chosen for one cloud from those in stock.
    GpuType,
    /// Third-party hosts, which a person must opt into. No provider offers it yet:
    /// clouds use Secure Cloud hosts only.
    CommunityHosts,
    /// More than one kind of network volume storage. No provider offers it yet.
    VolumeTiers,
    /// A region picker with live stock.
    Region,
    /// Machine types tried in order when the first is sold out.
    ServerTypeFallback,
}

/// One provider as a new cloud sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Description {
    pub kind: Kind,
    /// The profile's `provider` value, such as `runpod`.
    pub id: &'static str,
    /// Shown to people, such as `RunPod`.
    pub label: &'static str,
    /// The currency the provider bills in, as an ISO 4217 code. Amounts are never
    /// converted.
    pub currency: &'static str,
    /// Whether prices exclude VAT.
    pub net_of_vat: bool,
    /// Whether GPU profiles can run there.
    pub gpu: bool,
    pub placement: Placement,
    pub pricing: Pricing,
    pub stopped: StoppedCost,
    /// The choices this provider supports; interfaces show only these.
    pub choices: &'static [Choice],
    /// Whether Horizon can create clouds there yet; otherwise its prices are for
    /// comparison only, and an interface that offers it must refuse to create there.
    pub creatable: bool,
    /// The workspace volume sizes a CPU cloud can have there, in GB, inclusive.
    pub cpu_volume_gb: (u32, u32),
    /// Who stops a worker there after `idle_stop_minutes` without activity.
    pub idle_stop: IdleStop,
    /// The provider's site, as a card names it, such as `runpod.io`.
    pub site: &'static str,
    /// Whether the provider pulls private images with a registry auth it stores
    /// (`registry_pull_auth_id`); otherwise the host logs in to the registry itself.
    pub registry_auth: bool,
}

/// Who stops an idle worker on a provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleStop {
    /// Nobody: a profile there may not set `idle_stop_minutes`.
    Unsupported,
    /// The worker itself, with a credential the provider gives it for its own billing.
    Worker,
    /// Horizon, from the idle record the worker keeps, and only while Horizon runs:
    /// the provider gives the worker no credential that could stop it.
    Horizon,
}

pub const RUNPOD: Description = Description {
    kind: Kind::RunPod,
    id: "runpod",
    label: "RunPod",
    currency: "USD",
    net_of_vat: false,
    gpu: true,
    placement: Placement::DataCenters,
    pricing: Pricing::Flavors,
    stopped: StoppedCost::WorkerKept,
    choices: &[Choice::GpuType, Choice::Region, Choice::VolumeTiers],
    creatable: true,
    idle_stop: IdleStop::Worker,
    registry_auth: true,
    site: "runpod.io",
    cpu_volume_gb: (
        *crate::runpod::volumes::REQUEST_SIZE_GB.start(),
        *crate::runpod::volumes::REQUEST_SIZE_GB.end(),
    ),
};

pub const HETZNER: Description = Description {
    kind: Kind::Hetzner,
    id: crate::hetzner::PROVIDER,
    label: "Hetzner",
    currency: "EUR",
    net_of_vat: true,
    gpu: false,
    placement: Placement::Locations,
    pricing: Pricing::ServerTypes,
    stopped: StoppedCost::ServerDeleted,
    choices: &[Choice::ServerTypeFallback],
    creatable: crate::offers::HETZNER_DEPLOYABLE,
    // The project token must never reach a worker, so Horizon stops it.
    idle_stop: IdleStop::Horizon,
    registry_auth: false,
    site: "hetzner.com",
    cpu_volume_gb: (
        *crate::hetzner::volumes::SIZE_GB.start(),
        *crate::hetzner::volumes::SIZE_GB.end(),
    ),
};

/// Which provider a description is, for code that must handle every provider: a
/// match on it is exhaustive, so adding a provider fails to compile until each such
/// place handles it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    RunPod,
    Hetzner,
}

/// Every provider a cloud can run on, in the order they are offered.
pub const ALL: [&Description; 2] = [&RUNPOD, &HETZNER];

/// The provider a profile names.
#[must_use]
pub fn by_id(id: &str) -> Option<&'static Description> {
    ALL.into_iter().find(|provider| provider.id == id)
}

impl Description {
    /// The provider `profile` names; a profile without a known one is a `RunPod` one,
    /// as profiles were before providers were named.
    #[must_use]
    pub fn of(profile: &Profile) -> &'static Self {
        by_id(&profile.provider).unwrap_or(&RUNPOD)
    }

    /// Whether this provider supports `choice`.
    #[must_use]
    pub fn offers(&self, choice: Choice) -> bool {
        self.choices.contains(&choice)
    }

    /// Whether a cloud of `profile` can run on this provider: the profile is valid as
    /// one of this provider's, with every provider rule, such as GPU support, hosted
    /// devices and the workspace volume size, applied.
    #[must_use]
    pub fn supports(&self, profile: &Profile) -> bool {
        let candidate = Profile {
            provider: self.id.to_owned(),
            ..profile.clone()
        };
        let (smallest, largest) = self.cpu_volume_gb;
        let volume = u32::from(profile.storage.volume_gb);
        candidate.validate(false).is_ok()
            && (profile.gpu || (smallest..=largest).contains(&volume))
            && (self.idle_stop != IdleStop::Unsupported || profile.idle_stop_minutes.is_none())
    }

    /// The providers among `configured` a cloud of `profile` can choose from, in the
    /// order they are offered. A choice is worth showing only when there is more than one.
    /// Providers that are not yet `creatable` are included so their prices can be
    /// compared; callers refuse to create on them.
    #[must_use]
    pub fn choices(profile: &Profile, configured: &[&'static Self]) -> Vec<&'static Self> {
        ALL.into_iter()
            .filter(|provider| configured.contains(provider) && provider.supports(profile))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(gpu: bool) -> Profile {
        let mut profile = crate::CloudConfig::parse(crate::EXAMPLE).unwrap().profiles["image-only"].clone();
        profile.gpu = gpu;
        profile
    }

    #[test]
    fn providers_describe_what_a_new_cloud_can_choose() {
        assert_eq!(by_id("runpod"), Some(&RUNPOD));
        assert_eq!(by_id("hetzner"), Some(&HETZNER));
        assert_eq!(by_id("fly"), None);
        assert_eq!((RUNPOD.currency, HETZNER.currency), ("USD", "EUR"));
        assert_eq!(HETZNER.stopped, StoppedCost::ServerDeleted);
        assert_eq!(RUNPOD.stopped, StoppedCost::WorkerKept);
        assert_eq!(
            (RUNPOD.pricing, HETZNER.pricing),
            (Pricing::Flavors, Pricing::ServerTypes)
        );
        assert_eq!(HETZNER.creatable, crate::offers::HETZNER_DEPLOYABLE);
        // Choices that are one provider's own concepts never apply to the other.
        for choice in [Choice::GpuType, Choice::Region, Choice::VolumeTiers] {
            assert!(RUNPOD.offers(choice) && !HETZNER.offers(choice), "{choice:?}");
        }
        // Choices the code cannot honor yet are offered by no provider.
        assert!(ALL.iter().all(|provider| !provider.offers(Choice::CommunityHosts)));
        assert!(HETZNER.offers(Choice::ServerTypeFallback) && !RUNPOD.offers(Choice::ServerTypeFallback));
    }

    #[test]
    fn choices_follow_configuration_and_the_profile() {
        let cpu = profile(false);
        let gpu = profile(true);
        assert_eq!(Description::choices(&cpu, &[&RUNPOD]), [&RUNPOD]);
        assert_eq!(Description::choices(&cpu, &[&HETZNER, &RUNPOD]), [&RUNPOD, &HETZNER]);
        assert_eq!(
            Description::choices(&gpu, &[&RUNPOD, &HETZNER]),
            [&RUNPOD],
            "Hetzner has no GPUs"
        );
        assert!(Description::choices(&gpu, &[&HETZNER]).is_empty());
        assert!(HETZNER.supports(&cpu) && !HETZNER.supports(&gpu));
    }

    #[test]
    fn a_provider_is_offered_only_for_profiles_it_accepts() {
        // Hosted devices are not available on Hetzner yet.
        let mut devices = profile(false);
        devices.capabilities.browserstack = Some(crate::BrowserStack::default());
        assert!(devices.validate(false).is_ok());
        assert_eq!(Description::choices(&devices, &[&RUNPOD, &HETZNER]), [&RUNPOD]);
        // A workspace volume above RunPod's limit fits only on Hetzner.
        let mut large = profile(false);
        large.provider = HETZNER.id.to_owned();
        large.storage.volume_gb = 5000;
        assert_eq!(Description::choices(&large, &[&RUNPOD, &HETZNER]), [&HETZNER]);
        large.storage.volume_gb = 4000;
        assert_eq!(Description::choices(&large, &[&RUNPOD, &HETZNER]), [&RUNPOD, &HETZNER]);
    }

    #[test]
    fn every_provider_a_profile_accepts_is_described() {
        for id in ["runpod", crate::hetzner::PROVIDER] {
            assert!(by_id(id).is_some(), "{id}");
        }
    }

    #[test]
    fn a_profile_names_its_provider_and_what_it_does() {
        let mut profile: Profile = serde_json::from_value(serde_json::json!({
            "provider": "hetzner", "image": "registry.example/worker", "cpu": 4, "memory_gb": 8,
            "storage": {"container_gb": 20, "volume_gb": 50}
        }))
        .unwrap();
        assert_eq!(Description::of(&profile), &HETZNER);
        // Only RunPod stores a pull auth.
        assert_eq!((RUNPOD.registry_auth, HETZNER.registry_auth), (true, false));
        assert_eq!((RUNPOD.site, HETZNER.site), ("runpod.io", "hetzner.com"));
        // A provider Horizon does not know reads as RunPod, as profiles were before providers.
        profile.provider = "elsewhere".into();
        assert_eq!(Description::of(&profile), &RUNPOD);
    }

    #[test]
    fn a_profile_with_an_idle_period_is_offered_only_where_someone_stops_the_worker() {
        let mut profile: Profile = serde_json::from_value(serde_json::json!({
            "provider": "runpod", "image": "registry.example/worker", "cpu": 4, "memory_gb": 8,
            "storage": {"container_gb": 20, "volume_gb": 50}
        }))
        .unwrap();
        let unsupported = Description {
            idle_stop: IdleStop::Unsupported,
            ..HETZNER
        };
        assert!(RUNPOD.supports(&profile) && HETZNER.supports(&profile) && unsupported.supports(&profile));
        profile.idle_stop_minutes = Some(30);
        assert_eq!(
            (RUNPOD.idle_stop, HETZNER.idle_stop),
            (IdleStop::Worker, IdleStop::Horizon)
        );
        assert!(RUNPOD.supports(&profile) && HETZNER.supports(&profile));
        assert!(!unsupported.supports(&profile));
    }
}
