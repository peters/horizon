//! Where a new cloud lives: a region with live stock, or one data center under
//! Advanced. A cloud's workspace stays where it first starts, so the choice outlives it.
use super::{
    super::prices::{State, region_name},
    pricing::{option, requested_gpus},
};
use crate::theme;
use egui::{Color32, CornerRadius, FontId, RichText, Sense, Ui, Vec2};
use horizon_core::{
    cloud_panel::Placement,
    cloud_runtime::prices::{Availability, DataCenter, PriceList, Profile},
};
use std::collections::BTreeMap;

/// Whether a data center has the chosen size, or one of the requested GPUs, in stock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stock {
    Yes,
    No,
    /// A stock check is running.
    Checking,
    /// The last stock check failed; Refresh asks again.
    Unknown,
}

/// A data center this cloud can use, and its stock.
struct Candidate<'a> {
    center: &'a DataCenter,
    region: String,
    stock: Stock,
}

/// The data centers `profile` can use: CPU clouds need workspace storage there.
/// GPU profiles count data centers with any of `gpu_types` in stock.
fn candidates<'a>(prices: &State, list: &'a PriceList, gpu_types: &[String], profile: &Profile) -> Vec<Candidate<'a>> {
    let size = if profile.gpu {
        None
    } else {
        prices.size(profile, (profile.cpu, profile.memory_gb))
    };
    let mut candidates: Vec<_> = list
        .data_centers
        .iter()
        .filter(|center| profile.gpu || center.workspace_storage)
        .map(|center| {
            let here = std::slice::from_ref(&center.id);
            let stocked = if profile.gpu {
                Some(
                    gpu_types
                        .iter()
                        .any(|gpu| list.gpu_availability(gpu, here) != Availability::None),
                )
            } else {
                match size {
                    Some(Ok(size)) => Some(size.count(here) > 0),
                    Some(Err(_)) => None,
                    None => {
                        return Candidate {
                            center,
                            region: region_name(&center.region),
                            stock: Stock::Checking,
                        };
                    }
                }
            };
            Candidate {
                center,
                region: region_name(&center.region),
                stock: match stocked {
                    Some(true) => Stock::Yes,
                    Some(false) => Stock::No,
                    None => Stock::Unknown,
                },
            }
        })
        .collect();
    candidates.sort_by(|a, b| a.region.cmp(&b.region).then_with(|| a.center.id.cmp(&b.center.id)));
    candidates
}

/// How many data centers have stock, once every one is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Count {
    Known(usize),
    Checking,
    Unknown,
}

impl Count {
    /// Unknown wins over checking, which wins over a known count.
    fn with(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (Self::Checking, _) | (_, Self::Checking) => Self::Checking,
            (Self::Known(a), Self::Known(b)) => Self::Known(a + b),
        }
    }
}

impl From<Stock> for Count {
    fn from(stock: Stock) -> Self {
        match stock {
            Stock::Yes => Self::Known(1),
            Stock::No => Self::Known(0),
            Stock::Checking => Self::Checking,
            Stock::Unknown => Self::Unknown,
        }
    }
}

/// A region's data centers and how many have stock.
struct Region {
    name: String,
    data_centers: Vec<String>,
    in_stock: Count,
}

