//! Which profile of the checkout's `.horizon/cloud.yml` a rebuild can bind a cloud's
//! running worker to, what that changes, and whether the built image fits that worker.
//! The choices a cloud binds when it is created, its provider, a CPU cloud's vCPU and
//! memory, and its storage, stay the worker's: values for them in the file are defaults
//! for new clouds, never a request to move, resize or reformat this one. The settings a
//! rebuild may change (`Profile::with_rebuild_settings`) come from the file, the working
//! copy's when it is valid (`launch::for_rebuild`); any other difference is refused.
use super::{Error, Result};
use horizon_cloud::{AgentApprovals, Bootstrap, Capabilities, CloudConfig, Profile, ProfileError, Storage};

pub(super) const NO_CONFIG: &str = "Neither the working copy nor the latest commit has a readable .horizon/cloud.yml; add the cloud configuration to rebuild";
pub(super) const NO_PROFILE: &str = ".horizon/cloud.yml no longer defines this cloud's profile";
const GPU: &str = "This cloud's profile in .horizon/cloud.yml differs from its worker in gpu. A running worker cannot add or remove a GPU; create a new cloud to use it.";
const GPU_HOST: &str = "This cloud's profile in .horizon/cloud.yml differs from its GPU worker in cpu or memory_gb. A running worker cannot be resized; create a new cloud to use them.";
const GPU_REQUIREMENTS: &str = "This cloud's profile in .horizon/cloud.yml differs from its worker in min_cuda_version or min_gpu_memory_gb. A rebuild keeps the worker's GPU host; create a new cloud to use them.";
const IMAGE: &str = "This cloud's profile in .horizon/cloud.yml differs from its worker in image. A rebuild pushes to the worker's image repository; create a new cloud to use another one.";
const HOSTED_DEVICES: &str = "This cloud's profile in .horizon/cloud.yml differs from its worker in capabilities.browserstack. Hosted devices are fixed when the worker is created; create a new cloud to change them.";
const CONTRACT: &str = "This cloud's profile in .horizon/cloud.yml differs from its worker in bootstrap.contract_version. A rebuild keeps the worker's bootstrap contract; create a new cloud to use another one.";
const NO_BUILD: &str = "This cloud's profile in .horizon/cloud.yml has no build section, so there is no recipe to rebuild this cloud's image from";
const INVALID: &str = "This cloud's profile in .horizon/cloud.yml is not valid";
pub(super) const IMAGE_TOO_LARGE: &str = "The rebuilt image is larger than this cloud's container disk (storage.container_gb), so its worker cannot hold it. A rebuild keeps the worker's container disk; make the image smaller, or create a new cloud with a larger container disk.";
pub(super) const BUILT_IMAGE_TOO_LARGE: &str = "The built image is larger than this cloud's container disk (storage.container_gb), so its worker cannot hold it. Cancel the rebuild; then make the image smaller, or create a new cloud with a larger container disk.";
pub(super) const BYTES_PER_GB: u64 = 1_000_000_000;

/// The profile the cloud binds once a rebuild to the profile `name` of `config`, the
/// settings a rebuild reads, commits: the `bound` profile with its rebuild settings.
/// Refuses, naming the field, any other difference but the choices bound at creation,
/// and a combination of settings that no profile may have.
pub(super) fn next_profile(bound: &Profile, config: Option<&CloudConfig>, name: &str) -> Result<Profile> {
    let committed = config
        .ok_or(Error::Invalid(NO_CONFIG))?
        .profiles
        .get(name)
        .ok_or(Error::Invalid(NO_PROFILE))?;
    if let Some(reason) = refusal(bound, committed) {
        return Err(Error::Invalid(reason));
    }
    let next = bound.with_rebuild_settings(committed);
    next.validate(false).map_err(|error| {
        Error::Invalid(match error {
            ProfileError::Invalid(reason) => reason,
            ProfileError::Yaml => INVALID,
        })
    })?;
    Ok(next)
}

/// The first field in which `committed` differs from `bound` that a rebuild cannot take.
fn refusal(bound: &Profile, committed: &Profile) -> Option<&'static str> {
    // Destructured in full, so a new profile field is compared, named a rebuild setting
    // or named a creation choice.
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
        bootstrap: Bootstrap {
            readiness_seconds: _,
            contract_version,
        },
        capabilities:
            Capabilities {
                agents: _,
                browsers: _,
                desktop: _,
                browserstack,
                agent_approvals: _,
            },
        idle_stop_minutes: _,
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
        (*image != bound.image, IMAGE),
        (*browserstack != bound.capabilities.browserstack, HOSTED_DEVICES),
        // Version 1 is the only contract a profile may name today.
        (*contract_version != bound.bootstrap.contract_version, CONTRACT),
        (build.is_none(), NO_BUILD),
    ]
    .into_iter()
    .find_map(|(differs, reason)| differs.then_some(reason))
}

/// What binding `next` instead of `bound` changes, one plain sentence per change.
pub(super) fn changes(bound: &Profile, next: &Profile) -> Vec<String> {
    let (before, after) = (&bound.capabilities, &next.capabilities);
    let mut changes = Vec::new();
    let mut members = |kind: &str, before: Vec<&'static str>, after: Vec<&'static str>| {
        let added = after.iter().filter(|name| !before.contains(name));
        let removed = before.iter().filter(|name| !after.contains(name));
        changes.extend(added.map(|name| format!("Adds {kind} {name}")));
        changes.extend(removed.map(|name| format!("Removes {kind} {name}")));
    };
    members(
        "agent",
        before.agents.iter().map(|agent| agent.as_str()).collect(),
        after.agents.iter().map(|agent| agent.as_str()).collect(),
    );
    members(
        "browser",
        before.browsers.iter().map(|browser| browser.as_str()).collect(),
        after.browsers.iter().map(|browser| browser.as_str()).collect(),
    );
    if before.desktop != after.desktop {
        changes.push(
            if after.desktop {
                "Adds the desktop"
            } else {
                "Removes the desktop"
            }
            .into(),
        );
    }
    let approvals = |approvals: AgentApprovals| if approvals.is_auto() { "auto" } else { "ask" };
    let minutes = |minutes: Option<u16>| minutes.map_or_else(|| "off".to_owned(), |minutes| format!("{minutes} min"));
    let mut differs = |what: &str, before: String, after: String| {
        if before != after {
            changes.push(format!("{what} {before} → {after}"));
        }
    };
    differs(
        "Agent approvals",
        approvals(before.agent_approvals).into(),
        approvals(after.agent_approvals).into(),
    );
    differs(
        "Readiness timeout",
        format!("{} s", bound.bootstrap.readiness_seconds),
        format!("{} s", next.bootstrap.readiness_seconds),
    );
    differs(
        "Idle stop",
        minutes(bound.idle_stop_minutes),
        minutes(next.idle_stop_minutes),
    );
    if let (Some(before), Some(after)) = (&bound.build, &next.build) {
        differs("Build context", before.context.clone(), after.context.clone());
        differs("Build dockerfile", before.dockerfile.clone(), after.dockerfile.clone());
        differs("Build platform", before.platform.clone(), after.platform.clone());
    }
    changes
}

/// Whether the `bound` worker's container disk holds an image of `unpacked_bytes`. The
/// disk is the one the worker's profile guarantees: `RunPod`'s container disk, or the
/// least system disk of a Hetzner server type. Sizes are decimal, as Docker shows them.
pub(super) fn holds_image(bound: &Profile, unpacked_bytes: u64) -> bool {
    unpacked_bytes <= u64::from(bound.storage.container_gb) * BYTES_PER_GB
}
