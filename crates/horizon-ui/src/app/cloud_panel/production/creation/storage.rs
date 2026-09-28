//! Storage choices edit only this dialog's profile; creation captures that profile.
use crate::theme;
use egui::{DragValue, RichText, Ui};
use horizon_core::cloud_runtime::{
    flavors,
    prices::{Profile, StorageTier},
    provider::{Choice, Description, Kind},
};

/// Why the chosen worker cannot start now, if anything stands in the way.
pub(super) fn launch_reason(form: &super::Production) -> Option<&'static str> {
    let profile = form.profiles.as_ref()?.profiles.get(&form.selected_profile)?;
    if super::provider::current(form.provider, profile).kind == Kind::Hetzner && form.prices.hetzner.too_old() {
        return Some("Hetzner prices are over an hour old. Refresh them before starting.");
    }
    if super::provider::current(form.provider, profile).kind == Kind::RunPod {
        if form.prices.too_old() {
            return Some("Prices are over an hour old. Refresh them before starting.");
        }
        // A catalog that lists workers but none this profile allows cannot start one,
        // and a GPU profile starts only on a type the catalog offers it.
        let listed = form
            .prices
            .list
            .as_ref()
            .is_some_and(|fetched| !fetched.value.0.cpu.is_empty() || !fetched.value.0.gpus.is_empty());
        if listed && let Some(catalog) = super::selector::catalog(form) {
            if catalog.offers.is_empty() {
                return Some("No worker the provider lists meets this profile's minimums.");
            }
            if profile.gpu && catalog.selected.is_none() {
                return Some("Choose a GPU type the catalog offers for this profile.");
            }
        }
        // Data centers the catalog knows must hold the chosen kind of workspace volume.
        if let Some(fetched) = form.prices.list.as_ref().filter(|_| !profile.gpu) {
            let mut known = fetched
                .value
                .0
                .data_centers
                .iter()
                .filter(|center| form.placement.data_centers.contains(&center.id))
                .peekable();
            if known.peek().is_some() && !known.any(|center| center.holds(profile.storage.volume_tier)) {
                return Some(
                    "The chosen data center cannot hold this kind of workspace volume. Choose another data center or storage type.",
                );
            }
        }
        // A CPU size also starts only as the catalog offers it; a size no flavor holds
        // keeps its more specific reason.
        if let Some(reason) = size_reason(form) {
            return Some(reason);
        }
        if listed && !profile.gpu && super::selector::catalog(form).is_some_and(|catalog| catalog.selected.is_none()) {
            return Some("Choose a CPU size the catalog offers for this profile.");
        }
        return None;
    }
    size_reason(form)
}

pub(super) fn size_reason(form: &super::Production) -> Option<&'static str> {
    let profile = form.profiles.as_ref()?.profiles.get(&form.selected_profile)?;
    let provider = super::provider::current(form.provider, profile);
    if provider.kind == Kind::Hetzner {
        form.prices.hetzner.displayed()?.value.as_ref()?;
        let sized = super::provider::sized(provider, profile, form.size).ok()?;
        return (!super::provider::location_offers(&form.prices, &sized)
            .iter()
            .any(|offer| form.placement.is_any() || form.placement.data_centers.contains(&offer.location)))
        .then_some("Choose a system disk and CPU size supported by a server in the selected location.");
    }
    (provider.kind == Kind::RunPod
        && !profile.gpu
        && !flavors::offered(
            form.size.unwrap_or((profile.cpu, profile.memory_gb)),
            profile.storage.container_gb,
        ))
    .then_some("Choose a CPU and memory size that supports this container disk before starting.")
}

/// The container disk alone, for providers whose workspace volume the summary sets.
pub(super) fn container_field(ui: &mut Ui, profile: &mut Profile, provider: &Description) {
    let container_max = if provider.kind == Kind::RunPod && !profile.gpu {
        flavors::max_container_gb()
    } else {
        u16::MAX
    };
    ui.horizontal(|ui| {
        let label = ui.label("Container disk");
        ui.add(
            DragValue::new(&mut profile.storage.container_gb)
                .range(1..=container_max)
                .suffix(" GB"),
        )
        .labelled_by(label.id);
    });
    ui.small("Temporary storage for the image and installed tools; cleared when the worker stops.");
}

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
        (1, u32::from(u16::MAX))
    } else {
        provider.cpu_volume_gb
    };
    ui.horizontal(|ui| {
        let label = ui.label("Workspace size");
        ui.add(
            DragValue::new(&mut profile.storage.volume_gb)
                .range(f64::from(min)..=f64::from(max))
                .suffix(" GB"),
        )
        .labelled_by(label.id);
    });
    ui.small(if profile.gpu {
        "Files survive a stop, but are deleted with the pod."
    } else {
        "Your workspace is retained when the worker stops."
    });
    let container_max = if provider.kind == Kind::RunPod && !profile.gpu {
        flavors::max_container_gb()
    } else {
        u16::MAX
    };
    ui.horizontal(|ui| {
        let label = ui.label(if provider.kind == Kind::Hetzner {
            "System disk requirement"
        } else {
            "Container disk"
        });
        ui.add(
            DragValue::new(&mut profile.storage.container_gb)
                .range(1..=container_max)
                .suffix(" GB"),
        )
        .labelled_by(label.id);
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
    fn container_limit_uses_cpu_flavors_without_restricting_gpu_or_system_disks() {
        let mut profile = profile();
        profile.storage.container_gb = u16::MAX;
        labels(&mut profile, &provider::RUNPOD);
        assert_eq!(profile.storage.container_gb, flavors::max_container_gb());
        for (gpu, provider) in [(true, &provider::RUNPOD), (false, &provider::HETZNER)] {
            profile.gpu = gpu;
            profile.storage.container_gb = u16::MAX;
            labels(&mut profile, provider);
            assert_eq!(profile.storage.container_gb, u16::MAX);
        }
    }

    #[test]
    fn rendering_preserves_small_gpu_workspace_sizes() {
        let mut profile = profile();
        profile.gpu = true;
        for size in [1, 9] {
            profile.storage.volume_gb = size;
            let original = profile.clone();
            labels(&mut profile, &provider::RUNPOD);
            assert_eq!(profile, original);
        }
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
