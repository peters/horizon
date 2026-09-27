//! Repository profiles grouped by compute type before choosing a worker size.
use super::Production;
use crate::theme;
use egui::{Button, RichText, Ui, Vec2};
use horizon_core::cloud_panel::Placement;

pub(super) fn field(ui: &mut Ui, form: &mut Production) {
    let Some(config) = &form.profiles else {
        return;
    };
    for (gpu, label) in [(false, "CPU profiles"), (true, "GPU profiles")] {
        let mut profiles: Vec<_> = config
            .profiles
            .iter()
            .filter(|(_, profile)| profile.gpu == gpu)
            .collect();
        profiles.sort_by_key(|(name, profile)| (profile.cpu, profile.memory_gb, *name));
        if profiles.is_empty() {
            continue;
        }
        ui.label(RichText::new(label).size(14.0).strong().color(theme::FG()));
        ui.horizontal_wrapped(|ui| {
            for (name, _) in profiles {
                if ui
                    .add(
                        Button::new(RichText::new(name).size(14.0))
                            .selected(form.selected_profile == *name)
                            .min_size(Vec2::new(0.0, 34.0))
                            .corner_radius(8),
                    )
                    .clicked()
                    && form.selected_profile != *name
                {
                    form.size = None;
                    form.placement = Placement::default();
                    form.provider = None;
                    form.selected_profile.clone_from(name);
                    form.launch.accounts_checked = false;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;
    use horizon_core::cloud_panel::CloudConfig;

    #[test]
    fn profile_buttons_are_grouped_and_sorted_by_resources_then_name() {
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
                "CPU profiles",
                "cpu-small",
                "cpu-large",
                "GPU profiles",
                "gpu-small",
                "gpu-large"
            ]
        );
    }
}
