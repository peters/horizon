//! Where a new cloud lives: any data center, a region, or one exact data center, each
//! with live stock. A cloud's workspace stays where it first starts, so the choice
//! outlives it.
use super::{
    super::prices::{State, region_name},
    pricing::{button, option, requested_gpus},
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
    compatible: bool,
}

/// Every allowed data center, including places that cannot hold this workspace volume.
fn candidates<'a>(prices: &State, list: &'a PriceList, gpu_types: &[String], profile: &Profile) -> Vec<Candidate<'a>> {
    let size = if profile.gpu {
        None
    } else {
        prices.displayed_size(profile, (profile.cpu, profile.memory_gb))
    };
    let mut candidates: Vec<_> = list
        .data_centers
        .iter()
        .map(|center| {
            let compatible = profile.gpu || center.holds(profile.storage.volume_tier);
            let here = std::slice::from_ref(&center.id);
            let stocked = if !compatible {
                Some(false)
            } else if profile.gpu {
                let mut eligible = gpu_types
                    .iter()
                    .filter(|gpu| {
                        profile
                            .min_gpu_memory_gb
                            .is_none_or(|minimum| list.gpu(gpu).is_some_and(|gpu| gpu.memory_gb >= minimum))
                    })
                    .peekable();
                eligible
                    .peek()
                    .is_some()
                    .then(|| eligible.any(|gpu| list.gpu_availability(gpu, here) != Availability::None))
            } else {
                match size {
                    Some(Ok(size)) => Some(size.count(here) > 0),
                    Some(Err(_)) => None,
                    None => {
                        return Candidate {
                            center,
                            region: region_name(&center.region),
                            stock: Stock::Checking,
                            compatible,
                        };
                    }
                }
            };
            Candidate {
                center,
                region: region_name(&center.region),
                compatible,
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
        if candidate.compatible {
            region.data_centers.push(candidate.center.id.clone());
            region.in_stock = region.in_stock.with(candidate.stock.into());
        }
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

/// Any data center, a region, or one exact data center, all with live stock for the
/// chosen worker. Sold-out places stay selectable. Returns a newly chosen placement.
pub(super) fn field(ui: &mut Ui, prices: &State, profile: &Profile, current: &Placement) -> Option<Placement> {
    let (list, preferences) = &prices.list.as_ref()?.value;
    let candidates = candidates(prices, list, requested_gpus(preferences, current), profile);
    if candidates.is_empty() {
        ui.label(
            RichText::new("No allowed data center can hold this worker's workspace.")
                .size(13.0)
                .color(theme::FG_SOFT()),
        );
        return None;
    }
    let regions = regions(&candidates);
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        let total = regions
            .iter()
            .fold(Count::Known(0), |total, region| total.with(region.in_stock));
        let (detail, tint) = stock_label(total);
        if option(ui, "Any data center", current.is_any(), Some(&detail), Some(tint)) {
            chosen = Some(Placement {
                gpu_types: current.gpu_types.clone(),
                ..Placement::default()
            });
        }
        for region in &regions {
            let selected = !region.data_centers.is_empty() && current.data_centers == region.data_centers;
            let (detail, tint) = stock_label(region.in_stock);
            if ui
                .add_enabled(
                    !region.data_centers.is_empty(),
                    button(&region.name, selected, Some(&detail), Some(tint)),
                )
                .clicked()
            {
                chosen = Some(Placement {
                    cpu_types: Vec::new(),
                    region: Some(region.name.clone()),
                    data_centers: region.data_centers.clone(),
                    gpu_types: current.gpu_types.clone(),
                });
            }
        }
    });
    data_centers(ui, &candidates, &regions, current, &mut chosen);
    let excluded = list
        .regions
        .keys()
        .filter(|id| !list.data_centers.iter().any(|center| center.id == **id))
        .count();
    let mut note = where_it_lives(current);
    if excluded > 0 {
        use std::fmt::Write as _;
        let _ = write!(note, " Cloud settings exclude {excluded} other data centers.");
    }
    ui.label(RichText::new(note).size(12.0).color(theme::FG_DIM()));
    chosen.filter(|placement| placement != current)
}

fn data_centers(
    ui: &mut Ui,
    candidates: &[Candidate<'_>],
    regions: &[Region],
    current: &Placement,
    chosen: &mut Option<Placement>,
) {
    let unavailable = candidates.iter().filter(|candidate| !candidate.compatible).count();
    ui.small(format!(
        "{} data centers · {unavailable} unavailable for this storage type",
        candidates.len()
    ));
    for region in regions {
        ui.add_space(4.0);
        ui.label(RichText::new(&region.name).size(12.0).color(theme::FG_SOFT()));
        ui.horizontal_wrapped(|ui| {
            for candidate in candidates.iter().filter(|candidate| candidate.region == region.name) {
                let selected = matches!(current.data_centers.as_slice(), [id] if id == &candidate.center.id);
                let detail = if candidate.compatible {
                    candidate.region.as_str()
                } else {
                    "Storage unavailable"
                };
                if ui
                    .add_enabled(candidate.compatible, |ui: &mut Ui| {
                        chip(
                            ui,
                            &candidate.center.id,
                            detail,
                            candidate.compatible.then_some(candidate.stock),
                            selected,
                        )
                    })
                    .clicked()
                {
                    *chosen = Some(Placement {
                        cpu_types: Vec::new(),
                        region: Some(candidate.region.clone()),
                        data_centers: vec![candidate.center.id.clone()],
                        gpu_types: current.gpu_types.clone(),
                    });
                }
            }
        });
    }
}

pub(super) fn where_it_lives(placement: &Placement) -> String {
    let place = match (placement.data_centers.as_slice(), placement.region.as_deref()) {
        ([], _) => return "Horizon picks a data center with stock. The workspace stays there, and a stopped cloud resumes there.".to_owned(),
        ([one], _) => one.clone(),
        (_, Some(region)) => region.to_owned(),
        (_, None) => "the chosen data centers".to_owned(),
    };
    format!("The workspace stays in {place}, and a stopped cloud resumes there.")
}

/// The stock of the chosen worker where `placement` allows: `None` while any place in
/// scope is still being checked or unknown and none has stock.
pub(super) fn in_stock(prices: &State, profile: &Profile, placement: &Placement) -> Option<bool> {
    let (list, preferences) = &prices.list.as_ref()?.value;
    let candidates = candidates(prices, list, requested_gpus(preferences, placement), profile);
    let scoped = candidates
        .iter()
        .filter(|candidate| candidate.compatible)
        .filter(|candidate| placement.is_any() || placement.data_centers.contains(&candidate.center.id));
    let total = scoped.fold(Count::Known(0), |total, candidate| total.with(candidate.stock.into()));
    match total {
        Count::Known(count) => Some(count > 0),
        Count::Checking | Count::Unknown => None,
    }
}

/// A compact choice with a title, a line under it and an optional stock dot, painted at
/// its own size.
pub(super) fn chip(ui: &mut Ui, id: &str, region: &str, stock: Option<Stock>, selected: bool) -> egui::Response {
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
    let stroke = if response.has_focus() {
        egui::Stroke::new(1.5, theme::FG())
    } else {
        stroke
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
            ui.is_enabled(),
            selected,
            format!("{id}, {region}{stock}"),
        )
    });
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn center(id: &str, region: &str, storage: bool) -> DataCenter {
        DataCenter {
            id: id.into(),
            region: region.into(),
            workspace_storage: storage,
            high_performance_storage: false,
            cpus: Vec::new(),
            gpus: vec![("l4".into(), Availability::High)],
        }
    }

    fn candidate(center: &DataCenter, stock: Stock) -> Candidate<'_> {
        Candidate {
            region: region_name(&center.region),
            center,
            stock,
            compatible: true,
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

    fn choose_sold_out(current: &Placement, label: &str) -> Placement {
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
                    selected = field(ui, &prices, &profile, current);
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
    fn many_data_centers_wrap_inside_the_picker_column() {
        use crate::test_egui::DiscardTextures;
        let (mut state, profile) = fixture();
        let list = &mut state.list.as_mut().unwrap().value.0;
        list.data_centers = (0..30)
            .map(|index| center(&format!("EU-{index}"), "EUROPE", true))
            .collect();
        let ctx = egui::Context::default();
        let mut width = 0.0;
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                ui.set_width(640.0);
                field(ui, &state, &profile, &Placement::default());
                width = ui.min_rect().width();
            })
            .discard_textures();
        assert!(width <= 640.0, "data centers expanded the column to {width}");
    }

    #[test]
    fn sold_out_data_centers_and_regions_are_visible_and_selectable() {
        let europe = choose_sold_out(&Placement::default(), "Europe\nnone in stock");
        assert_eq!(europe.data_centers, ["EU-2"]);
        // Exact data centers are visible before choosing a region and stay visible afterward.
        let labels = |current: &Placement| {
            use crate::test_egui::DiscardTextures;
            let (prices, profile) = fixture();
            egui::Context::default()
                .run_ui(egui::RawInput::default(), |ui| {
                    let _ = field(ui, &prices, &profile, current);
                })
                .discard_textures()
                .shapes
                .into_iter()
                .filter_map(|shape| match shape.shape {
                    egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert!(labels(&Placement::default()).iter().any(|label| label == "EU-2"));
        assert!(labels(&europe).iter().any(|label| label == "EU-2"));
        assert!(labels(&europe).iter().any(|label| label == "US-1"));
        let exact = choose_sold_out(&Placement::default(), "EU-2");
        assert_eq!(exact.data_centers, ["EU-2"]);
        let any = choose_sold_out(&europe, "Any data center\n1 in stock");
        assert!(any.is_any());
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
        assert_eq!(cpu.len(), 2);
        assert!(!cpu[0].compatible);
        assert_eq!(cpu[1].center.id, "US-1");
        assert!(cpu[1].compatible);
    }

    #[test]
    fn premium_placement_and_watch_use_exact_tier_stock() {
        use horizon_core::cloud_runtime::prices::{Availability, SizeAvailability, watch::Selection};
        let (mut prices, mut profile) = fixture();
        profile.gpu = false;
        profile.storage.volume_tier = serde_json::from_value(serde_json::json!("HIGH_PERFORMANCE")).unwrap();
        let (mut list, preferences) = prices.list.as_ref().unwrap().value.clone();
        list.data_centers.push(center("EU-3", "EUROPE", false));
        for center in &mut list.data_centers {
            center.high_performance_storage = center.id != "US-1";
        }
        let available = SizeAvailability {
            centers: vec![("EU-2".into(), Availability::High), ("EU-3".into(), Availability::None)],
        };
        prices.answered(list.clone(), preferences, vec![(profile.clone(), available.clone())]);
        let choices = candidates(&prices, &list, &[], &profile);
        assert_eq!(
            choices
                .iter()
                .filter(|entry| entry.compatible)
                .map(|entry| (entry.center.id.as_str(), entry.stock))
                .collect::<Vec<_>>(),
            [("EU-2", Stock::Yes), ("EU-3", Stock::No)]
        );
        let chosen = Selection::new(
            profile,
            Placement {
                data_centers: vec!["EU-2".into()],
                ..Placement::default()
            },
        )
        .unwrap();
        assert!(!chosen.available(&list, None));
        assert!(chosen.available(&list, Some(&available)));
        assert!(!chosen.available(&list, Some(&SizeAvailability { centers: Vec::new() })));
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
            cpu_types: Vec::new(),
            region: Some("Europe".into()),
            data_centers: vec!["EU-RO-1".into(), "EUR-IS-1".into()],
            gpu_types: Vec::new(),
        };
        assert_eq!(
            where_it_lives(&europe),
            "The workspace stays in Europe, and a stopped cloud resumes there."
        );
        let one = Placement {
            cpu_types: Vec::new(),
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