fn regions(candidates: &[Candidate<'_>]) -> Vec<Region> {
    let mut regions: BTreeMap<&str, Region> = BTreeMap::new();
    for candidate in candidates {
        let region = regions.entry(&candidate.region).or_insert_with(|| Region {
            name: candidate.region.clone(),
            data_centers: Vec::new(),
            in_stock: Count::Known(0),
        });
        region.data_centers.push(candidate.center.id.clone());
        region.in_stock = region.in_stock.with(candidate.stock.into());
    }
    let mut regions: Vec<Region> = regions.into_values().collect();
    for region in &mut regions {
        region.data_centers.sort();
    }
    regions
}

fn stock_label(in_stock: Count) -> (String, Color32) {
    match in_stock {
        Count::Checking => ("checking stock".to_owned(), theme::FG_DIM()),
        Count::Unknown => ("stock unknown".to_owned(), theme::FG_DIM()),
        Count::Known(0) => ("none in stock".to_owned(), theme::PALETTE_RED()),
        Count::Known(count) => (format!("{count} in stock"), theme::PALETTE_GREEN()),
    }
}

/// Region buttons with live stock, once prices are known and there is a choice to make.
/// Returns a newly chosen placement.
pub(super) fn region_field(ui: &mut Ui, prices: &State, profile: &Profile, current: &Placement) -> Option<Placement> {
    let (list, preferences) = &prices.list.as_ref()?.value;
    let candidates = candidates(prices, list, requested_gpus(preferences, current), profile);
    let regions = regions(&candidates);
    if regions.len() < 2 && current.is_any() {
        return None;
    }
    ui.add_space(6.0);
    ui.label(RichText::new("Region").size(14.0).strong().color(theme::FG()));
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        let total = regions
            .iter()
            .fold(Count::Known(0), |total, region| total.with(region.in_stock));
        let (detail, tint) = stock_label(total);
        if option(ui, "Any region", current.is_any(), Some(&detail), Some(tint)) {
            chosen = Some(Placement {
                gpu_types: current.gpu_types.clone(),
                ..Placement::default()
            });
        }
        for region in &regions {
            let selected = current.data_centers == region.data_centers;
            let (detail, tint) = stock_label(region.in_stock);
            if option(ui, &region.name, selected, Some(&detail), Some(tint)) {
                chosen = Some(Placement {
                    region: Some(region.name.clone()),
                    data_centers: region.data_centers.clone(),
                    gpu_types: current.gpu_types.clone(),
                });
            }
        }
    });
    ui.small(where_it_lives(current));
    chosen.filter(|placement| placement != current)
}

fn where_it_lives(placement: &Placement) -> String {
    let place = match (placement.data_centers.as_slice(), placement.region.as_deref()) {
        ([], _) => return "Horizon picks a data center with stock. The workspace stays there, and a stopped cloud resumes there.".to_owned(),
        ([one], _) => one.clone(),
        (_, Some(region)) => region.to_owned(),
        (_, None) => "the chosen data centers".to_owned(),
    };
    format!("The workspace stays in {place}, and a stopped cloud resumes there.")
}

/// Every compatible allowed data center, including sold-out choices, for Advanced.
/// Returns a newly chosen placement.
pub(super) fn data_center_field(
    ui: &mut Ui,
    prices: &State,
    profile: &Profile,
    current: &Placement,
) -> Option<Placement> {
    let (list, preferences) = &prices.list.as_ref()?.value;
    let candidates = candidates(prices, list, requested_gpus(preferences, current), profile);
    if candidates.is_empty() {
        return None;
    }
    ui.add_space(6.0);
    ui.label(RichText::new("Data center").size(14.0).strong().color(theme::FG()));
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(theme::PALETTE_GREEN(), "● In stock");
        ui.colored_label(theme::PALETTE_RED(), "● Out of stock");
        ui.colored_label(theme::FG_DIM(), "● Checking or unknown");
    });
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        for candidate in &candidates {
            let selected = current.data_centers == [candidate.center.id.clone()];
            if chip(
                ui,
                &candidate.center.id,
                &candidate.region,
                Some(candidate.stock),
                selected,
            ) {
                chosen = Some(Placement {
                    region: Some(candidate.region.clone()),
                    data_centers: vec![candidate.center.id.clone()],
                    gpu_types: current.gpu_types.clone(),
                });
            }
        }
    });
    ui.small("Out-of-stock locations stay selectable. Selecting one does not start a cloud.");
    let excluded = list
        .regions
        .keys()
        .filter(|id| !list.data_centers.iter().any(|center| center.id == **id))
        .count();
    if excluded > 0 {
        ui.small(format!(
            "Cloud settings exclude {excluded} other data centers from this list."
        ));
    }
    chosen.filter(|placement| placement != current)
}

