//! The three starting points as cards, and every matching worker as a dense list.
use super::{Catalog, Production, widgets};
use crate::theme;
use egui::{Align2, FontId, Frame, RichText, Sense, Stroke, Ui};
use horizon_core::cloud_runtime::offers::Offer;

const PICKS: [&str; 3] = ["CHEAPEST", "BALANCED", "MOST POWERFUL"];

/// What a worker is called, and what it has, in the words a card uses.
fn title(offer: &Offer) -> (String, String) {
    if offer.kind == "gpu" {
        let memory = offer
            .gpu_memory_gb
            .map(|gb| format!("{gb} GB GPU memory"))
            .unwrap_or_default();
        (offer.name.clone(), format!("{} · {memory}", offer.provider))
    } else {
        let size = format!("{} vCPU · {} GB", offer.vcpu.unwrap_or(0), offer.memory_gb.unwrap_or(0));
        let family = offer.name.split(" · ").next().unwrap_or_default().to_owned();
        (
            size,
            format!(
                "{} · {family}{}",
                offer.provider,
                offer
                    .location
                    .as_ref()
                    .map(|place| format!(" · {place}"))
                    .unwrap_or_default()
            ),
        )
    }
}

fn price(offer: &Offer) -> String {
    let prefix = if offer.flavors.len() > 1 { "up to " } else { "" };
    let symbol = symbol(offer.currency);
    if offer.currency == "EUR" {
        format!("{prefix}{symbol}{:.4}/hr", offer.hourly)
    } else {
        format!("{prefix}{symbol}{:.2}/hr", offer.hourly)
    }
}

fn symbol(currency: &str) -> &str {
    match currency {
        "EUR" => "€",
        "USD" => "$",
        currency => currency,
    }
}

/// The three starting points. Returns the one clicked.
pub(super) fn picks(ui: &mut Ui, catalog: &Catalog, form: &Production) -> Option<usize> {
    let picks: Vec<(&str, usize)> = PICKS
        .into_iter()
        .zip([catalog.picks.cheapest, catalog.picks.balanced, catalog.picks.powerful])
        .filter_map(|(label, index)| Some((label, index?)))
        .collect();
    // Cards keep their width when fewer than three are left to show.
    let columns = if ui.available_width() >= 540.0 { PICKS.len() } else { 1 };
    let mut chosen = None;
    for row in picks.chunks(columns) {
        ui.columns(columns, |uis| {
            for (ui, &(label, index)) in uis.iter_mut().zip(row) {
                widgets::caption(ui, label);
                if card(ui, catalog, form, index) {
                    chosen = Some(index);
                }
            }
        });
    }
    chosen
}

/// A whole card is the click target; the chosen one has an accent outline.
fn card(ui: &mut Ui, catalog: &Catalog, form: &Production, index: usize) -> bool {
    let offer = &catalog.offers[index];
    let selected = catalog.selected == Some(index);
    let id = ui
        .id()
        .with(("worker-card", offer.provider, &offer.id, &offer.location));
    let hovered = ui.is_enabled() && ui.ctx().read_response(id).is_some_and(|response| response.hovered());
    // Cards take keyboard focus like buttons, and Enter or Space chooses one.
    let focused = ui.memory(|memory| memory.has_focus(id));
    let (fill, stroke) = if selected {
        (
            theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.14),
            Stroke::new(2.0, theme::ACCENT()),
        )
    } else if hovered {
        (theme::PANEL_BG_ALT(), Stroke::new(1.0, theme::FG_DIM()))
    } else {
        (theme::PANEL_BG_ALT(), Stroke::new(1.0, theme::BORDER_SUBTLE()))
    };
    let stroke = if focused { Stroke::new(2.0, theme::FG()) } else { stroke };
    let (name, detail) = title(offer);
    let (stock, color) = stock(catalog, index, form);
    let frame = Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(10)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.label(RichText::new(name).size(15.0).strong().color(theme::FG()));
            ui.label(RichText::new(&detail).size(12.5).color(theme::FG_SOFT()));
            ui.add_space(4.0);
            ui.label(RichText::new(price(offer)).size(19.0).strong().color(theme::FG()));
            if let Some(total) = catalog.total(offer, form) {
                ui.label(
                    RichText::new(format!("≈ {}{total:.3} total", symbol(catalog.currency)))
                        .size(12.0)
                        .color(theme::FG_SOFT()),
                );
            }
            ui.horizontal(|ui| widgets::pill(ui, stock, color));
        });
    let response = ui
        .interact(frame.response.rect, id, Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            ui.is_enabled(),
            selected,
            format!("{}, {detail}, {}, {stock}", title(offer).0, price(offer)),
        )
    });
    ui.add_space(4.0);
    response.clicked()
}

/// Search and eligibility controls keep their place when rankings refresh.
pub(super) fn filters(ui: &mut Ui, catalog: &Catalog, form: &mut Production) {
    let state = &mut form.launch.selector;
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.search)
                .desired_width(220.0)
                .hint_text("Search, e.g. 4090 or 16 vCPU"),
        );
        widgets::checkbox(ui, &mut state.in_stock_only, "In stock only");
    });
    let excluded = catalog.offers.len() - catalog.matching;
    if excluded > 0 {
        widgets::checkbox(ui, &mut state.show_below_minimums, "Show workers below requirements");
    }
}

