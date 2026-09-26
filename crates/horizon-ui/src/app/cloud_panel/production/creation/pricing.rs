//! Worker size choices with their prices, and a price card with live stock, for New cloud.
use super::{
    super::{
        machine_size::{self, Size},
        prices::{FRESH, State},
    },
    costs::{self, money, range},
};
use crate::theme;
use egui::{
    Align, Button, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, Sense, Stroke, TextFormat, Ui, Vec2,
    WidgetInfo, WidgetType, text::LayoutJob,
};
use horizon_core::{
    cloud_panel::Placement,
    cloud_runtime::prices::{self, Availability, GpuPrice, Preferences, PriceList, Profile},
};

/// Size buttons for a CPU profile, each with its hourly price. Returns a newly chosen size.
pub(super) fn size_field(ui: &mut Ui, prices: &State, profile: &Profile, current: Size) -> Option<Size> {
    ui.label(RichText::new("Size").size(14.0).strong().color(theme::FG()));
    let mut chosen = None;
    if profile.gpu {
        ui.label(
            RichText::new(machine_size::fixed(current, true))
                .size(13.0)
                .color(theme::FG_SOFT()),
        );
        ui.small("GPU workers use the size set by their profile.");
    } else {
        let disk = profile.storage.container_gb;
        let list = prices.list.as_ref().map(|list| &list.value);
        ui.horizontal_wrapped(|ui| {
            for choice in machine_size::vcpu_choices(current, disk) {
                let from = list.and_then(|(list, preferences)| {
                    machine_size::memory_choices(choice.size, disk)
                        .iter()
                        .filter_map(|memory| hourly(list, preferences, profile, memory.size))
                        .map(|(low, _)| low)
                        .min_by(f64::total_cmp)
                });
                let from = from.map(|low| format!("from {}/h", money(low)));
                if option(ui, &choice.label, choice.selected, from.as_deref(), None) {
                    chosen = Some(choice.size);
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            for choice in machine_size::memory_choices(current, disk) {
                let price = list
                    .and_then(|(list, preferences)| hourly(list, preferences, profile, choice.size))
                    .map(|(low, high)| format!("{}/h", range(low, high)));
                if option(ui, &choice.label, choice.selected, price.as_deref(), None) {
                    chosen = Some(choice.size);
                }
            }
        });
        if let Some(warning) = machine_size::unoffered(current, disk) {
            ui.colored_label(theme::PALETTE_RED(), warning);
        } else {
            // Tooltips would draw below this modal, so the offer rule is shown inline.
            ui.small(format!(
                "RunPod CPU sizes offered with this profile's {disk} GB container disk."
            ));
        }
    }
    chosen.filter(|size| *size != current)
}

/// A choice button with an optional second line, such as a price or stock, drawn in
/// `tint` or else in the accent color when selected.
pub(super) fn option(ui: &mut Ui, label: &str, selected: bool, detail: Option<&str>, tint: Option<Color32>) -> bool {
    let mut job = LayoutJob::default();
    let format = |size: f32, color: Color32| TextFormat {
        font_id: FontId::proportional(size),
        color,
        ..TextFormat::default()
    };
    job.append(label, 0.0, format(13.0, theme::FG()));
    if let Some(detail) = detail {
        let color = tint.unwrap_or(if selected { theme::ACCENT() } else { theme::FG_DIM() });
        job.append(&format!("\n{detail}"), 0.0, format(11.0, color));
    }
    ui.add(
        Button::new(job)
            .selected(selected)
            .min_size(Vec2::new(0.0, if detail.is_some() { 42.0 } else { 30.0 }))
            .corner_radius(8),
    )
    .clicked()
}

/// The price range of `size`, across the flavors a deployment would request.
fn hourly(list: &PriceList, preferences: &Preferences, profile: &Profile, size: Size) -> Option<(f64, f64)> {
    let sized = Profile {
        cpu: size.0,
        memory_gb: size.1,
        ..profile.clone()
    };
    list.cpu_hourly(&prices::requested_flavors(&sized, preferences), size.0)
}

/// What the person asked for on the price card.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CardAction {
    /// Fetch prices and stock again.
    Refresh,
    /// Request this GPU type for the new cloud instead of the preferences.
    UseGpu(String),
}

/// The price card for `profile` at its chosen size in `placement`, once a price check
/// has started.
pub(super) fn card(ui: &mut Ui, prices: &State, profile: &Profile, placement: &Placement) -> Option<CardAction> {
    let mut action = None;
    if prices.list.is_none() && prices.list_error.is_none() && !prices.loading() {
        return action;
    }
    Frame::new()
        .fill(theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.06))
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(12)
        .inner_margin(Margin::symmetric(16, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            match (&prices.list, &prices.list_error) {
                (Some(fetched), _) => {
                    let (list, preferences) = &fetched.value;
                    if profile.gpu {
                        action = gpu_summary(ui, list, preferences, profile, placement).map(CardAction::UseGpu);
                    } else {
                        cpu_summary(ui, prices, list, preferences, profile, placement);
                    }
                    ui.add_space(10.0);
                    if footer(ui, list.provider, fetched.at.elapsed(), prices.loading()) {
                        action = Some(CardAction::Refresh);
                    }
                }
                (None, Some(error)) => {
                    ui.label(
                        RichText::new("Prices are unavailable")
                            .size(14.0)
                            .color(theme::FG_SOFT()),
                    );
                    ui.label(RichText::new(error).size(11.5).color(theme::FG_DIM()));
                    if ui.add(Button::new("Try again").corner_radius(8)).clicked() {
                        action = Some(CardAction::Refresh);
                    }
                }
                (None, None) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Checking RunPod prices and stock…").color(theme::FG_DIM()));
                    });
                }
            }
        });
    action
}

