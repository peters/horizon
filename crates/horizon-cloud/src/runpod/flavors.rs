//! `RunPod` CPU pod flavors. Memory and the container disk limit scale with the
//! vCPU count; neither can be requested separately. Existing pods cannot change
//! vCPU or flavor, so sizing applies only before a worker is requested.
//! Limits follow `RunPod`'s CPU flavor catalog and v2 pod rules (September 2026).
use crate::{CloudError, Profile};

pub struct Flavor {
    pub id: &'static str,
    pub memory_per_vcpu: u16,
    pub disk_per_vcpu: u16,
    pub family: &'static str,
}
/// Least memory per vCPU first, then least price.
pub const FLAVORS: [Flavor; 6] = [
    flavor("cpu3c", 2, 10, "compute-optimized"),
    flavor("cpu5c", 2, 15, "compute-optimized"),
    flavor("cpu3g", 4, 10, "general purpose"),
    flavor("cpu5g", 4, 15, "general purpose"),
    flavor("cpu3m", 8, 10, "memory-optimized"),
    flavor("cpu5m", 8, 15, "memory-optimized"),
];
/// `RunPod` requires a power of two from 2 to 32.
pub const VCPU_COUNTS: [u16; 5] = [2, 4, 8, 16, 32];

const fn flavor(id: &'static str, memory_per_vcpu: u16, disk_per_vcpu: u16, family: &'static str) -> Flavor {
    Flavor {
        id,
        memory_per_vcpu,
        disk_per_vcpu,
        family,
    }
}
impl Flavor {
    #[must_use]
    pub fn get(id: &str) -> Option<&'static Self> {
        FLAVORS.iter().find(|flavor| flavor.id == id)
    }
    /// The provider's name for this flavor at `cpu` vCPUs, such as `cpu3g-8-32`.
    #[must_use]
    pub fn instance_id(&self, cpu: u16) -> String {
        format!("{}-{cpu}-{}", self.id, u32::from(cpu) * u32::from(self.memory_per_vcpu))
    }
    /// Offers `cpu` vCPUs with at least `memory_gb` memory and `container_gb` container disk.
    #[must_use]
    pub fn fits(&self, cpu: u16, memory_gb: u16, container_gb: u16) -> bool {
        VCPU_COUNTS.contains(&cpu)
            && u32::from(cpu) * u32::from(self.memory_per_vcpu) >= u32::from(memory_gb)
            && u32::from(cpu) * u32::from(self.disk_per_vcpu) >= u32::from(container_gb)
    }
}

/// Whether `reported` is `flavor` or one of its size ids, such as `cpu3c-2-4`.
///
/// `RunPod` documents `cpuAvailability[].id` as a size id: the family, then the vCPU
/// count, then the memory in GB. Catalogs today also send the family id. Either form
/// names the same flavor. A shorter family never matches a longer one (`cpu3` is not
/// `cpu3c`), and a partial suffix such as `cpu3c-2` is not a size id.
#[must_use]
pub fn same_flavor(reported: &str, flavor: &str) -> bool {
    reported == flavor || is_size_id(flavor, reported) || is_size_id(reported, flavor)
}

