//! Storage choices edit only this dialog's profile; creation captures that profile.
use crate::theme;
use egui::{DragValue, RichText, Ui};
use horizon_core::cloud_runtime::{
    prices::{Profile, StorageTier},
    provider::{Choice, Description, Kind},
};

pub(super) fn field(ui: &mut Ui, profile: &mut Profile, provider: &Description) -> bool {
    let before = profile.storage.clone();
    ui.add_space(6.0);
    ui.label(RichText::new("Storage").size(14.0).strong().color(theme::FG()));
    if provider.offers(Choice::VolumeTiers) && !profile.gpu {
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(
                &mut profile.storage.volume_tier,
                StorageTier::Standard,
                "Standard network",
            );
            ui.selectable_value(
                &mut profile.storage.volume_tier,
                StorageTier::HighPerformance,
                "High-performance network",
            );
        });
    } else {
        profile.storage.volume_tier = StorageTier::Standard;
        ui.label(if profile.gpu {
            "Pod volume"
        } else {
            "Persistent block volume"
        });
    }
    let (min, max) = if profile.gpu {
        (10, u32::from(u16::MAX))
    } else {
        provider.cpu_volume_gb
    };
    ui.horizontal(|ui| {
        ui.label("Workspace size");
        ui.add(
            DragValue::new(&mut profile.storage.volume_gb)
                .range(f64::from(min)..=f64::from(max))
                .suffix(" GB"),
        );
    });
    ui.small(if profile.gpu {
        "Files survive a stop, but are deleted with the pod."
    } else {
        "Your workspace is retained when the worker stops."
    });
    ui.horizontal(|ui| {
        ui.label(if provider.kind == Kind::Hetzner {
            "System disk requirement"
        } else {
            "Container disk"
        });
        ui.add(
            DragValue::new(&mut profile.storage.container_gb)
                .range(1..=u16::MAX)
                .suffix(" GB"),
        );
    });
    ui.small(if provider.kind == Kind::Hetzner {
        "Choose a server type with at least this much system disk."
    } else {
        "Temporary storage for the image and installed tools; cleared when the worker stops."
    });
    before != profile.storage
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;
    use horizon_core::cloud_runtime::provider;

    fn profile() -> Profile {
        serde_json::from_value(serde_json::json!({
            "provider":"runpod", "image":"example.invalid/worker", "cpu":4,"memory_gb":8
        }))
        .unwrap()
    }

    fn labels(profile: &mut Profile, provider: &Description) -> Vec<String> {
        egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| {
                field(ui, profile, provider);
            })
            .discard_textures()
            .shapes
            .into_iter()
            .filter_map(|shape| match shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn storage_types_match_the_worker_and_provider() {
        let mut profile = profile();
        let cpu = labels(&mut profile, &provider::RUNPOD);
        assert!(cpu.iter().any(|label| label == "High-performance network"));
        profile.gpu = true;
        profile.storage.volume_tier = StorageTier::HighPerformance;
        let gpu = labels(&mut profile, &provider::RUNPOD);
        assert!(gpu.iter().any(|label| label == "Pod volume"));
        assert!(!gpu.iter().any(|label| label == "High-performance network"));
        assert_eq!(profile.storage.volume_tier, StorageTier::Standard);
        profile.gpu = false;
        let block = labels(&mut profile, &provider::HETZNER);
        assert!(block.iter().any(|label| label == "Persistent block volume"));
        assert!(block.iter().any(|label| label == "System disk requirement"));
    }
}
