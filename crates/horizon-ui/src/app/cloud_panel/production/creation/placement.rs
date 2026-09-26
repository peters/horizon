//! Where a new cloud lives: a region with live stock, or one data center under
//! Advanced. A cloud's workspace stays where it first starts, so the choice outlives it.
use super::{super::prices::State, pricing::option};
use crate::theme;
use egui::{Color32, CornerRadius, FontId, RichText, Sense, Ui, Vec2};
use horizon_core::{
    cloud_panel::Placement,
    cloud_runtime::prices::{Availability, DataCenter, Preferences, PriceList, Profile},
};
use std::collections::BTreeMap;

/// A data center this cloud can use, and whether it has the chosen size or one of the
/// preferred GPUs in stock: `None` while that is unknown.
struct Candidate<'a> {
    center: &'a DataCenter,
    region: String,
    stocked: Option<bool>,
}

/// The data centers `profile` can use: CPU clouds need workspace storage there.
fn candidates<'a>(
    prices: &State,
    list: &'a PriceList,
    preferences: &Preferences,
    profile: &Profile,
) -> Vec<Candidate<'a>> {
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
                    preferences
                        .gpu_types
                        .iter()
                        .any(|gpu| list.gpu_availability(gpu, here) != Availability::None),
                )
            } else {
                match size {
                    Some(Ok(size)) => Some(size.count(here) > 0),
                    None | Some(Err(_)) => None,
                }
            };
            Candidate {
                center,
                region: region_name(&center.region),
                stocked,
            }
        })
        .collect()
}

/// A region's data centers and how many have stock: `None` while any is unknown.
struct Region {
    name: String,
    data_centers: Vec<String>,
    in_stock: Option<usize>,
}

