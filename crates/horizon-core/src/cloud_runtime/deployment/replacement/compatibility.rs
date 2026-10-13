//! Whether the committed profile still describes a cloud's running worker, so a rebuild
//! can build its recipe for that worker, and whether the built image fits that worker.
//! The choices a cloud binds when it is created, its provider, a CPU cloud's vCPU and
//! memory, and its storage, stay the worker's: committed values for them are defaults
//! for new clouds, never a request to move, resize or reformat this one.
use super::{Error, Result};
use horizon_cloud::{CloudConfig, Profile, Storage};

pub(super) const NO_CONFIG: &str =
    "The latest commit has no readable .horizon/cloud.yml; commit the cloud configuration to rebuild";
pub(super) const NO_PROFILE: &str = "The latest commit's .horizon/cloud.yml no longer defines this cloud's profile";
const GPU: &str = "This cloud's profile in the latest commit's .horizon/cloud.yml differs from its worker in gpu. A running worker cannot add or remove a GPU; create a new cloud to use it.";
const GPU_HOST: &str = "This cloud's profile in the latest commit's .horizon/cloud.yml differs from its GPU worker in cpu or memory_gb. A running worker cannot be resized; create a new cloud to use them.";
const GPU_REQUIREMENTS: &str = "This cloud's profile in the latest commit's .horizon/cloud.yml differs from its worker in min_cuda_version or min_gpu_memory_gb. A rebuild keeps the worker's GPU host; create a new cloud to use them.";
const CAPABILITIES: &str = "This cloud's profile in the latest commit's .horizon/cloud.yml differs from its worker in capabilities. They are fixed when the worker is created; create a new cloud to use them.";
const IMAGE: &str = "This cloud's profile in the latest commit's .horizon/cloud.yml differs from its worker in image. A rebuild pushes to the worker's image repository; create a new cloud to use another one.";
const BUILD: &str = "This cloud's profile in the latest commit's .horizon/cloud.yml differs from its worker in its build section (context, dockerfile or platform). Create a new cloud to use it.";
const BOOTSTRAP: &str = "This cloud's profile in the latest commit's .horizon/cloud.yml differs from its worker in bootstrap (contract_version or readiness_seconds). Create a new cloud to use it.";
const IDLE_STOP: &str = "This cloud's profile in the latest commit's .horizon/cloud.yml differs from its worker in idle_stop_minutes. A rebuild keeps the worker's idle stop; create a new cloud to use it.";
pub(super) const IMAGE_TOO_LARGE: &str = "The rebuilt image is larger than this cloud's container disk (storage.container_gb), so its worker cannot hold it. A rebuild keeps the worker's container disk; make the image smaller, or create a new cloud with a larger container disk.";
pub(super) const BYTES_PER_GB: u64 = 1_000_000_000;

/// Refuses, naming the field, when the profile `name` of the latest commit's `config`
/// differs from the `bound` one in anything but the choices bound at creation.
pub(super) fn unchanged(bound: &Profile, config: Option<&CloudConfig>, name: &str) -> Result<()> {
    let committed = config
        .ok_or(Error::Invalid(NO_CONFIG))?
        .profiles
        .get(name)
        .ok_or(Error::Invalid(NO_PROFILE))?;
    difference(bound, committed).map_or(Ok(()), |reason| Err(Error::Invalid(reason)))
}

/// The first field in which `committed` differs from `bound`, other than a creation choice.
fn difference(bound: &Profile, committed: &Profile) -> Option<&'static str> {
    // Destructured in full, so a new profile field is either compared or named a creation choice.
    let Profile {
        provider: _,
        image,
        cpu,
        memory_gb,
        gpu,
        build,
        // New cloud chooses each of them; a workspace can also grow after creation.
        storage: Storage {
            container_gb: _,
            volume_gb: _,
            volume_tier: _,
        },
        bootstrap,
        capabilities,
        idle_stop_minutes,
        min_cuda_version,
        min_gpu_memory_gb,
    } = committed;
    [
        (*gpu != bound.gpu, GPU),
        // A GPU profile's vCPU and memory are the minimums its worker's host was chosen for.
        (
            bound.gpu && (*cpu, *memory_gb) != (bound.cpu, bound.memory_gb),
            GPU_HOST,
        ),
        (
            (min_cuda_version, min_gpu_memory_gb) != (&bound.min_cuda_version, &bound.min_gpu_memory_gb),
            GPU_REQUIREMENTS,
        ),
        (*capabilities != bound.capabilities, CAPABILITIES),
        (*image != bound.image, IMAGE),
        (*build != bound.build, BUILD),
        (*bootstrap != bound.bootstrap, BOOTSTRAP),
        (*idle_stop_minutes != bound.idle_stop_minutes, IDLE_STOP),
    ]
    .into_iter()
    .find_map(|(differs, reason)| differs.then_some(reason))
}

/// Whether the `bound` worker's container disk holds an image of `unpacked_bytes`. The
/// disk is the one the worker's profile guarantees: `RunPod`'s container disk, or the
/// least system disk of a Hetzner server type. Sizes are decimal, as Docker shows them.
pub(super) fn holds_image(bound: &Profile, unpacked_bytes: u64) -> bool {
    unpacked_bytes <= u64::from(bound.storage.container_gb) * BYTES_PER_GB
}