fn cpu_summary(
    ui: &mut Ui,
    prices: &State,
    list: &PriceList,
    preferences: &Preferences,
    profile: &Profile,
    placement: &Placement,
) {
    let flavors = prices::requested_flavors(profile, preferences);
    let Some((low, high)) = list.cpu_hourly(&flavors, profile.cpu) else {
        ui.label(RichText::new("RunPod publishes no price for this size").color(theme::FG_SOFT()));
        return;
    };
    let within = &placement.data_centers;
    let stock = match prices.size(profile, (profile.cpu, profile.memory_gb)) {
        None => Stock::Checking,
        Some(Ok(size)) => Stock::Known(size.best(within), Some(in_scope(size.count(within), placement))),
        Some(Err(error)) => Stock::Unknown(error.to_owned()),
    };
    headline(ui, &range(low, high), &stock);
    let names: Vec<&str> = list
        .cpu
        .iter()
        .filter(|flavor| flavors.contains(&flavor.id))
        .map(|flavor| flavor.name.as_str())
        .collect();
    ui.label(
        RichText::new(format!(
            "{} vCPU · {} GB · {} · Secure Cloud",
            profile.cpu,
            profile.memory_gb,
            names.join(" or ")
        ))
        .size(12.0)
        .color(theme::FG_SOFT()),
    );
    stock_detail(ui, &stock);
    costs::show(ui, list, profile, Some((low, high)));
}

/// What the stock pill summarizes, in words: hover text would draw below this modal.
fn stock_detail(ui: &mut Ui, stock: &Stock) {
    let detail = match stock {
        Stock::Known(_, Some(detail)) => detail.clone(),
        Stock::Unknown(error) => format!("Stock unknown: {error}"),
        Stock::Checking | Stock::NotOffered | Stock::Known(_, None) => return,
    };
    ui.label(RichText::new(detail).size(11.5).color(theme::FG_DIM()));
}

/// Where GPU preferences are set until New cloud offers its own GPU choice.
const GPU_SETTING: &str = "gpu_types in ~/.horizon/cloud/settings.json";

/// A preferred GPU type in the order deployments request them, with its catalog entry
/// when the provider offers it and its best availability in the chosen data centers.
struct Ranked<'a> {
    id: &'a str,
    gpu: Option<&'a GpuPrice>,
    availability: Availability,
}

/// Every preferred GPU type in request order, and the first one in stock.
fn ranked<'a>(list: &'a PriceList, gpu_types: &'a [String], within: &[String]) -> (Vec<Ranked<'a>>, Option<usize>) {
    let preferred: Vec<Ranked<'a>> = gpu_types
        .iter()
        .map(|id| Ranked {
            id,
            gpu: list.gpu(id),
            availability: list.gpu_availability(id, within),
        })
        .collect();
    let chosen = preferred
        .iter()
        .position(|row| row.gpu.is_some() && row.availability != Availability::None);
    (preferred, chosen)
}