/// `reported` is `{family}-{vCPU}-{memory}`, with both counts made only of digits.
fn is_size_id(family: &str, reported: &str) -> bool {
    let Some(rest) = reported.strip_prefix(family).and_then(|rest| rest.strip_prefix('-')) else {
        return false;
    };
    let Some((vcpu, memory)) = rest.split_once('-') else {
        return false;
    };
    !vcpu.is_empty()
        && !memory.is_empty()
        && vcpu.bytes().all(|byte| byte.is_ascii_digit())
        && memory.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether any flavor offers `(cpu, memory_gb)` with this container disk. Flavors
/// bound memory from above, so zero memory, which no profile may request, is refused here.
#[must_use]
pub fn offered((cpu, memory_gb): (u16, u16), container_gb: u16) -> bool {
    memory_gb > 0 && FLAVORS.iter().any(|flavor| flavor.fits(cpu, memory_gb, container_gb))
}

/// Largest container disk any supported CPU flavor can provide.
#[must_use]
pub fn max_container_gb() -> u16 {
    VCPU_COUNTS
        .into_iter()
        .flat_map(|cpu| {
            FLAVORS
                .iter()
                .map(move |flavor| cpu.saturating_mul(flavor.disk_per_vcpu))
        })
        .max()
        .unwrap_or_default()
}
/// vCPU counts that some flavor offers with this container disk.
pub fn vcpu_options(container_gb: u16) -> impl Iterator<Item = u16> {
    VCPU_COUNTS
        .into_iter()
        .filter(move |&cpu| FLAVORS.iter().any(|flavor| flavor.fits(cpu, 0, container_gb)))
}
/// Memory sizes, least first, with their flavor family, offered at `cpu` vCPUs.
#[must_use]
pub fn memory_options(cpu: u16, container_gb: u16) -> Vec<(u16, &'static str)> {
    let mut options: Vec<(u16, &'static str)> = Vec::new();
    for flavor in FLAVORS.iter().filter(|flavor| flavor.fits(cpu, 0, container_gb)) {
        let memory = cpu * flavor.memory_per_vcpu;
        if options.last().is_none_or(|&(last, _)| last != memory) {
            options.push((memory, flavor.family));
        }
    }
    options
}
/// Size after changing `(current_cpu, memory_gb)` to `cpu` vCPUs, keeping at least the
/// current memory per vCPU where offered.
#[must_use]
pub fn resize_vcpu((current_cpu, memory_gb): (u16, u16), container_gb: u16, cpu: u16) -> Option<(u16, u16)> {
    let per_vcpu = FLAVORS
        .iter()
        .map(|flavor| flavor.memory_per_vcpu)
        .find(|&ratio| u32::from(current_cpu) * u32::from(ratio) >= u32::from(memory_gb))
        .unwrap_or(8);
    let options = memory_options(cpu, container_gb);
    options
        .iter()
        .find(|(memory, _)| *memory >= cpu * per_vcpu)
        .or(options.last())
        .map(|&(memory, _)| (cpu, memory))
}
/// The profile at a chosen CPU size or GPU host minimum before its worker is requested.
/// # Errors
/// Rejects zero GPU host minimums and CPU sizes that no flavor offers with the container disk.
pub fn sized(profile: &Profile, (cpu, memory_gb): (u16, u16)) -> Result<Profile, CloudError> {
    if profile.gpu {
        if cpu == 0 || memory_gb == 0 {
            return Err(CloudError::Invalid("GPU host CPU and memory minimums must be positive"));
        }
    } else if !offered((cpu, memory_gb), profile.storage.container_gb) {
        return Err(CloudError::Invalid(
            "RunPod offers no CPU worker with this size and container disk",
        ));
    }
    Ok(Profile {
        cpu,
        memory_gb,
        ..profile.clone()
    })
}

/// Preferred flavors that offer the profile. When none do, the flavor with the
/// least memory per vCPU and price that does.
/// # Errors
/// Rejects sizes that no flavor offers.
pub fn for_profile(profile: &Profile, preferred: &[String]) -> Result<Vec<String>, CloudError> {
    for_size(
        (profile.cpu, profile.memory_gb),
        profile.storage.container_gb,
        preferred,
    )
}

/// The flavors a worker of `(cpu, memory_gb)` with this container disk requests, as
/// [`for_profile`] chooses them; the provider allocates one of them.
/// # Errors
/// Rejects sizes that no flavor offers.
pub fn for_size(
    (cpu, memory_gb): (u16, u16),
    container_gb: u16,
    preferred: &[String],
) -> Result<Vec<String>, CloudError> {
    let fits = |flavor: &Flavor| flavor.fits(cpu, memory_gb, container_gb);
    if !VCPU_COUNTS.contains(&cpu) {
        return Err(CloudError::Invalid("RunPod CPU workers need 2, 4, 8, 16 or 32 vCPU"));
    }
    let selected: Vec<String> = preferred
        .iter()
        .filter(|id| Flavor::get(id).is_some_and(fits))
        .cloned()
        .collect();
    if !selected.is_empty() {
        return Ok(selected);
    }
    FLAVORS
        .iter()
        .find(|flavor| fits(flavor))
        .map(|flavor| vec![flavor.id.to_owned()])
        .ok_or(CloudError::Invalid(
            "No RunPod CPU flavor offers this memory and container disk at this vCPU count",
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile(cpu: u16, memory_gb: u16, container_gb: u16) -> Profile {
        let mut profile = crate::CloudConfig::parse(crate::EXAMPLE).unwrap().profiles["image-only"].clone();
        profile.cpu = cpu;
        profile.memory_gb = memory_gb;
        profile.storage.container_gb = container_gb;
        profile
    }
    #[test]
    fn preferred_flavors_are_kept_when_they_offer_the_profile() {
        let preferred = ["cpu5c".to_owned(), "cpu5g".to_owned(), "unknown".to_owned()];
        assert_eq!(for_profile(&profile(4, 8, 20), &preferred).unwrap(), ["cpu5c", "cpu5g"]);
        assert_eq!(for_profile(&profile(8, 32, 30), &preferred).unwrap(), ["cpu5g"]);
    }
    #[test]
    fn unsuitable_preferences_fall_back_to_the_least_sufficient_flavor() {
        let preferred = ["cpu3c".to_owned()];
        assert_eq!(for_profile(&profile(8, 32, 30), &preferred).unwrap(), ["cpu3g"]);
        assert_eq!(for_profile(&profile(8, 33, 30), &preferred).unwrap(), ["cpu3m"]);
        // A cpu3 pod allows 10 GB of container disk per vCPU, cpu5 allows 15 GB.
        assert_eq!(for_profile(&profile(2, 4, 30), &preferred).unwrap(), ["cpu5c"]);
        assert!(for_profile(&profile(2, 4, 31), &preferred).is_err());
        assert!(for_profile(&profile(8, 65, 30), &preferred).is_err());
        for cpu in [1, 3, 6, 64] {
            assert!(for_profile(&profile(cpu, 1, 1), &[]).is_err());
        }
    }
    #[test]
    fn options_list_only_offered_sizes() {
        assert_eq!(max_container_gb(), 480);
        assert!(offered((32, 64), max_container_gb()));
        assert_eq!(vcpu_options(max_container_gb() + 1).count(), 0);
        assert_eq!(vcpu_options(20).collect::<Vec<_>>(), VCPU_COUNTS);
        assert_eq!(vcpu_options(50).collect::<Vec<_>>(), [4, 8, 16, 32]);
        assert_eq!(vcpu_options(481).count(), 0);
        let memory = |cpu, disk| {
            memory_options(cpu, disk)
                .into_iter()
                .map(|(gb, _)| gb)
                .collect::<Vec<_>>()
        };
        assert_eq!(memory(8, 30), [16, 32, 64]);
        assert_eq!(memory(2, 30), [4, 8, 16]);
        assert!(memory(1, 1).is_empty());
        assert_eq!(resize_vcpu((8, 32), 30, 16), Some((16, 64)));
        assert_eq!(resize_vcpu((8, 32), 30, 2), Some((2, 8)));
        assert_eq!(resize_vcpu((8, 32), 50, 2), None);
        assert!(offered((8, 32), 30) && !offered((1, 2), 10));
    }
    #[test]
    fn sizing_applies_only_offered_cpu_sizes() {
        let base = profile(4, 8, 20);
        let resized = sized(&base, (16, 64)).unwrap();
        assert_eq!((resized.cpu, resized.memory_gb), (16, 64));
        assert_eq!(resized.storage, base.storage, "only vCPU and memory change");
        assert_eq!(resized.image, base.image);
        assert!(sized(&base, (3, 8)).is_err(), "RunPod needs a power of two vCPU");
        assert!(sized(&base, (4, 64)).is_err(), "no flavor offers 16 GB per vCPU");
        assert!(sized(&base, (2, 0)).is_err(), "a profile needs memory");
        assert!(!offered((2, 0), 20));
        assert!(
            sized(&profile(2, 4, 31), (2, 4)).is_err(),
            "container disk limits small workers"
        );
        let gpu = Profile { gpu: true, ..base };
        let resized = sized(&gpu, (3, 17)).unwrap();
        assert_eq!((resized.cpu, resized.memory_gb), (3, 17));
        assert_eq!(resized.storage, gpu.storage);
        assert!(resized.gpu);
        assert!(sized(&gpu, (0, 8)).is_err());
        assert!(sized(&gpu, (4, 0)).is_err());
    }
    #[test]
    fn instance_ids_name_the_memory_the_flavor_assigns() {
        assert_eq!(Flavor::get("cpu3g").unwrap().instance_id(8), "cpu3g-8-32");
        assert_eq!(Flavor::get("cpu3c").unwrap().instance_id(2), "cpu3c-2-4");
        assert_eq!(Flavor::get("cpu5m").unwrap().instance_id(32), "cpu5m-32-256");
    }
    #[test]
    fn a_size_id_names_its_family_and_a_shorter_prefix_does_not() {
        assert!(same_flavor("cpu3c", "cpu3c"));
        assert!(same_flavor("cpu3c-2-4", "cpu3c"));
        assert!(same_flavor("cpu3c", "cpu3c-2-4"));
        assert!(!same_flavor("cpu3", "cpu3c"));
        assert!(!same_flavor("cpu3c", "cpu5c"));
        assert!(!same_flavor("cpu3c-extra", "cpu3c"));
        assert!(!same_flavor("cpu3cx-2-4", "cpu3c"));
        assert!(same_flavor("cpu3c-16-32", "cpu3c"));
        assert!(!same_flavor("cpu3c-2", "cpu3c"));
        assert!(!same_flavor("cpu3c-2garbage", "cpu3c"));
        assert!(!same_flavor("cpu3c-2-", "cpu3c"));
        assert!(!same_flavor("cpu3c-2-4extra", "cpu3c"));
    }
}
