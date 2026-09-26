//! Worker size choices with their prices, and a price card with live stock, for New cloud.
use super::super::{
    machine_size::{self, Choice, Size},
    prices::{FRESH, State},
};
use crate::theme;
use egui::{
    Align, Button, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, Sense, Stroke, TextFormat, Ui, Vec2,
    WidgetInfo, WidgetType, text::LayoutJob,
};
use horizon_core::cloud_runtime::prices::{self, Availability, GpuPrice, Preferences, PriceList, Profile};

/// Size buttons for a CPU profile, each with its hourly price, then the price card.
/// Returns a newly chosen size and whether prices should be fetched again.
pub(super) fn size_field(ui: &mut Ui, prices: &State, profile: &Profile, current: Size) -> (Option<Size>, bool) {
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
                if option(ui, &choice, from.map(|low| format!("from {}/h", money(low)))) {
                    chosen = Some(choice.size);
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            for choice in machine_size::memory_choices(current, disk) {
                let price = list.and_then(|(list, preferences)| hourly(list, preferences, profile, choice.size));
                if option(ui, &choice, price.map(|(low, high)| format!("{}/h", range(low, high)))) {
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
    ui.add_space(4.0);
    let sized = Profile {
        cpu: current.0,
        memory_gb: current.1,
        ..profile.clone()
    };
    (chosen.filter(|size| *size != current), card(ui, prices, &sized))
}

fn option(ui: &mut Ui, choice: &Choice, price: Option<String>) -> bool {
    let mut job = LayoutJob::default();
    let format = |size: f32, color: Color32| TextFormat {
        font_id: FontId::proportional(size),
        color,
        ..TextFormat::default()
    };
    job.append(&choice.label, 0.0, format(13.0, theme::FG()));
    let two_lines = price.is_some();
    if let Some(price) = price {
        let color = if choice.selected {
            theme::ACCENT()
        } else {
            theme::FG_DIM()
        };
        job.append(&format!("\n{price}"), 0.0, format(11.0, color));
    }
    ui.add(
        Button::new(job)
            .selected(choice.selected)
            .min_size(Vec2::new(0.0, if two_lines { 42.0 } else { 30.0 }))
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

/// The price card, once a price check has started. Returns whether prices should be
/// fetched again.
fn card(ui: &mut Ui, prices: &State, profile: &Profile) -> bool {
    let mut refresh = false;
    if prices.list.is_none() && prices.list_error.is_none() && !prices.loading() {
        return refresh;
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
                        gpu_summary(ui, list, preferences);
                    } else {
                        cpu_summary(ui, prices, list, preferences, profile);
                    }
                    ui.add_space(10.0);
                    refresh = footer(ui, list.provider, fetched.at.elapsed(), prices.loading());
                }
                (None, Some(error)) => {
                    ui.label(
                        RichText::new("Prices are unavailable")
                            .size(14.0)
                            .color(theme::FG_SOFT()),
                    );
                    ui.label(RichText::new(error).size(11.5).color(theme::FG_DIM()));
                    refresh = ui.add(Button::new("Try again").corner_radius(8)).clicked();
                }
                (None, None) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Checking RunPod prices and stock…").color(theme::FG_DIM()));
                    });
                }
            }
        });
    refresh
}

fn cpu_summary(ui: &mut Ui, prices: &State, list: &PriceList, preferences: &Preferences, profile: &Profile) {
    let flavors = prices::requested_flavors(profile, preferences);
    let Some((low, high)) = list.cpu_hourly(&flavors, profile.cpu) else {
        ui.label(RichText::new("RunPod publishes no price for this size").color(theme::FG_SOFT()));
        return;
    };
    let stock = match prices.size(profile, (profile.cpu, profile.memory_gb)) {
        None => Stock::Checking,
        Some(Ok(size)) => Stock::Known(size.best, Some(size.centers)),
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
    let storage = f64::from(profile.storage.volume_gb) * list.storage_gb_month;
    estimates(
        ui,
        (low, high),
        Some(&(
            format!("{}/mo", money(storage)),
            format!(
                "{} GB network volume. Storage is billed while the worker is stopped too.",
                profile.storage.volume_gb
            ),
        )),
    );
}

fn gpu_summary(ui: &mut Ui, list: &PriceList, preferences: &Preferences) {
    let preferred: Vec<&GpuPrice> = preferences.gpu_types.iter().filter_map(|id| list.gpu(id)).collect();
    let chosen = preferred
        .iter()
        .copied()
        .find(|gpu| gpu.availability != Availability::None);
    match chosen {
        Some(gpu) => {
            headline(ui, &money(gpu.hourly), &Stock::Known(gpu.availability, None));
            ui.label(
                RichText::new(format!(
                    "{} · {} GB · first preferred GPU in stock · Secure Cloud",
                    gpu.name, gpu.memory_gb
                ))
                .size(12.0)
                .color(theme::FG_SOFT()),
            );
        }
        None => {
            ui.label(
                RichText::new("None of your preferred GPUs is in stock")
                    .size(16.0)
                    .strong()
                    .color(theme::PALETTE_RED()),
            );
        }
    }
    if !preferred.is_empty() {
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
                for (rank, gpu) in preferred.iter().enumerate() {
                    let first = chosen.is_some_and(|chosen| chosen.id == gpu.id);
                    let color = if first { theme::FG() } else { theme::FG_SOFT() };
                    let marker = if first { theme::ACCENT() } else { theme::FG_DIM() };
                    ui.label(RichText::new(format!("{}", rank + 1)).size(12.0).strong().color(marker));
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
                    pill(ui, &Stock::Known(gpu.availability, None), true);
                    ui.end_row();
                }
            });
    }
    if preferred.is_empty() {
        ui.label(
            RichText::new("Add preferred GPU types in Cloud settings to see their prices.")
                .size(12.0)
                .color(theme::FG_DIM()),
        );
    } else if chosen.is_none()
        && let Some(cheapest) = list.cheapest_available_gpu()
    {
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!(
                "Cheapest in stock now: {} · {} GB · {}/h. Change GPU preferences in Cloud settings.",
                cheapest.name,
                cheapest.memory_gb,
                money(cheapest.hourly)
            ))
            .size(12.0)
            .color(theme::FG_SOFT()),
        );
    }
    if let Some(gpu) = chosen {
        estimates(ui, (gpu.hourly, gpu.hourly), None);
    }
}