/// Data centers in `within` (every allowed one when empty) with GPU type `id` in stock.
fn gpu_centers(list: &PriceList, id: &str, within: &[String]) -> usize {
    list.data_centers
        .iter()
        .filter(|center| within.is_empty() || within.contains(&center.id))
        .filter(|center| list.gpu_availability(id, std::slice::from_ref(&center.id)) != Availability::None)
        .count()
}

/// The GPU types a new cloud would request: those chosen for it, or else the machine's
/// preferences.
pub(super) fn requested_gpus<'a>(preferences: &'a Preferences, placement: &'a Placement) -> &'a [String] {
    if placement.gpu_types.is_empty() {
        &preferences.gpu_types
    } else {
        &placement.gpu_types
    }
}

/// Returns a GPU type to request instead, when the person picks one.
fn gpu_summary(
    ui: &mut Ui,
    list: &PriceList,
    preferences: &Preferences,
    profile: &Profile,
    placement: &Placement,
) -> Option<String> {
    let within = &placement.data_centers;
    let chosen_here = !placement.gpu_types.is_empty();
    let (preferred, chosen) = ranked(list, requested_gpus(preferences, placement), within);
    let chosen_gpu = chosen.and_then(|index| Some((preferred[index].gpu?, preferred[index].availability)));
    if let Some((gpu, availability)) = chosen_gpu {
        let stock = Stock::Known(
            availability,
            Some(in_scope(gpu_centers(list, &gpu.id, within), placement)),
        );
        headline(ui, &money(gpu.hourly), &stock);
        let why = match (chosen_here, placement.gpu_types.len()) {
            (false, _) => "first preferred GPU in stock",
            (true, 1) => "chosen for this cloud",
            (true, _) => "first chosen GPU in stock",
        };
        ui.label(
            RichText::new(format!("{} · {} GB · {why} · Secure Cloud", gpu.name, gpu.memory_gb))
                .size(12.0)
                .color(theme::FG_SOFT()),
        );
        stock_detail(ui, &stock);
    } else {
        let heading = match (chosen_here, placement.gpu_types.len()) {
            (false, _) => "None of your preferred GPUs is in stock",
            (true, 1) => "The GPU chosen for this cloud is out of stock",
            (true, _) => "None of the GPUs chosen for this cloud is in stock",
        };
        ui.label(RichText::new(heading).size(16.0).strong().color(theme::PALETTE_RED()));
    }
    let mut use_gpu = None;
    if preferred.is_empty() {
        ui.label(
            RichText::new(format!(
                "Set {GPU_SETTING} to see prices for the GPUs this cloud may use."
            ))
            .size(12.0)
            .color(theme::FG_DIM()),
        );
    } else {
        gpu_rows(ui, &preferred, chosen);
    }
    // Offered with or without preferences, so a sold-out or empty list never blocks a cloud.
    if chosen_gpu.is_none()
        && let Some(cheapest) = list.cheapest_available_gpu(within)
    {
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!(
                "Cheapest in stock now: {} · {} GB · {}/h",
                cheapest.name,
                cheapest.memory_gb,
                money(cheapest.hourly)
            ))
            .size(12.0)
            .color(theme::FG_SOFT()),
        );
        let label = RichText::new(format!("Use {} instead", cheapest.name)).color(theme::FG());
        if ui
            .add(
                Button::new(label)
                    .corner_radius(8)
                    .fill(theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.25)),
            )
            .clicked()
        {
            use_gpu = Some(cheapest.id.clone());
        }
        ui.label(
            RichText::new(format!(
                "Only this cloud changes. Other GPU types are under Advanced; the defaults are {GPU_SETTING}."
            ))
            .size(11.0)
            .color(theme::FG_DIM()),
        );
    }
    // Storage is billed whichever GPU the cloud gets, so it shows even when none is in stock.
    costs::show(ui, list, profile, chosen_gpu.map(|(gpu, _)| (gpu.hourly, gpu.hourly)));
    use_gpu
}

