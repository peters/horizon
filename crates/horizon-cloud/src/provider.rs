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
    /// Docker engines the person saved, each with its own free capacity.
    Hosts,
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
    /// Per server type and location; Hetzner caps it per month.
    ServerTypes,
    /// Nothing: the person's own machine runs the worker.
    NoRent,
}

/// A choice a provider supports beyond size and placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// A GPU type chosen for one cloud from those in stock.
    GpuType,
    /// Third-party hosts, which a person must opt into. No provider offers it yet:
    /// clouds use Secure Cloud hosts only.
    CommunityHosts,
    /// More than one kind of network volume storage for CPU workers.
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
    /// changed on provider invoices; comparisons may use reference rates.
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
    /// The provider's site, as a card names it, such as `runpod.io`.
    pub site: &'static str,
    /// Whether the provider pulls private images with a registry auth it stores
    /// (`registry_pull_auth_id`); otherwise the host logs in to the registry itself.
    pub registry_auth: bool,
    /// How a ready worker moves onto a rebuilt image.
    pub rebuild: Rebuild,
    /// Whether the provider reads the VM sizes and regions it may use from a settings
    /// section on this machine, which no credential form makes. A dialog lists such a
    /// provider even when the section is missing, so that it can say what to add; a
    /// provider with a credential form is listed only once this machine has one.
    pub needs_settings: bool,
    /// What a worker's hourly rate here covers, and what it leaves out, so a card states
    /// it without naming the provider.
    pub rate_covers: &'static str,
    /// Why a chosen worker needs an exact location, for a provider that prices one server
    /// type in one location. Empty for a provider that places a worker another way, which
    /// never reads it.
    pub exact_placement_refusal: &'static str,
    /// The files a cloud's record keeps this provider's own resources in, from the one a
    /// delete proves to any marker beside it. A file here that Horizon finds without its
    /// deployment may still name resources at the provider.
    pub journals: &'static [&'static str],
}

/// How a provider moves a ready worker onto a rebuilt image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rebuild {
    /// The provider switches the same worker to the new image and reports which image
    /// it runs, so the switch can be watched and undone.
    InPlace,
    /// The provider cannot report a server's image, so the server is released and a
    /// new one runs the new image on the same workspace volume. A Hetzner stop already
    /// deletes the server, so its release is that stop; an Azure stop keeps the VM
    /// (`StoppedCost::WorkerKept`), so its release deletes the VM and keeps the disk.
    NewServer,
    /// Not offered: a cloud there keeps the image it was created with.
    Unsupported,
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
    registry_auth: true,
    rebuild: Rebuild::InPlace,
    needs_settings: false,
    site: "runpod.io",
    cpu_volume_gb: (
        *crate::runpod::volumes::REQUEST_SIZE_GB.start(),
        *crate::runpod::volumes::REQUEST_SIZE_GB.end(),
    ),
    rate_covers: "Last reported worker hourly rate. Storage and other provider charges may be additional.",
    exact_placement_refusal: "",
    journals: &["workspace-volume.json", "workspace-volume.required"],
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
    registry_auth: false,
    rebuild: Rebuild::NewServer,
    needs_settings: false,
    site: "hetzner.com",
    cpu_volume_gb: (
        *crate::hetzner::volumes::SIZE_GB.start(),
        *crate::hetzner::volumes::SIZE_GB.end(),
    ),
    rate_covers: "Hetzner's hourly price for the server type, in euros net of VAT: compute only. Not included: the \
             workspace volume, which a stop keeps, the primary IPv4 address, traffic over the included amount and \
             VAT. Hetzner bills each started hour, up to a monthly cap, so the run estimate can trail its bill by up \
             to an hour.",
    exact_placement_refusal: "Choose an exact Hetzner worker location",
    journals: &["hetzner.json"],
};

/// Microsoft Azure: one VM per cloud in its own resource group. A stop deallocates the
/// virtual machine and keeps its disks and static address. A rebuild deletes the VM,
/// keeps its workspace disk and static address, and starts a new VM on them.
pub const AZURE: Description = Description {
    kind: Kind::Azure,
    id: "azure",
    label: "Azure",
    currency: "USD",
    net_of_vat: false,
    gpu: false,
    placement: Placement::Locations,
    pricing: Pricing::ServerTypes,
    stopped: StoppedCost::WorkerKept,
    choices: &[],
    creatable: crate::offers::AZURE_DEPLOYABLE,
    registry_auth: false,
    rebuild: Rebuild::NewServer,
    needs_settings: true,
    site: "azure.microsoft.com",
    cpu_volume_gb: (4, 32_767),
    rate_covers: "Azure reports no hourly rate to Horizon, so none is shown. A running worker bills, in US dollars \
             without VAT, for its VM size, its OS and workspace disks and its Standard static IPv4 address. A stop \
             deallocates the VM, which ends the compute charge; the disks and the address bill until the cloud is \
             deleted.",
    exact_placement_refusal: "Choose an exact Azure VM size and region",
    journals: &["azure.json"],
};

