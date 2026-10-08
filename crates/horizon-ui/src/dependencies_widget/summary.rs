//! Filter tiles and the pull request pipeline above the repository table.

use egui::{Align2, FontId, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};

use horizon_core::maintenance::portfolio::{Counts, Filter, PrStage};

use super::{portfolio::State, tone, widgets};
use crate::{app::util::usize_to_f32, theme};

const GAP: f32 = 10.0;
const PIPELINE_WIDTH: f32 = 430.0;
/// Below this width the pull request card gives its room to the tiles.
const PIPELINE_MIN: f32 = 1080.0;

pub(super) fn show(ui: &mut egui::Ui, state: &mut State, counts: &Counts, total: usize, stages: &[usize; 6]) {
    let height = ui.available_height();
    let width = ui.available_width();
    let pipeline_room = if width >= PIPELINE_MIN {
        PIPELINE_WIDTH + GAP
    } else {
        0.0
    };
    let tile_width = (width - pipeline_room - GAP * 4.0) / 5.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        for filter in Filter::ALL {
            let count = filter.count(counts, total);
            if tile(ui, vec2(tile_width, height), filter, count, state.filter == filter).clicked() {
                state.filter = if state.filter == filter && filter != Filter::All {
                    Filter::All
                } else {
                    filter
                };
            }
        }
        if pipeline_room > 0.0 {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
            pipeline(ui, rect, stages, counts.open_prs);
        }
    });
}

fn tile(ui: &mut egui::Ui, size: Vec2, filter: Filter, count: usize, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            format!("{} {count}", filter.label()),
        )
    });
    let hovered = response.hovered();
    let fill = if selected {
        theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.14)
    } else if hovered {
        theme::blend(theme::PANEL_BG_ALT(), theme::FG(), 0.06)
    } else {
        theme::PANEL_BG_ALT()
    };
    let stroke = if response.has_focus() {
        Stroke::new(2.0, theme::FG())
    } else if selected {
        Stroke::new(1.5, theme::ACCENT())
    } else if hovered {
        Stroke::new(1.0, theme::FG_DIM())
    } else {
        Stroke::new(1.0, theme::BORDER_SUBTLE())
    };
    let painter = ui.painter();
    painter.rect(rect, 10, fill, stroke, StrokeKind::Inside);
    let label_y = rect.top() + 21.0;
    let dot = pos2(rect.left() + 18.0, label_y);
    match filter.status() {
        Some(status) => {
            painter.circle_filled(dot, 4.0, tone::color(status.tone()));
        }
        None => {
            painter.circle_stroke(dot, 3.6, Stroke::new(1.5, theme::FG_SOFT()));
        }
    }
    let label = widgets::line(
        painter,
        filter.label(),
        &FontId::proportional(13.0),
        if selected { theme::FG() } else { theme::FG_SOFT() },
        rect.width() - 44.0,
    );
    widgets::paint_line(painter, pos2(rect.left() + 30.0, label_y), Align2::LEFT_CENTER, label);
    let number_color = match filter {
        Filter::Attention if count > 0 => theme::PALETTE_YELLOW(),
        _ if count == 0 => theme::FG_DIM(),
        _ => theme::FG(),
    };
    painter.text(
        pos2(rect.left() + 16.0, rect.bottom() - 10.0),
        Align2::LEFT_BOTTOM,
        count.to_string(),
        FontId::monospace(26.0),
        number_color,
    );
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if selected && filter != Filter::All {
            "Show all repositories"
        } else {
            "Show only these repositories"
        })
}

/// Every pull request the worker reported, as one segmented bar with a legend.
fn pipeline(ui: &egui::Ui, rect: Rect, stages: &[usize; 6], open: usize) {
    let painter = ui.painter();
    painter.rect(
        rect,
        10,
        theme::PANEL_BG_ALT(),
        Stroke::new(1.0, theme::BORDER_SUBTLE()),
        StrokeKind::Inside,
    );
    let inner = rect.shrink2(vec2(16.0, 0.0));
    let total: usize = stages.iter().sum();
    let top = rect.top() + 21.0;
    painter.text(
        pos2(inner.left(), top),
        Align2::LEFT_CENTER,
        "Pull requests",
        FontId::proportional(13.0),
        theme::FG_SOFT(),
    );
    painter.text(
        pos2(inner.right(), top),
        Align2::RIGHT_CENTER,
        format!("{open} open · {total} total"),
        FontId::proportional(12.5),
        theme::FG_SOFT(),
    );
    let bar = Rect::from_min_size(pos2(inner.left(), rect.top() + 36.0), vec2(inner.width(), 8.0));
    segmented_bar(painter, bar, stages, total);
    let mut x = inner.left();
    let legend_y = rect.bottom() - 14.0;
    for (stage, count) in PrStage::ORDER.iter().zip(stages) {
        if *count == 0 {
            continue;
        }
        let galley = painter.layout_no_wrap(
            format!("{count} {}", stage.label()),
            FontId::proportional(11.5),
            theme::FG_SOFT(),
        );
        let width = 11.0 + galley.size().x;
        if x + width > inner.right() {
            break;
        }
        painter.circle_filled(pos2(x + 3.0, legend_y), 3.0, tone::color(stage.tone()));
        painter.galley(
            pos2(x + 11.0, legend_y - galley.size().y / 2.0),
            galley,
            theme::FG_SOFT(),
        );
        x += width + 11.0;
    }
}

fn segmented_bar(painter: &egui::Painter, bar: Rect, stages: &[usize; 6], total: usize) {
    if total == 0 {
        painter.rect_filled(bar, 4, theme::alpha(theme::BORDER_SUBTLE(), 160));
        return;
    }
    let present = stages.iter().filter(|count| **count > 0).count();
    let gaps = 3.0 * usize_to_f32(present.saturating_sub(1));
    let room = bar.width() - gaps;
    let mut x = bar.left();
    let mut drawn = 0;
    for (stage, count) in PrStage::ORDER.iter().zip(stages) {
        if *count == 0 {
            continue;
        }
        drawn += 1;
        let width = if drawn == present {
            bar.right() - x
        } else {
            (room * usize_to_f32(*count) / usize_to_f32(total)).max(4.0)
        };
        let segment = Rect::from_min_size(pos2(x, bar.top()), vec2(width.max(0.0), bar.height()));
        painter.rect_filled(segment, 3, tone::color(stage.tone()));
        x += width + 3.0;
    }
}