fn gpu_rows(ui: &mut Ui, preferred: &[Ranked<'_>], chosen: Option<usize>) {
    ui.add_space(8.0);
    ui.label(
        RichText::new("REQUESTED IN THIS ORDER")
            .size(10.5)
            .color(theme::FG_DIM()),
    );
    egui::Grid::new("cloud-creation-gpu-prices")
        .num_columns(5)
        .spacing([14.0, 6.0])
        .show(ui, |ui| {
            for (rank, row) in preferred.iter().enumerate() {
                let first = chosen == Some(rank);
                let color = if first { theme::FG() } else { theme::FG_SOFT() };
                let marker = if first { theme::ACCENT() } else { theme::FG_DIM() };
                ui.label(RichText::new(format!("{}", rank + 1)).size(12.0).strong().color(marker));
                let Some(gpu) = row.gpu else {
                    ui.label(RichText::new(row.id).size(12.5).color(theme::FG_DIM()));
                    ui.label("");
                    ui.label("");
                    pill(ui, &Stock::NotOffered, true);
                    ui.end_row();
                    continue;
                };
                ui.label(RichText::new(&gpu.name).size(12.5).color(color));
                ui.label(
                    RichText::new(format!("{} GB", gpu.memory_gb))
                        .size(12.0)
                        .color(theme::FG_DIM()),
                );
                ui.label(
                    RichText::new(format!("{}/h", money(gpu.hourly)))
                        .size(12.5)
                        .monospace()
                        .color(color),
                );
                pill(ui, &Stock::Known(row.availability, None), true);
                ui.end_row();
            }
        });
    if preferred.iter().any(|row| row.gpu.is_none()) {
        ui.label(
            RichText::new("Not offered: RunPod's Secure Cloud catalog does not list that GPU type.")
                .size(11.5)
                .color(theme::FG_DIM()),
        );
    }
}

enum Stock {
    Checking,
    /// The provider's catalog does not list this type.
    NotOffered,
    /// A level, and in words where it applies.
    Known(Availability, Option<String>),
    Unknown(String),
}

fn headline(ui: &mut Ui, price: &str, stock: &Stock) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(price).size(28.0).strong().color(theme::FG()));
        ui.label(RichText::new("/ hour").size(13.0).color(theme::FG_DIM()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| pill(ui, stock, false));
    });
}

const DOT: f32 = 7.0;
const GAP: f32 = 6.0;

fn pill(ui: &mut Ui, stock: &Stock, compact: bool) {
    let (text, color, detail) = match stock {
        Stock::Checking => ("Checking stock", theme::FG_DIM(), None),
        Stock::NotOffered => ("Not offered", theme::FG_DIM(), None),
        Stock::Unknown(error) => ("Stock unknown", theme::FG_DIM(), Some(error.as_str())),
        Stock::Known(level, detail) => {
            let (text, color) = level_text(*level);
            (text, color, detail.as_deref())
        }
    };
    let text = text.to_owned();
    let description = detail.map_or_else(|| text.clone(), |detail| format!("{text}. {detail}"));
    // Painted at its own size, so it stays compact in any parent layout.
    let galley = ui.painter().layout_no_wrap(text, FontId::proportional(11.5), color);
    let padding = Vec2::new(8.0, if compact { 2.0 } else { 4.0 });
    let size = Vec2::new(
        padding.x * 2.0 + DOT + GAP + galley.size().x,
        galley.size().y + padding.y * 2.0,
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        CornerRadius::same(10),
        theme::blend(theme::PANEL_BG_ALT(), color, 0.16),
    );
    let left = rect.left() + padding.x;
    painter.circle_filled(egui::pos2(left + DOT / 2.0, rect.center().y), DOT / 2.0, color);
    let text_top = rect.center().y - galley.size().y / 2.0;
    painter.galley(egui::pos2(left + DOT + GAP, text_top), galley, color);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &description));
}

pub(super) fn level_text(level: Availability) -> (&'static str, Color32) {
    match level {
        Availability::High | Availability::Medium => ("In stock", theme::PALETTE_GREEN()),
        Availability::Low => ("Low stock", theme::PALETTE_YELLOW()),
        Availability::None => ("Out of stock", theme::PALETTE_RED()),
    }
}

/// Where a size or GPU is in stock, in the words of the chosen placement.
fn in_scope(count: usize, placement: &Placement) -> String {
    let place = match (placement.data_centers.as_slice(), placement.region.as_deref()) {
        ([], _) => None,
        ([one], _) => Some(one.clone()),
        (_, Some(region)) => Some(region.to_owned()),
        (_, None) => Some("the chosen data centers".to_owned()),
    };
    let one = placement.data_centers.len() == 1;
    match (count, place) {
        (0, None) => "Out of stock in every allowed data center".to_owned(),
        (1, None) => "In stock in 1 allowed data center".to_owned(),
        (count, None) => format!("In stock in {count} allowed data centers"),
        (0, Some(place)) => format!("Out of stock in {place}"),
        (_, Some(place)) if one => format!("In stock in {place}"),
        (1, Some(place)) => format!("In stock in 1 data center in {place}"),
        (count, Some(place)) => format!("In stock in {count} data centers in {place}"),
    }
}