/// A Docker engine the person already runs: this computer's, or one reached over SSH
/// or Tailscale SSH. Nothing is rented, and a stop keeps the container and its volume.
pub const DOCKER_HOST: Description = Description {
    kind: Kind::DockerHost,
    id: crate::docker_host::PROVIDER,
    label: "Docker host",
    currency: "USD",
    net_of_vat: false,
    gpu: false,
    placement: Placement::Hosts,
    pricing: Pricing::NoRent,
    stopped: StoppedCost::WorkerKept,
    choices: &[],
    creatable: true,
    registry_auth: false,
    rebuild: Rebuild::Unsupported,
    needs_settings: false,
    site: "your own machine",
    // A Docker volume has no size of its own; it shares the host's disk.
    cpu_volume_gb: (1, 65_535),
    rate_covers: "No rent: the worker runs on a Docker host you already have.",
    exact_placement_refusal: "",
    journals: &["docker-host.json"],
};

/// Which provider a description is, for code that must handle every provider: a
/// match on it is exhaustive, so adding a provider fails to compile until each such
/// place handles it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    RunPod,
    Hetzner,
    DockerHost,
    Azure,
}

impl Kind {
    /// The description of this provider. This is the one match on `Kind` in Horizon that
    /// only names providers, so a new provider is added here and nowhere else.
    #[must_use]
    pub const fn description(self) -> &'static Description {
        match self {
            Self::RunPod => &RUNPOD,
            Self::Hetzner => &HETZNER,
            Self::DockerHost => &DOCKER_HOST,
            Self::Azure => &AZURE,
        }
    }
}

/// Every provider journal file, so a caller that must reject a foreign provider's record
/// never keeps its own list.
pub fn journals() -> impl Iterator<Item = &'static str> {
    ALL.iter().flat_map(|provider| provider.journals.iter().copied())
}

