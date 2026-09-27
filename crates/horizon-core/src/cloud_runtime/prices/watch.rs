//! An explicit, fixed size and location whose fresh stock can authorize one launch.
use super::{Availability, PriceList, Profile, SizeAvailability};
use crate::cloud_panel::Placement;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub profile: Profile,
    pub placement: Placement,
}

impl Selection {
    /// A watch never broadens a person's chosen location or GPU type.
    /// # Errors
    /// Requires a supported provider and one explicit data center and GPU type.
    pub fn new(profile: Profile, placement: Placement) -> Result<Self, &'static str> {
        if profile.provider != "runpod" {
            return Err("Stock watching is available for RunPod clouds.");
        }
        if placement.data_centers.len() != 1 {
            return Err("Choose one data center under Advanced to watch its stock.");
        }
        if profile.gpu && placement.gpu_types.len() != 1 {
            return Err("Choose one GPU type to watch its stock.");
        }
        Ok(Self { profile, placement })
    }

    /// Both inputs must be current; CPU stock must have been fetched for this profile.
    /// Missing or excluded catalog entries cannot authorize an automatic launch.
    #[must_use]
    pub fn available(&self, list: &PriceList, cpu: Option<&SizeAvailability>) -> bool {
        let Some(center) = list
            .data_centers
            .iter()
            .find(|center| self.placement.data_centers.as_slice() == std::slice::from_ref(&center.id))
        else {
            return false;
        };
        if self.profile.gpu {
            self.placement.gpu_types.first().is_some_and(|gpu| {
                list.gpus.iter().any(|price| price.id == *gpu)
                    && list.gpu_availability(gpu, &self.placement.data_centers) != Availability::None
            })
        } else {
            center.workspace_storage
                && cpu.is_some_and(|size| size.best(&self.placement.data_centers) != Availability::None)
        }
    }
}

#[cfg(test)]
mod tests;
