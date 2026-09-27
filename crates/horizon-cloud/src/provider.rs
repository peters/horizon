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

/// What a stopped cloud keeps paying for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoppedCost {
    /// The stopped worker keeps its disk, and the workspace volume stays billed.
    WorkerAndVolume,
    /// Stopping deletes the server, so only the workspace volume stays billed.
    VolumeOnly,
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
    /// Third-party hosts, which a person must opt into.
    CommunityHosts,
    /// More than one kind of network volume storage.
    VolumeTiers,
    /// A region picker with live stock.
    Region,
    /// Machine types tried in order when the first is sold out.
    ServerTypeFallback,
}

/// One provider as a new cloud sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Description {
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
    /// comparison only.
    pub creatable: bool,
}

pub const RUNPOD: Description = Description {
    id: "runpod",
    label: "RunPod",
    currency: "USD",
    net_of_vat: false,
    gpu: true,
    placement: Placement::DataCenters,
    pricing: Pricing::Flavors,
    stopped: StoppedCost::WorkerAndVolume,
    choices: &[
        Choice::GpuType,
        Choice::CommunityHosts,
        Choice::VolumeTiers,
        Choice::Region,
    ],
    creatable: true,
};

pub const HETZNER: Description = Description {
    id: crate::hetzner::PROVIDER,
    label: "Hetzner",
    currency: "EUR",
    net_of_vat: true,
    gpu: false,
    placement: Placement::Locations,
    pricing: Pricing::ServerTypes,
    stopped: StoppedCost::VolumeOnly,
    choices: &[Choice::ServerTypeFallback],
    creatable: crate::offers::HETZNER_DEPLOYABLE,
};

/// Every provider a cloud can run on, in the order they are offered.
pub const ALL: [&Description; 2] = [&RUNPOD, &HETZNER];

/// The provider a profile names.
#[must_use]
pub fn by_id(id: &str) -> Option<&'static Description> {
    ALL.into_iter().find(|provider| provider.id == id)
}

impl Description {
    /// Whether this provider supports `choice`.
    #[must_use]
    pub fn offers(&self, choice: Choice) -> bool {
        self.choices.contains(&choice)
    }

    /// Whether a cloud of `profile` can run on this provider.
    #[must_use]
    pub fn supports(&self, profile: &Profile) -> bool {
        !profile.gpu || self.gpu
    }

    /// The providers among `configured` a cloud of `profile` can choose from, in the
    /// order they are offered. A choice is worth showing only when there is more than one.
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
        assert_eq!(HETZNER.stopped, StoppedCost::VolumeOnly);
        assert_eq!(RUNPOD.stopped, StoppedCost::WorkerAndVolume);
        assert_eq!(
            (RUNPOD.pricing, HETZNER.pricing),
            (Pricing::Flavors, Pricing::ServerTypes)
        );
        assert_eq!(HETZNER.creatable, crate::offers::HETZNER_DEPLOYABLE);
        // Choices that are one provider's own concepts never apply to the other.
        for choice in [
            Choice::GpuType,
            Choice::CommunityHosts,
            Choice::VolumeTiers,
            Choice::Region,
        ] {
            assert!(RUNPOD.offers(choice) && !HETZNER.offers(choice), "{choice:?}");
        }
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
    fn every_provider_a_profile_accepts_is_described() {
        for id in ["runpod", crate::hetzner::PROVIDER] {
            assert!(by_id(id).is_some(), "{id}");
        }
    }
}
