//! CPU worker size choices shared by the New cloud dialog and cloud cards, so both
//! offer only the sizes `RunPod` provides and apply the same resize rules.
use horizon_core::cloud_runtime::flavors;

/// A worker's vCPU count and memory in GB.
pub(super) type Size = (u16, u16);

/// One size choice: its label, whether it is the current size, and the size choosing it selects.
pub(super) struct Choice {
    pub label: String,
    pub selected: bool,
    pub size: Size,
}

/// Each vCPU count offered with the container disk, keeping memory per vCPU where offered.
pub(super) fn vcpu_choices(current: Size, container_gb: u16) -> Vec<Choice> {
    flavors::vcpu_options(container_gb)
        .map(|cpu| Choice {
            label: format!("{cpu} vCPU"),
            selected: cpu == current.0,
            size: flavors::resize_vcpu(current, container_gb, cpu).unwrap_or(current),
        })
        .collect()
}

/// Each memory size offered at the current vCPU count.
pub(super) fn memory_choices(current: Size, container_gb: u16) -> Vec<Choice> {
    flavors::memory_options(current.0, container_gb)
        .into_iter()
        .map(|(memory, family)| Choice {
            label: format!("{memory} GB · {family}"),
            selected: memory == current.1,
            size: (current.0, memory),
        })
        .collect()
}

/// Offers each vCPU count for the container disk through `option(label, selected)`, which
/// returns whether it was chosen. Returns the size a changed choice selects.
pub(super) fn vcpu(current: Size, container_gb: u16, option: impl FnMut(String, bool) -> bool) -> Option<Size> {
    choose(vcpu_choices(current, container_gb), option)
}

/// Offers each memory size at the current vCPU count like [`vcpu`].
pub(super) fn memory(current: Size, container_gb: u16, option: impl FnMut(String, bool) -> bool) -> Option<Size> {
    choose(memory_choices(current, container_gb), option)
}

fn choose(choices: Vec<Choice>, mut option: impl FnMut(String, bool) -> bool) -> Option<Size> {
    let mut chosen = None;
    for choice in choices {
        if option(choice.label, choice.selected) && !choice.selected {
            chosen = Some(choice.size);
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