fn regions(candidates: &[Candidate<'_>]) -> Vec<Region> {
    let mut regions: BTreeMap<&str, Region> = BTreeMap::new();
    for candidate in candidates {
        let region = regions.entry(&candidate.region).or_insert_with(|| Region {
            name: candidate.region.clone(),
            data_centers: Vec::new(),
            in_stock: Some(0),
        });
        region.data_centers.push(candidate.center.id.clone());
        region.in_stock = region
            .in_stock
            .zip(candidate.stocked)
            .map(|(count, stocked)| count + usize::from(stocked));
    }
    let mut regions: Vec<Region> = regions.into_values().collect();
    for region in &mut regions {
        region.data_centers.sort();
    }
    regions
}

/// The provider's region as people say it, such as `NORTH_AMERICA` as North America.
fn region_name(region: &str) -> String {
    if region.is_empty() {
        return "Other".to_owned();
    }
    region
        .split('_')
        .map(|word| {
            let lower = word.to_ascii_lowercase();
            let mut letters = lower.chars();
            letters
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + letters.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn stock_label(in_stock: Option<usize>) -> (String, Color32) {
    match in_stock {
        None => ("checking stock".to_owned(), theme::FG_DIM()),
        Some(0) => ("none in stock".to_owned(), theme::PALETTE_RED()),
        Some(count) => (format!("{count} in stock"), theme::PALETTE_GREEN()),
    }
}

/// Region buttons with live stock, once prices are known and there is a choice to make.
/// Returns a newly chosen placement.
pub(super) fn region_field(ui: &mut Ui, prices: &State, profile: &Profile, current: &Placement) -> Option<Placement> {
    let (list, preferences) = &prices.list.as_ref()?.value;
    let candidates = candidates(prices, list, preferences, profile);
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
            .try_fold(0, |total, region| Some(total + region.in_stock?));
        let (detail, tint) = stock_label(total);
        if option(ui, "Any region", current.is_any(), Some(&detail), Some(tint)) {
            chosen = Some(Placement::default());
        }
        for region in &regions {
            let selected = current.data_centers == region.data_centers;
            let (detail, tint) = stock_label(region.in_stock);
            // A region known to be sold out stays visible but cannot be chosen.
            let enabled = selected || region.in_stock != Some(0);
            let clicked = ui
                .add_enabled_ui(enabled, |ui| {
                    option(ui, &region.name, selected, Some(&detail), Some(tint))
                })
                .inner;
            if clicked {
                chosen = Some(Placement {
                    region: Some(region.name.clone()),
                    data_centers: region.data_centers.clone(),
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
    let candidates = candidates(prices, list, preferences, profile);
    let shown: Vec<&Candidate<'_>> = candidates
        .iter()
        .filter(|candidate| candidate.stocked != Some(false) || current.data_centers == [candidate.center.id.clone()])
        .collect();
    if shown.is_empty() {
        return None;
    }
    ui.add_space(6.0);
    ui.label(RichText::new("Data center").size(14.0).strong().color(theme::FG()));
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        for candidate in &shown {
            let selected = current.data_centers == [candidate.center.id.clone()];
            if chip(ui, &candidate.center.id, &candidate.region, candidate.stocked, selected) {
                chosen = Some(Placement {
                    region: Some(candidate.region.clone()),
                    data_centers: vec![candidate.center.id.clone()],
                });
            }
        }
    });
    let hidden = candidates.len() - shown.len();
    if hidden > 0 {
        ui.small(format!("{hidden} more without stock for this cloud right now."));
    }
    chosen.filter(|placement| placement != current)
}

/// A compact data center choice with a stock dot, painted at its own size.
fn chip(ui: &mut Ui, id: &str, region: &str, stocked: Option<bool>, selected: bool) -> bool {
    const DOT: f32 = 6.0;
    let color = match stocked {
        Some(true) => theme::PALETTE_GREEN(),
        Some(false) => theme::PALETTE_RED(),
        None => theme::FG_DIM(),
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
    let stock = match stocked {
        Some(true) => "in stock",
        Some(false) => "out of stock",
        None => "stock unknown",
    };
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            true,
            selected,
            format!("{id}, {region}, {stock}"),
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

    fn candidate(center: &DataCenter, stocked: Option<bool>) -> Candidate<'_> {
        Candidate {
            region: region_name(&center.region),
            center,
            stocked,
        }
    }

    #[test]
    fn regions_read_as_people_say_them() {
        assert_eq!(region_name("NORTH_AMERICA"), "North America");
        assert_eq!(region_name("EUROPE"), "Europe");
        assert_eq!(region_name(""), "Other");
    }

    #[test]
    fn regions_group_data_centers_and_count_stock_once_known() {
        let (eu, eu2, us) = (
            center("EU-RO-1", "EUROPE", true),
            center("EUR-IS-1", "EUROPE", true),
            center("US-MO-2", "NORTH_AMERICA", true),
        );
        let candidates = [
            candidate(&eu, Some(true)),
            candidate(&eu2, Some(false)),
            candidate(&us, None),
        ];
        let regions = regions(&candidates);
        let summary: Vec<(&str, usize, Option<usize>)> = regions
            .iter()
            .map(|region| (region.name.as_str(), region.data_centers.len(), region.in_stock))
            .collect();
        assert_eq!(summary, [("Europe", 2, Some(1)), ("North America", 1, None)]);
    }

    #[test]
    fn the_dialog_says_where_the_workspace_stays() {
        assert!(where_it_lives(&Placement::default()).starts_with("Horizon picks a data center with stock."));
        let europe = Placement {
            region: Some("Europe".into()),
            data_centers: vec!["EU-RO-1".into(), "EUR-IS-1".into()],
        };
        assert_eq!(
            where_it_lives(&europe),
            "The workspace stays in Europe, and a stopped cloud resumes there."
        );
        let one = Placement {
            region: Some("Europe".into()),
            data_centers: vec!["EU-RO-1".into()],
        };
        assert_eq!(
            where_it_lives(&one),
            "The workspace stays in EU-RO-1, and a stopped cloud resumes there."
        );
    }
}
