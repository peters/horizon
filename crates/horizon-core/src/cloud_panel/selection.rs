//! A catalog choice translated to the same deployment profile and persisted placement.
use super::Placement;
use crate::cloud_runtime::{Error, Result, provider};
use horizon_cloud::{
    Profile,
    offers::{Offer, Requirements},
};
use serde::{Deserialize, Serialize};

/// The allocation identity carried by an offer, independent of its changing price.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WorkerChoice {
    pub provider: String,
    pub id: String,
    pub kind: String,
    pub vcpu: Option<u16>,
    pub memory_gb: Option<u16>,
    pub gpu_memory_gb: Option<u16>,
    pub location: Option<String>,
}

pub struct ChosenWorker {
    pub provider: &'static provider::Description,
    pub size: Option<(u16, u16)>,
    pub placement: Placement,
}

impl From<&Offer> for WorkerChoice {
    fn from(offer: &Offer) -> Self {
        Self {
            provider: offer.provider.into(),
            id: offer.id.clone(),
            kind: offer.kind.into(),
            vcpu: offer.vcpu,
            memory_gb: offer.memory_gb,
            gpu_memory_gb: offer.gpu_memory_gb,
            location: offer.location.clone(),
        }
    }
}

impl WorkerChoice {
    /// Bind the chosen worker, preserving the repository's minimum requirements.
    /// # Errors
    /// Rejects unsupported providers, mismatched worker kinds and insufficient resources.
    pub fn for_profile(&self, profile: &Profile) -> Result<ChosenWorker> {
        let provider = provider::ALL
            .into_iter()
            .find(|provider| provider.id == self.provider || provider.label == self.provider)
            .ok_or(Error::Invalid("Unknown worker provider"))?;
        if !provider.supports(profile)
            || !provider.creatable
            || self.id.is_empty()
            || self.kind != if profile.gpu { "gpu" } else { "cpu" }
        {
            return Err(Error::Invalid("This worker cannot run the repository profile"));
        }
        let requirements = Requirements::for_profile(profile);
        if requirements
            .min_vcpu
            .is_some_and(|min| self.vcpu.is_none_or(|value| value < min))
            || requirements
                .min_memory_gb
                .is_some_and(|min| self.memory_gb.is_none_or(|value| value < min))
            || requirements
                .min_gpu_memory_gb
                .is_some_and(|min| self.gpu_memory_gb.is_none_or(|value| value < min))
        {
            return Err(Error::Invalid(
                "This worker is below the profile's minimum requirements",
            ));
        }
        let size = self.vcpu.zip(self.memory_gb);
        let mut placement = Placement::default();
        match provider.kind {
            provider::Kind::RunPod if profile.gpu => placement.gpu_types = vec![self.id.clone()],
            provider::Kind::RunPod => {
                let (cpu, memory) = size.ok_or(Error::Invalid("CPU worker has no size"))?;
                if self.id != format!("cpu-{cpu}-{memory}") {
                    return Err(Error::Invalid("CPU worker identity does not match its size"));
                }
            }
            provider::Kind::Hetzner => {
                let location = self
                    .location
                    .as_ref()
                    .filter(|location| !location.is_empty())
                    .ok_or(Error::Invalid("Choose an exact Hetzner worker location"))?;
                placement.cpu_types = vec![self.id.clone()];
                placement.data_centers = vec![location.clone()];
            }
        }
        Ok(ChosenWorker {
            provider,
            size,
            placement,
        })
    }
}