/// Every provider a cloud can run on, in the order they are offered.
pub const ALL: [&Description; 4] = [&RUNPOD, &HETZNER, &DOCKER_HOST, &AZURE];

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
        candidate.validate(false).is_ok() && (profile.gpu || (smallest..=largest).contains(&volume))
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
    fn only_a_provider_that_reads_its_own_settings_section_is_listed_without_a_credential() {
        // Azure's VM sizes and regions come from the `azure` settings section, which no key
        // form makes, so a dialog lists it and says what to add. Every other provider is
        // listed once this machine has its credential or its saved host.
        const {
            assert!(
                AZURE.needs_settings,
                "Azure reads its VM sizes and regions from settings"
            );
        }
        assert!(
            [&RUNPOD, &HETZNER, &DOCKER_HOST]
                .iter()
                .all(|provider| !provider.needs_settings)
        );
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
        for id in [
            "runpod",
            crate::hetzner::PROVIDER,
            crate::docker_host::PROVIDER,
            AZURE.id,
        ] {
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
        profile.provider = "azure".into();
        assert_eq!(Description::of(&profile), &AZURE);
        assert_ne!(Description::of(&profile), &RUNPOD);
        // A provider Horizon does not know reads as RunPod, as profiles were before providers.
        profile.provider = "elsewhere".into();
        assert_eq!(Description::of(&profile), &RUNPOD);
    }

    #[test]
    fn an_idle_period_has_no_effect_so_every_provider_offers_its_profile() {
        let mut profile = profile(false);
        profile.idle_stop_minutes = Some(30);
        assert!(RUNPOD.supports(&profile) && HETZNER.supports(&profile) && DOCKER_HOST.supports(&profile));
    }

    #[test]
    fn every_provider_kind_names_its_own_description() {
        for provider in ALL {
            assert_eq!(provider.kind.description(), provider, "{}", provider.id);
            assert_eq!(by_id(provider.id), Some(provider), "{}", provider.id);
        }
    }

    #[test]
    fn only_a_provider_priced_by_server_type_in_a_location_refuses_a_missing_location() {
        // `ChosenWorker::for_profile` reads this string only for that shape, so it must be
        // present exactly there and empty everywhere else.
        for provider in ALL {
            let exact = (provider.placement, provider.pricing) == (Placement::Locations, Pricing::ServerTypes);
            assert_eq!(
                !provider.exact_placement_refusal.is_empty(),
                exact,
                "{} says {:?}",
                provider.id,
                provider.exact_placement_refusal
            );
        }
        assert_eq!(
            HETZNER.exact_placement_refusal,
            "Choose an exact Hetzner worker location"
        );
        assert_eq!(
            AZURE.exact_placement_refusal,
            "Choose an exact Azure VM size and region"
        );
    }

    #[test]
    fn each_provider_keeps_its_own_journal_files() {
        let all: Vec<&str> = journals().collect();
        assert!(!all.is_empty());
        for provider in ALL {
            assert!(!provider.journals.is_empty(), "{} keeps no journal", provider.id);
            for file in provider.journals {
                assert_eq!(
                    all.iter().filter(|other| *other == file).count(),
                    1,
                    "{file} belongs to more than one provider"
                );
            }
        }
        // The pair RunPod keeps, and the one file each of the others, are all named.
        assert!(RUNPOD.journals.contains(&"workspace-volume.json"));
        assert!(RUNPOD.journals.contains(&"workspace-volume.required"));
        assert_eq!(HETZNER.journals, ["hetzner.json"]);
        assert_eq!(AZURE.journals, ["azure.json"]);
        assert_eq!(DOCKER_HOST.journals, ["docker-host.json"]);
    }

    #[test]
    fn every_provider_states_what_its_rate_covers() {
        for provider in ALL {
            assert!(
                !provider.rate_covers.is_empty(),
                "{} states no rate coverage",
                provider.id
            );
            assert_eq!(
                ALL.into_iter()
                    .filter(|other| other.rate_covers == provider.rate_covers)
                    .count(),
                1,
                "{} repeats another provider's rate coverage",
                provider.id
            );
        }
    }

    #[test]
    fn a_docker_host_offers_cpu_clouds_without_rent_or_rebuild() {
        let (cpu, gpu) = (profile(false), profile(true));
        assert_eq!(by_id("docker-host"), Some(&DOCKER_HOST));
        assert!(DOCKER_HOST.supports(&cpu) && !DOCKER_HOST.supports(&gpu));
        let mut devices = profile(false);
        devices.capabilities.browserstack = Some(crate::BrowserStack::default());
        assert!(!DOCKER_HOST.supports(&devices));
        assert_eq!(
            (DOCKER_HOST.pricing, DOCKER_HOST.rebuild, DOCKER_HOST.stopped),
            (Pricing::NoRent, Rebuild::Unsupported, StoppedCost::WorkerKept)
        );
        assert!(DOCKER_HOST.choices.is_empty() && DOCKER_HOST.creatable);
        assert_eq!(
            Description::choices(&cpu, &[&DOCKER_HOST, &RUNPOD]),
            [&RUNPOD, &DOCKER_HOST]
        );
    }

    #[test]
    fn an_azure_provider_is_described_and_creatable() {
        assert_eq!(by_id("azure"), Some(&AZURE));
        assert_eq!(AZURE.kind, Kind::Azure);
        assert_eq!(AZURE.creatable, crate::offers::AZURE_DEPLOYABLE);
        const { assert!(AZURE.creatable, "Horizon creates, stops and deletes Azure clouds") };
        // Deallocate keeps the virtual machine and its disks; a stop does not delete it.
        // So a rebuild cannot release the VM as a stop does, unlike Hetzner's, whose
        // stop deletes the server: Azure is the one provider with this pair.
        assert_eq!(
            (AZURE.stopped, AZURE.rebuild),
            (StoppedCost::WorkerKept, Rebuild::NewServer)
        );
        assert_eq!(
            (HETZNER.stopped, HETZNER.rebuild),
            (StoppedCost::ServerDeleted, Rebuild::NewServer)
        );
        // Stated over every provider, so a second one with this pair cannot land silently.
        assert_eq!(
            ALL.into_iter()
                .filter(|provider| (provider.stopped, provider.rebuild)
                    == (StoppedCost::WorkerKept, Rebuild::NewServer))
                .count(),
            1,
            "Azure alone keeps its worker on a stop and needs a new server for a rebuild; another \
             such provider must say how its release differs"
        );
        assert_eq!(
            (AZURE.placement, AZURE.pricing, AZURE.currency, AZURE.net_of_vat),
            (Placement::Locations, Pricing::ServerTypes, "USD", false)
        );
        assert!(!AZURE.gpu && AZURE.choices.is_empty() && !AZURE.registry_auth);
        let (cpu, gpu) = (profile(false), profile(true));
        assert!(AZURE.supports(&cpu) && !AZURE.supports(&gpu));
        // Offered for comparison only: callers refuse to create on it.
        assert_eq!(Description::choices(&cpu, &[&AZURE, &RUNPOD]), [&RUNPOD, &AZURE]);
    }
}
