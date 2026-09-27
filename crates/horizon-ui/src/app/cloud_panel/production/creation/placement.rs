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
    list.data_centers
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
        .collect()
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
            // A region known to be sold out stays visible but cannot be chosen; one whose
            // stock is unknown can, since the provider may still place the cloud there.
            let enabled = selected || region.in_stock != Count::Known(0);
            let clicked = ui
                .add_enabled_ui(enabled, |ui| {
                    option(ui, &region.name, selected, Some(&detail), Some(tint))
                })
                .inner;
            if clicked {
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

/// One data center, for the Advanced section: those with stock, and the chosen one.
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
    let shown: Vec<&Candidate<'_>> = candidates
        .iter()
        .filter(|candidate| candidate.stock != Stock::No || current.data_centers == [candidate.center.id.clone()])
        .collect();
    ui.add_space(6.0);
    ui.label(RichText::new("Data center").size(14.0).strong().color(theme::FG()));
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        for candidate in &shown {
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
    if let Some(note) = hidden_note(candidates.len() - shown.len(), shown.is_empty()) {
        ui.small(note);
    }
    chosen.filter(|placement| placement != current)
}

fn hidden_note(hidden: usize, none_shown: bool) -> Option<String> {
    match (hidden, none_shown) {
        (0, _) => None,
        (1, true) => Some("The one data center for this cloud is out of stock right now.".to_owned()),
        (hidden, true) => Some(format!(
            "All {hidden} data centers for this cloud are out of stock right now."
        )),
        (hidden, false) => Some(format!("{hidden} more without stock for this cloud right now.")),
    }
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
    fn a_sold_out_cloud_still_says_how_many_data_centers_it_could_use() {
        assert_eq!(hidden_note(0, false), None);
        assert_eq!(
            hidden_note(13, false).as_deref(),
            Some("13 more without stock for this cloud right now.")
        );
        assert_eq!(
            hidden_note(3, true).as_deref(),
            Some("All 3 data centers for this cloud are out of stock right now.")
        );
        assert_eq!(
            hidden_note(1, true).as_deref(),
            Some("The one data center for this cloud is out of stock right now.")
        );
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
