//! Repository profiles, CPU ones first, before choosing a worker for one.
use super::{Production, pricing::option};
use crate::theme;
use egui::{RichText, Ui};
use horizon_core::cloud_panel::Placement;

pub(super) fn field(ui: &mut Ui, form: &mut Production) {
    let Some(config) = &form.profiles else {
        return;
    };
    let mut profiles: Vec<_> = config.profiles.iter().collect();
    profiles.sort_by_key(|(name, profile)| (profile.gpu, profile.cpu, profile.memory_gb, *name));
    ui.label(RichText::new("Profile").size(14.0).strong().color(theme::FG()));
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        for (name, profile) in profiles {
            let detail = if profile.gpu {
                "GPU worker".to_owned()
            } else {
                format!("CPU · from {} vCPU, {} GB", profile.cpu, profile.memory_gb)
            };
            if option(ui, name, form.selected_profile == *name, Some(&detail), None) && form.selected_profile != *name {
                chosen = Some(name.clone());
            }
        }
    });
    if let Some(name) = chosen {
        form.size = None;
        form.placement = Placement::default();
        form.provider = None;
        form.selected_profile = name;
        form.launch.accounts_checked = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;
    use horizon_core::cloud_panel::CloudConfig;

    #[test]
    fn profile_buttons_list_cpu_profiles_first_sorted_by_resources_then_name() {
        let config = CloudConfig::parse("version: 1\ndefault: cpu-large\nprofiles:\n  cpu-large:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 16\n    memory_gb: 64\n  cpu-small:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n  gpu-large:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 16\n    memory_gb: 64\n    gpu: true\n  gpu-small:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n    gpu: true\n").unwrap();
        let mut form = Production {
            profiles: Some(config),
            ..Production::default()
        };
        let ctx = egui::Context::default();
        let output = ctx
            .run_ui(egui::RawInput::default(), |ui| field(ui, &mut form))
            .discard_textures();
        let labels: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            labels,
            [
                "Profile",
                "cpu-small\nCPU · from 4 vCPU, 8 GB",
                "cpu-large\nCPU · from 16 vCPU, 64 GB",
                "gpu-small\nGPU worker",
                "gpu-large\nGPU worker"
            ]
        );
    }
}
