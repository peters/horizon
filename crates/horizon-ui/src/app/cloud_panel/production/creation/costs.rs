//! What a new cloud costs: hours of compute, a month running, a month stopped, and
//! every kind of storage it is billed for.
use crate::theme;
use egui::{RichText, Sense, Ui, Vec2};
use horizon_core::cloud_runtime::prices::{PriceList, Profile};

/// Hours in an average month, for monthly totals.
const MONTH_HOURS: f64 = 730.0;

/// One kind of storage a cloud is billed for, per month.
#[derive(Debug, PartialEq)]
struct Storage {
    kind: &'static str,
    gb: u16,
    running: f64,
    /// `None` when it is not billed while the cloud is stopped.
    stopped: Option<f64>,
    note: &'static str,
}

fn storage(list: &PriceList, profile: &Profile) -> Vec<Storage> {
    let rates = list.storage;
    let volume = profile.storage.volume_gb;
    let mut items = Vec::new();
    if profile.gpu {
        let (running, stopped) = rates.pod_volume_month(u32::from(volume));
        items.push(Storage {
            kind: "Pod volume",
            gb: volume,
            running,
            stopped: Some(stopped),
            note: "The pod volume holds your files and costs twice as much while stopped.",
        });
    } else {
        let month = rates.network_month(u32::from(volume));
        items.push(Storage {
            kind: "Network volume",
            gb: volume,
            running: month,
            stopped: Some(month),
            note: "The network volume holds your files and stays in its data center.",
        });
    }
    let container = profile.storage.container_gb;
    items.push(Storage {
        kind: "Container disk",
        gb: container,
        running: f64::from(container) * rates.container,
        stopped: None,
        note: "The container disk is cleared when the cloud stops.",
    });
    items.retain(|item| item.gb > 0);
    items
}

/// Monthly totals running all month and stopped all month, as low and high bounds.
fn monthly(hourly: (f64, f64), storage: &[Storage]) -> ((f64, f64), f64) {
    let running: f64 = storage.iter().map(|item| item.running).sum();
    let stopped = storage.iter().filter_map(|item| item.stopped).sum();
    let (low, high) = hourly;
    ((low * MONTH_HOURS + running, high * MONTH_HOURS + running), stopped)
}

/// Totals for `hourly`, the lowest and highest price a deployment may be charged, and
/// the storage `profile` keeps. Without an hourly price, such as when no preferred GPU
/// is in stock, only the storage is shown.
pub(super) fn show(ui: &mut Ui, list: &PriceList, profile: &Profile, hourly: Option<(f64, f64)>) {
    let storage = storage(list, profile);
    if hourly.is_none() && storage.is_empty() {
        return;
    }
    let ((running_low, running_high), stopped) = monthly(hourly.unwrap_or_default(), &storage);
    divider(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 28.0;
        if let Some((low, high)) = hourly {
            stat(ui, "8 HOURS", &range(low * 8.0, high * 8.0), "compute");
            stat(ui, "24 HOURS", &range(low * 24.0, high * 24.0), "compute");
            stat(
                ui,
                "RUNNING",
                &format!("{}/mo", range(running_low, running_high)),
                "compute and storage",
            );
        }
        stat(ui, "STOPPED", &format!("{}/mo", money(stopped)), "storage it keeps");
    });
    if !storage.is_empty() {
        ui.add_space(10.0);
        breakdown(ui, &storage, list);
    }
}

