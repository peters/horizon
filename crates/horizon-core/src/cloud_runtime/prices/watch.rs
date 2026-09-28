//! An explicit, fixed size and location whose fresh stock can authorize one launch.
use super::{Availability, Preferences, PriceList, Profile, SizeAvailability, requested_flavors};
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
            return Err("Choose one data center to watch its stock.");
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
            center.holds(self.profile.storage.volume_tier)
                && cpu.is_some_and(|size| size.best(&self.placement.data_centers) != Availability::None)
        }
    }

    /// The most this selection is charged per hour for compute in `list`: its GPU
    /// type's price, or the dearest flavor a CPU deployment of its size would request.
    /// A watch compares it with the price shown when it was started.
    #[must_use]
    pub fn hourly(&self, list: &PriceList, preferences: &Preferences) -> Option<f64> {
        if self.profile.gpu {
            let gpu = self.placement.gpu_types.first()?;
            return list.gpu(gpu).map(|price| price.hourly);
        }
        let flavors = requested_flavors(&self.profile, preferences);
        list.cpu_hourly(&flavors, self.profile.cpu).map(|(_, high)| high)
    }
}

#[cfg(test)]
mod tests;