/// A compact choice with a title, a line under it and an optional stock dot, painted at
/// its own size.
pub(super) fn chip(ui: &mut Ui, id: &str, region: &str, stock: Option<Stock>, selected: bool) -> bool {
    const DOT: f32 = 6.0;
    let color = match stock {
        Some(Stock::Yes) => theme::PALETTE_GREEN(),
        Some(Stock::No) => theme::PALETTE_RED(),
        Some(Stock::Checking | Stock::Unknown) => theme::FG_DIM(),
        None => Color32::TRANSPARENT,
    };
    let name = ui
        .painter()
        .layout_no_wrap(id.to_owned(), FontId::proportional(12.5), theme::FG());
    let place = ui
        .painter()
        .layout_no_wrap(region.to_owned(), FontId::proportional(10.5), theme::FG_DIM());
    let padding = Vec2::new(10.0, 6.0);
    let width = padding.x * 2.0 + DOT + 7.0 + name.size().x.max(place.size().x);
    let height = padding.y * 2.0 + name.size().y + place.size().y;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let (fill, stroke) = if selected {
        (
            theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.22),
            egui::Stroke::new(1.0, theme::ACCENT()),
        )
    } else if response.hovered() {
        (
            theme::blend(theme::PANEL_BG_ALT(), theme::FG(), 0.06),
            egui::Stroke::new(1.0, theme::BORDER_SUBTLE()),
        )
    } else {
        (theme::PANEL_BG_ALT(), egui::Stroke::new(1.0, theme::BORDER_SUBTLE()))
    };
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(8), fill, stroke, egui::StrokeKind::Inside);
    let left = rect.left() + padding.x;
    painter.circle_filled(
        egui::pos2(left + DOT / 2.0, rect.top() + padding.y + name.size().y / 2.0),
        DOT / 2.0,
        color,
    );
    let text_left = left + DOT + 7.0;
    painter.galley(egui::pos2(text_left, rect.top() + padding.y), name.clone(), theme::FG());
    painter.galley(
        egui::pos2(text_left, rect.top() + padding.y + name.size().y),
        place,
        theme::FG_DIM(),
    );
    let stock = match stock {
        Some(Stock::Yes) => ", in stock",
        Some(Stock::No) => ", out of stock",
        Some(Stock::Checking) => ", checking stock",
        Some(Stock::Unknown) => ", stock unknown",
        None => "",
    };
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            true,
            selected,
            format!("{id}, {region}{stock}"),
        )
    });
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn center(id: &str, region: &str, storage: bool) -> DataCenter {
        DataCenter {
            id: id.into(),
            region: region.into(),
            workspace_storage: storage,
            gpus: vec![("l4".into(), Availability::High)],
        }
    }

    fn candidate(center: &DataCenter, stock: Stock) -> Candidate<'_> {
        Candidate {
            region: region_name(&center.region),
            center,
            stock,
        }
    }

    fn fixture() -> (State, Profile) {
        use super::super::super::prices::Fetched;
        use horizon_core::cloud_runtime::prices::{Preferences, RUNPOD_STORAGE};
        let profile = horizon_core::cloud_panel::CloudConfig::parse(
            "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n    gpu: true\n",
        ).unwrap().profiles["dev"].clone();
        let mut sold_out = center("EU-2", "EUROPE", false);
        sold_out.gpus.clear();
        let list = PriceList {
            provider: "RunPod",
            cpu: Vec::new(),
            gpus: Vec::new(),
            data_centers: vec![center("US-1", "NORTH_AMERICA", true), sold_out],
            regions: [("EU-2", "EUROPE"), ("US-1", "NORTH_AMERICA"), ("AS-3", "ASIA")]
                .into_iter()
                .map(|(id, region)| (id.to_owned(), region.to_owned()))
                .collect(),
            storage: RUNPOD_STORAGE,
        };
        let preferences = Preferences {
            gpu_types: vec!["l4".into()],
            ..Preferences::default()
        };
        let mut state = State::default();
        state.list = Some(Fetched {
            value: (list, preferences),
            at: std::time::Instant::now(),
        });
        (state, profile)
    }

    fn choose_sold_out(region: bool, label: &str) -> Placement {
        use crate::test_egui::DiscardTextures;
        let (prices, profile) = fixture();
        let ctx = egui::Context::default();
        let mut selected = None;
        let mut render = |events| {
            ctx.run_ui(
                egui::RawInput {
                    events,
                    ..egui::RawInput::default()
                },
                |ui| {
                    ui.set_width(600.0);
                    selected = if region {
                        region_field(ui, &prices, &profile, &Placement::default())
                    } else {
                        data_center_field(ui, &prices, &profile, &Placement::default())
                    };
                },
            )
            .discard_textures()
        };
        let output = render(Vec::new());
        let at = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.job.text == label => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()).center())
                }
                _ => None,
            })
            .unwrap();
        for pressed in [true, false] {
            let _ = render(vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
        }
        selected.unwrap()
    }

    #[test]
    fn sold_out_data_centers_and_regions_are_visible_and_selectable() {
        assert_eq!(choose_sold_out(false, "EU-2").data_centers, ["EU-2"]);
        assert_eq!(choose_sold_out(true, "Europe\nnone in stock").data_centers, ["EU-2"]);
    }

    #[test]
    fn choices_are_sorted_without_stock_filtering_and_cpu_requires_workspace_storage() {
        let (prices, mut profile) = fixture();
        let (list, preferences) = &prices.list.as_ref().unwrap().value;
        let gpu = candidates(&prices, list, &preferences.gpu_types, &profile);
        assert_eq!(
            gpu.iter()
                .map(|entry| (entry.center.id.as_str(), entry.stock))
                .collect::<Vec<_>>(),
            [("EU-2", Stock::No), ("US-1", Stock::Yes)]
        );
        profile.gpu = false;
        let cpu = candidates(&prices, list, &[], &profile);
        assert_eq!(cpu.len(), 1);
        assert_eq!(cpu[0].center.id, "US-1");
    }

    #[test]
    fn regions_group_data_centers_and_count_stock_once_known() {
        let (eu, eu2, us) = (
            center("EU-RO-1", "EUROPE", true),
            center("EUR-IS-1", "EUROPE", true),
            center("US-MO-2", "NORTH_AMERICA", true),
        );
        let candidates = [
            candidate(&eu, Stock::Yes),
            candidate(&eu2, Stock::No),
            candidate(&us, Stock::Checking),
        ];
        let regions = regions(&candidates);
        let summary: Vec<(&str, usize, Count)> = regions
            .iter()
            .map(|region| (region.name.as_str(), region.data_centers.len(), region.in_stock))
            .collect();
        assert_eq!(
            summary,
            [("Europe", 2, Count::Known(1)), ("North America", 1, Count::Checking)]
        );
        // A failed check reads as unknown rather than as a check still running.
        let failed = [candidate(&eu, Stock::Yes), candidate(&us, Stock::Unknown)];
        let total = super::regions(&failed)
            .iter()
            .fold(Count::Known(0), |total, region| total.with(region.in_stock));
        assert_eq!(total, Count::Unknown);
        assert_eq!(stock_label(Count::Unknown).0, "stock unknown");
        assert_eq!(Count::Checking.with(Count::Known(2)), Count::Checking);
    }

    #[test]
    fn the_dialog_says_where_the_workspace_stays() {
        assert!(where_it_lives(&Placement::default()).starts_with("Horizon picks a data center with stock."));
        let europe = Placement {
            region: Some("Europe".into()),
            data_centers: vec!["EU-RO-1".into(), "EUR-IS-1".into()],
            gpu_types: Vec::new(),
        };
        assert_eq!(
            where_it_lives(&europe),
            "The workspace stays in Europe, and a stopped cloud resumes there."
        );
        let one = Placement {
            region: Some("Europe".into()),
            data_centers: vec!["EU-RO-1".into()],
            gpu_types: Vec::new(),
        };
        assert_eq!(
            where_it_lives(&one),
            "The workspace stays in EU-RO-1, and a stopped cloud resumes there."
        );
    }
}