fn divider(ui: &mut Ui) {
    ui.add_space(10.0);
    let (line, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(line, 0.0, theme::BORDER_SUBTLE());
    ui.add_space(8.0);
}

fn breakdown(ui: &mut Ui, storage: &[Storage], list: &PriceList) {
    let heading = |text: &str| RichText::new(text).size(10.5).color(theme::FG_DIM());
    egui::Grid::new("cloud-creation-storage")
        .num_columns(4)
        .spacing([18.0, 5.0])
        .show(ui, |ui| {
            ui.label(heading("STORAGE"));
            ui.label("");
            ui.label(heading("RUNNING"));
            ui.label(heading("STOPPED"));
            ui.end_row();
            for item in storage {
                ui.label(RichText::new(item.kind).size(12.5).color(theme::FG_SOFT()));
                ui.label(
                    RichText::new(format!("{} GB", item.gb))
                        .size(12.0)
                        .color(theme::FG_DIM()),
                );
                ui.label(amount(Some(item.running)));
                ui.label(amount(item.stopped));
                ui.end_row();
            }
        });
    // Said in words: hover text would draw below this modal.
    let notes: Vec<&str> = storage.iter().map(|item| item.note).collect();
    ui.add_space(4.0);
    ui.label(
        RichText::new(format!(
            "{} {} storage list prices, checked {}.",
            notes.join(" "),
            list.provider,
            list.storage.confirmed
        ))
        .size(11.0)
        .color(theme::FG_DIM()),
    );
}

fn amount(value: Option<f64>) -> RichText {
    match value {
        Some(value) => RichText::new(format!("{}/mo", money(value)))
            .size(12.5)
            .monospace()
            .color(theme::FG()),
        None => RichText::new("not billed").size(12.0).color(theme::FG_DIM()),
    }
}

fn stat(ui: &mut Ui, label: &str, value: &str, caption: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(RichText::new(label).size(10.5).color(theme::FG_DIM()));
        ui.label(RichText::new(value).size(16.0).color(theme::FG()));
        ui.label(RichText::new(caption).size(10.5).color(theme::FG_DIM()));
    });
}

pub(super) fn money(value: f64) -> String {
    if value >= 100.0 {
        format!("${value:.0}")
    } else {
        format!("${value:.2}")
    }
}

/// One amount when both ends read the same once rounded, otherwise a range.
pub(super) fn range(low: f64, high: f64) -> String {
    let (low, high) = (money(low), money(high));
    if low == high {
        low
    } else {
        format!("{low}–{}", high.trim_start_matches('$'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::cloud_runtime::prices::RUNPOD_STORAGE;

    fn list() -> PriceList {
        PriceList {
            provider: "RunPod",
            cpu: Vec::new(),
            gpus: Vec::new(),
            data_centers: Vec::new(),
            storage: RUNPOD_STORAGE,
        }
    }

    fn profile(gpu: bool, container_gb: u16, volume_gb: u16) -> Profile {
        let mut profile = horizon_core::cloud_panel::CloudConfig::parse(
            "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker:latest\n    cpu: 8\n    memory_gb: 16\n",
        )
        .unwrap()
        .profiles["dev"]
        .clone();
        profile.gpu = gpu;
        profile.storage.container_gb = container_gb;
        profile.storage.volume_gb = volume_gb;
        profile
    }

    #[test]
    fn prices_read_naturally() {
        assert_eq!(money(0.24), "$0.24");
        assert_eq!(money(5.76), "$5.76");
        assert_eq!(money(242.0), "$242");
        assert_eq!(range(0.24, 0.24), "$0.24");
        assert_eq!(range(0.24, 0.28), "$0.24–0.28");
        assert_eq!(range(0.244, 0.248), "$0.24–0.25");
        assert_eq!(range(0.241, 0.244), "$0.24");
        assert_eq!(range(1.92, 2.24), "$1.92–2.24");
    }

    #[test]
    fn a_cpu_cloud_keeps_its_network_volume_while_stopped() {
        let items = storage(&list(), &profile(false, 40, 100));
        let kinds: Vec<(&str, u16)> = items.iter().map(|item| (item.kind, item.gb)).collect();
        assert_eq!(kinds, [("Network volume", 100), ("Container disk", 40)]);
        let ((low, high), stopped) = monthly((0.24, 0.28), &items);
        // 730 hours of compute, plus $7 of network volume and $4 of container disk.
        assert!((low - 186.2).abs() < 1e-9 && (high - 215.4).abs() < 1e-9);
        assert!((stopped - 7.0).abs() < 1e-9);
    }

    #[test]
    fn a_stopped_gpu_cloud_pays_twice_for_its_pod_volume() {
        let items = storage(&list(), &profile(true, 50, 200));
        assert_eq!(items[0].kind, "Pod volume");
        assert!((items[0].running - 20.0).abs() < 1e-9);
        assert_eq!(items[0].stopped.map(|stopped| (stopped * 100.0).round()), Some(4000.0));
        assert_eq!(items[1].stopped, None);
        let (_, stopped) = monthly((0.49, 0.49), &items);
        assert!((stopped - 40.0).abs() < 1e-9);
        assert!(storage(&list(), &profile(true, 0, 0)).is_empty());
    }
}