/// Returns whether prices should be fetched again.
fn footer(ui: &mut Ui, provider: &str, age: std::time::Duration, loading: bool) -> bool {
    let mut refresh = false;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "{provider} list prices · updated {} · refreshed every {} min",
                ago(age),
                FRESH.as_secs() / 60
            ))
            .size(11.0)
            .color(theme::FG_DIM()),
        );
        if loading {
            ui.spinner();
        } else {
            refresh = ui
                .add(Button::new(RichText::new("Refresh").size(11.0).color(theme::ACCENT())).frame(false))
                .clicked();
        }
    });
    refresh
}

fn ago(age: std::time::Duration) -> String {
    match age.as_secs() / 60 {
        0 => "just now".to_owned(),
        1 => "1 min ago".to_owned(),
        minutes => format!("{minutes} min ago"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_read_naturally() {
        assert_eq!(ago(std::time::Duration::from_secs(20)), "just now");
        assert_eq!(ago(std::time::Duration::from_secs(150)), "2 min ago");
    }

    #[test]
    fn stock_details_name_where_the_cloud_may_go() {
        let any = Placement::default();
        assert_eq!(in_scope(0, &any), "Out of stock in every allowed data center");
        assert_eq!(in_scope(1, &any), "In stock in 1 allowed data center");
        assert_eq!(in_scope(3, &any), "In stock in 3 allowed data centers");
        let europe = Placement {
            region: Some("Europe".into()),
            data_centers: vec!["EU-RO-1".into(), "EUR-IS-1".into()],
            gpu_types: Vec::new(),
        };
        assert_eq!(in_scope(0, &europe), "Out of stock in Europe");
        assert_eq!(in_scope(1, &europe), "In stock in 1 data center in Europe");
        assert_eq!(in_scope(2, &europe), "In stock in 2 data centers in Europe");
        let one = Placement {
            region: Some("Europe".into()),
            data_centers: vec!["EU-RO-1".into()],
            gpu_types: Vec::new(),
        };
        assert_eq!(in_scope(1, &one), "In stock in EU-RO-1");
        assert_eq!(in_scope(0, &one), "Out of stock in EU-RO-1");
    }

    #[test]
    fn gpu_preferences_keep_their_rank_when_the_catalog_lacks_one() {
        let gpu = |id: &str, hourly| GpuPrice {
            id: id.into(),
            name: id.into(),
            memory_gb: 24,
            hourly,
        };
        let center = |id: &str, gpus: &[(&str, Availability)]| horizon_core::cloud_runtime::prices::DataCenter {
            id: id.into(),
            region: "EUROPE".into(),
            workspace_storage: false,
            gpus: gpus.iter().map(|&(gpu, level)| (gpu.into(), level)).collect(),
        };
        let list = PriceList {
            provider: "RunPod",
            cpu: Vec::new(),
            gpus: vec![gpu("ada", 0.28), gpu("l4", 0.49)],
            data_centers: vec![
                center("EU-RO-1", &[("ada", Availability::None), ("l4", Availability::High)]),
                center("EU-SE-1", &[("ada", Availability::Low)]),
            ],
            regions: std::collections::BTreeMap::new(),
            storage: horizon_core::cloud_runtime::prices::RUNPOD_STORAGE,
        };
        let preferences = Preferences {
            cpu_flavors: Vec::new(),
            gpu_types: vec!["retired".into(), "ada".into(), "l4".into()],
        };
        let (preferred, chosen) = ranked(&list, &preferences.gpu_types, &["EU-RO-1".into()]);
        let ids: Vec<(&str, bool)> = preferred.iter().map(|row| (row.id, row.gpu.is_some())).collect();
        assert_eq!(ids, [("retired", false), ("ada", true), ("l4", true)]);
        assert_eq!(chosen, Some(2));
        // In another data center the second preference is in stock and comes first.
        assert_eq!(ranked(&list, &preferences.gpu_types, &["EU-SE-1".into()]).1, Some(1));
        assert_eq!(gpu_centers(&list, "ada", &[]), 1);

        let retired = Preferences {
            cpu_flavors: Vec::new(),
            gpu_types: vec!["retired".into()],
        };
        let (preferred, chosen) = ranked(&list, &retired.gpu_types, &[]);
        assert_eq!(preferred.len(), 1);
        assert!(chosen.is_none());
    }
}
