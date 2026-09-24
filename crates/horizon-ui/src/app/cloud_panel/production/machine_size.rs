//! CPU worker size choices shared by the New cloud dialog and cloud cards, so both
//! offer only the sizes `RunPod` provides and apply the same resize rules.
use horizon_core::cloud_runtime::flavors;

/// A worker's vCPU count and memory in GB.
pub(super) type Size = (u16, u16);

/// Offers each vCPU count for the container disk through `option(label, selected)`, which
/// returns whether it was chosen. Returns the size a changed choice selects.
pub(super) fn vcpu(current: Size, container_gb: u16, mut option: impl FnMut(String, bool) -> bool) -> Option<Size> {
    let mut chosen = None;
    for cpu in flavors::vcpu_options(container_gb) {
        if option(format!("{cpu} vCPU"), cpu == current.0) && cpu != current.0 {
            chosen = flavors::resize_vcpu(current, container_gb, cpu);
        }
    }
    chosen
}

/// Offers each memory size at the current vCPU count like [`vcpu`].
pub(super) fn memory(current: Size, container_gb: u16, mut option: impl FnMut(String, bool) -> bool) -> Option<Size> {
    let mut chosen = None;
    for (memory, family) in flavors::memory_options(current.0, container_gb) {
        if option(format!("{memory} GB · {family}"), memory == current.1) && memory != current.1 {
            chosen = Some((current.0, memory));
        }
    }
    chosen
}

/// Why no CPU worker can be requested at `current`, if none can.
pub(super) fn unoffered(current: Size, container_gb: u16) -> Option<String> {
    (!flavors::offered(current, container_gb))
        .then(|| format!("RunPod offers no CPU worker with this size and {container_gb} GB container disk"))
}

/// A size that cannot be chosen here, such as a GPU profile's.
pub(super) fn fixed((cpu, memory_gb): Size, gpu: bool) -> String {
    format!("{cpu} vCPU · {memory_gb} GB · {}", if gpu { "GPU" } else { "CPU only" })
}
