//! Runtime profile details and pre-allocation size choices.
use super::{HorizonApp, RichText, cloud_runtime, placement, self_stop, theme};
use cloud_runtime::state::Store;

impl HorizonApp {
    /// The next deployment attempt adopts the size. The saved record, not the
    /// cached snapshot, decides whether a worker may already be requested.
    pub(in crate::app::cloud_panel::production) fn resize_production_cloud(
        &mut self,
        id: u32,
        (cpu, memory_gb): (u16, u16),
    ) {
        let Some(index) = self.cloud_prototype.groups.0.iter().position(|group| group.issue == id) else {
            return;
        };
        let Some(launch) = self.cloud_prototype.groups.0[index].remote.clone() else {
            return;
        };
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
        let loaded = cloud_runtime::state::cloud_directory(&root, &launch.id)
            .and_then(|path| Store::lock(&path))
            .and_then(|store| store.load());
        let allowed = match loaded {
            Ok(Some(state)) if !state.accepts_next_size() => {
                let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
                runtime.stage = Some(state.stage);
                runtime.state = Some(state);
                false
            }
            Ok(Some(_)) => true,
            Ok(None) => !launch.deployment_started,
            Err(error) => {
                self.cloud_prototype.error = Some(error.to_string());
                return;
            }
        };
        if !allowed {
            self.cloud_prototype.error =
                Some("A worker was requested for this cloud; its size can no longer change".into());
            return;
        }
        if let Some(launch) = self.cloud_prototype.groups.0[index].remote.as_mut() {
            launch.profile.cpu = cpu;
            launch.profile.memory_gb = memory_gb;
        }
        self.save_cloud_prototype();
    }
}

pub(super) fn profile_details(
    ui: &mut egui::Ui,
    id: u32,
    launch: &horizon_core::cloud_panel::CloudLaunch,
    runtime: &super::super::Runtime,
    region_of: &dyn Fn(&str) -> Option<String>,
) -> Option<(u16, u16)> {
    ui.label(RichText::new(&launch.profile_name).size(17.0).color(theme::FG_DIM()));
    let resize = machine_size(ui, id, launch, runtime);
    ui.label(RichText::new(&launch.profile.image).monospace().size(14.0));
    placement::where_it_lives(ui, launch, runtime.state.as_ref(), region_of);
    self_stop::show(ui, runtime.state.as_ref());
    ui.small(format!(
        "Agents: {}",
        if launch.profile.capabilities.agents.is_empty() {
            "none".into()
        } else {
            launch.profile.capabilities.agents_argument()
        }
    ));
    ui.small(format!(
        "Browsers: {}",
        if launch.profile.capabilities.browsers.is_empty() {
            "disabled".into()
        } else {
            launch.profile.capabilities.browsers_argument()
        }
    ));
    if let Some(selected) = &launch.profile.capabilities.browserstack {
        ui.small(format!("Remote account: {}", selected.provider));
    }
    ui.small(if launch.profile.capabilities.desktop {
        "Desktop: enabled"
    } else {
        "Desktop: disabled"
    });
    resize
}

/// CPU and memory can change until a worker is requested; `RunPod` cannot resize an
/// existing pod. Sizes here are `RunPod`'s CPU flavors, so a provider priced by server
/// type shows its fixed size; its size is chosen when the cloud is created.
fn machine_size(
    ui: &mut egui::Ui,
    id: u32,
    launch: &horizon_core::cloud_panel::CloudLaunch,
    runtime: &super::super::Runtime,
) -> Option<(u16, u16)> {
    use super::super::machine_size;
    let profile = &launch.profile;
    let idle = runtime.receiver.is_none() && runtime.recovery_receiver.is_none() && !runtime.state_unavailable;
    let flavors = cloud_runtime::provider::by_id(&profile.provider)
        .is_some_and(|provider| provider.pricing == cloud_runtime::provider::Pricing::Flavors);
    if profile.gpu
        || !flavors
        || !idle
        || !runtime
            .state
            .as_ref()
            .is_none_or(horizon_core::cloud_runtime::state::Deployment::accepts_next_size)
    {
        // A requested worker's saved size is authoritative.
        let fixed = runtime.state.as_ref().filter(|state| !state.accepts_next_size());
        let shown = fixed.map_or(profile, |state| &state.profile);
        let label = ui.label(machine_size::fixed((shown.cpu, shown.memory_gb), profile.gpu));
        if fixed.is_some() && flavors {
            label.on_hover_text(
                "A ready CPU cloud can change compute size or grow its workspace using the resize controls below.",
            );
        }
        return None;
    }
    let hint = format!(
        "CPU-only RunPod worker. Only sizes offered with this cloud's {} GB container disk are listed.",
        profile.storage.container_gb
    );
    let current = (profile.cpu, profile.memory_gb);
    let disk = profile.storage.container_gb;
    let mut size = None;
    // Top alignment keeps equally tall drop-downs level when the theme pads them above row height.
    ui.horizontal_top(|ui| {
        egui::ComboBox::from_id_salt(("cloud-vcpu", id))
            .selected_text(format!("{} vCPU", profile.cpu))
            .show_ui(ui, |ui| {
                size = machine_size::vcpu(current, disk, |label, selected| {
                    ui.selectable_label(selected, label).clicked()
                });
            })
            .response
            .on_hover_text(&hint);
        egui::ComboBox::from_id_salt(("cloud-memory", id))
            .selected_text(format!("{} GB", profile.memory_gb))
            .show_ui(ui, |ui| {
                size = machine_size::memory(current, disk, |label, selected| {
                    ui.selectable_label(selected, label).clicked()
                })
                .or(size);
            })
            .response
            .on_hover_text(&hint);
    });
    if let Some(warning) = machine_size::unoffered(current, disk) {
        ui.colored_label(egui::Color32::LIGHT_RED, warning);
    }
    size
}