enum Stock {
    Checking,
    Known(Availability, Option<usize>),
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
    let (text, color, hover) = match stock {
        Stock::Checking => ("Checking stock".to_owned(), theme::FG_DIM(), None),
        Stock::Unknown(error) => ("Stock unknown".to_owned(), theme::FG_DIM(), Some(error.clone())),
        Stock::Known(level, centers) => {
            let (text, color) = match level {
                Availability::High | Availability::Medium => ("In stock", theme::PALETTE_GREEN()),
                Availability::Low => ("Low stock", theme::PALETTE_YELLOW()),
                Availability::None => ("Out of stock", theme::PALETTE_RED()),
            };
            (text.to_owned(), color, centers.map(in_centers))
        }
    };
    let description = hover
        .as_ref()
        .map_or_else(|| text.clone(), |hover| format!("{text}. {hover}"));
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
    if let Some(hover) = hover {
        response.on_hover_text(hover);
    }
}

fn in_centers(centers: usize) -> String {
    match centers {
        0 => "Out of stock in every allowed data center".to_owned(),
        1 => "In stock in 1 allowed data center".to_owned(),
        count => format!("In stock in {count} allowed data centers"),
    }
}

/// Totals for `hourly`, the lowest and highest price a deployment may be charged.
fn estimates(ui: &mut Ui, hourly: (f64, f64), storage: Option<&(String, String)>) {
    ui.add_space(10.0);
    let (line, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(line, 0.0, theme::BORDER_SUBTLE());
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 32.0;
        let (low, high) = hourly;
        stat(ui, "8 HOURS", &range(low * 8.0, high * 8.0), None);
        stat(ui, "24 HOURS", &range(low * 24.0, high * 24.0), None);
        if let Some((value, hover)) = storage {
            stat(ui, "STORAGE", value, Some(hover));
        }
    });
}

fn stat(ui: &mut Ui, label: &str, value: &str, hover: Option<&str>) {
    let response = ui
        .vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(label).size(10.5).color(theme::FG_DIM()));
            ui.label(RichText::new(value).size(16.0).color(theme::FG()));
        })
        .response;
    if let Some(hover) = hover {
        response.on_hover_text(hover);
    }
}

/// Returns whether prices should be fetched again.
fn footer(ui: &mut Ui, provider: &str, age: std::time::Duration, loading: bool) -> bool {
    let mut refresh = false;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{provider} list prices · updated {}", ago(age)))
                .size(11.0)
                .color(theme::FG_DIM()),
        )
        .on_hover_text(format!(
            "Prices refresh every {} minutes while this dialog is open.",
            FRESH.as_secs() / 60
        ));
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

fn money(value: f64) -> String {
    if value >= 100.0 {
        format!("${value:.0}")
    } else {
        format!("${value:.2}")
    }
}

/// One amount when both ends read the same once rounded, otherwise a range.
fn range(low: f64, high: f64) -> String {
    let (low, high) = (money(low), money(high));
    if low == high {
        low
    } else {
        format!("{low}–{}", high.trim_start_matches('$'))
    }
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
    fn prices_read_naturally() {
        assert_eq!(money(0.24), "$0.24");
        assert_eq!(money(5.76), "$5.76");
        assert_eq!(money(242.0), "$242");
        assert_eq!(range(0.24, 0.24), "$0.24");
        assert_eq!(range(0.24, 0.28), "$0.24–0.28");
        assert_eq!(range(0.244, 0.248), "$0.24–0.25");
        assert_eq!(range(0.241, 0.244), "$0.24");
        assert_eq!(range(1.92, 2.24), "$1.92–2.24");
        assert_eq!(ago(std::time::Duration::from_secs(20)), "just now");
        assert_eq!(ago(std::time::Duration::from_secs(150)), "2 min ago");
    }

    #[test]
    fn stock_details_name_the_data_centers() {
        assert_eq!(in_centers(0), "Out of stock in every allowed data center");
        assert_eq!(in_centers(1), "In stock in 1 allowed data center");
        assert_eq!(in_centers(3), "In stock in 3 allowed data centers");
    }
}