/// Excluded workers can be inspected, but never chosen.
pub(super) fn all(ui: &mut Ui, catalog: &Catalog, form: &mut Production) -> Option<usize> {
    let state = &form.launch.selector;
    let excluded = catalog.offers.len() - catalog.matching;
    let search = state.search.trim().to_lowercase();
    let in_stock_only = state.in_stock_only;
    let show_below_minimums = state.show_below_minimums;
    let form = &*form;
    let rows: Vec<usize> = (0..catalog.offers.len())
        .filter(|&index| {
            form.launch
                .selector
                .provider_filter
                .as_ref()
                .is_none_or(|provider| provider == catalog.offers[index].provider)
        })
        .filter(|&index| show_below_minimums || index < catalog.matching)
        .filter(|&index| {
            let (name, detail) = title(&catalog.offers[index]);
            search.is_empty() || format!("{name} {detail}").to_lowercase().contains(&search)
        })
        .filter(|&index| {
            !in_stock_only
                || catalog
                    .stock(index, form)
                    .is_some_and(|stock| stock.level != horizon_core::cloud_runtime::prices::Availability::None)
        })
        .collect();
    let hidden = if show_below_minimums { 0 } else { excluded };
    widgets::note(
        ui,
        &format!(
            "Showing {} of {} workers · {hidden} below requirements hidden",
            rows.len(),
            catalog.offers.len()
        ),
    );
    if rows.is_empty() {
        widgets::note(
            ui,
            "No worker matches these filters. Clear the search or change the filters.",
        );
        return None;
    }
    let mut chosen = None;
    super::super::super::super::runtime::solid_scroll_area(ui)
        .id_salt("cloud-worker-catalog")
        .max_height(180.0)
        .show(ui, |ui| {
            for (stripe, index) in rows.into_iter().enumerate() {
                let response = ui.add_enabled_ui(index < catalog.matching, |ui| {
                    row(ui, catalog, form, index, stripe % 2 == 1)
                });
                if response.inner {
                    chosen = Some(index);
                }
            }
        });
    chosen
}

fn row(ui: &mut Ui, catalog: &Catalog, form: &Production, index: usize, striped: bool) -> bool {
    let offer = &catalog.offers[index];
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 32.0), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let selected = catalog.selected == Some(index);
    let (name, detail) = title(offer);
    let reason = super::profile(form).and_then(|profile| {
        horizon_core::cloud_runtime::offers::Requirements::for_profile(profile).resource_reason(offer)
    });
    let (stock, stock_color) = if reason.is_some() {
        ("Below requirements", theme::FG_DIM())
    } else {
        stock(catalog, index, form)
    };
    let painter = ui.painter();
    let fill = if selected {
        theme::alpha(theme::ACCENT(), 44)
    } else if response.hovered() && ui.is_enabled() {
        theme::alpha(theme::FG(), 16)
    } else if striped {
        theme::alpha(theme::FG(), 6)
    } else {
        egui::Color32::TRANSPARENT
    };
    painter.rect_filled(rect, 6, fill);
    if response.has_focus() {
        painter.rect_stroke(rect, 6, Stroke::new(1.5, theme::FG()), egui::StrokeKind::Inside);
    }
    if selected {
        let bar = egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height()));
        painter.rect_filled(bar, 2, theme::ACCENT());
    }
    let font = FontId::proportional(13.0);
    let y = rect.center().y;
    let at = |fraction: f32| rect.left() + 12.0 + (width - 24.0) * fraction;
    painter
        .with_clip_rect(painter.clip_rect().intersect(egui::Rect::from_min_max(
            egui::pos2(at(0.0), rect.top()),
            egui::pos2(at(0.33), rect.bottom()),
        )))
        .text(
            egui::pos2(at(0.0), y),
            Align2::LEFT_CENTER,
            &name,
            font.clone(),
            theme::FG(),
        );
    painter
        .with_clip_rect(painter.clip_rect().intersect(egui::Rect::from_min_max(
            egui::pos2(at(0.34), rect.top()),
            egui::pos2(at(0.65), rect.bottom()),
        )))
        .text(
            egui::pos2(at(0.34), y),
            Align2::LEFT_CENTER,
            &detail,
            font.clone(),
            theme::FG_DIM(),
        );
    painter.text(
        egui::pos2(at(0.66), y),
        Align2::LEFT_CENTER,
        stock,
        font.clone(),
        stock_color,
    );
    painter.text(
        egui::pos2(at(1.0), y),
        Align2::RIGHT_CENTER,
        price(offer),
        font,
        theme::FG(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            ui.is_enabled(),
            selected,
            format!("{name}, {detail}, {}, {stock}", price(offer)),
        )
    });
    let response = if let Some(reason) = reason {
        response.on_disabled_hover_text(reason)
    } else {
        response
    };
    response.clicked()
}

fn stock(catalog: &Catalog, index: usize, form: &Production) -> (&'static str, egui::Color32) {
    let offer = &catalog.offers[index];
    if offer.location.is_some() {
        return if offer.availability == "listed" {
            ("Listed available", theme::PALETTE_GREEN())
        } else {
            ("Unlisted · advisory", theme::PALETTE_YELLOW())
        };
    }
    widgets::stock(catalog.stock(index, form))
}
