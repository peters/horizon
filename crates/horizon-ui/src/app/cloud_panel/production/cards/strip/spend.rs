//! The header's spend: what the worker cost so far and what it costs a day, with the
//! week, the month and the rest of the month beneath when the header has room.
use crate::theme;
use egui::{FontId, Rect, Sense, pos2};

pub(in crate::app::cloud_panel) struct Spend {
    /// "$1.02 run · $8.86 total · $16.56/day", or what is known instead.
    pub line: String,
    /// The second row while a worker bills, such as
    /// "$115.92/wk · $503.70/mo · +$283.25 by Oct 31 if left running".
    pub outlook: Option<String>,
    pub explanation: String,
}

/// Space between the first row's bottom and the title row's centre.
const ABOVE: f32 = 4.0;
/// Space between the title row's centre and the second row's top.
const BELOW: f32 = 2.0;

/// Paints `spend` with its right edge at `right`, centred on `y`. Both rows show when
/// `fits` allows the wider one, otherwise only the first; `None` when even that does
/// not fit.
pub(super) fn paint(ui: &egui::Ui, right: f32, y: f32, spend: &Spend, fits: impl Fn(f32) -> bool) -> Option<Rect> {
    let painter = ui.painter();
    let line = painter.layout_no_wrap(spend.line.clone(), FontId::proportional(14.0), theme::FG_SOFT());
    let outlook = spend
        .outlook
        .as_ref()
        .map(|text| painter.layout_no_wrap(text.clone(), FontId::proportional(12.5), theme::FG_DIM()))
        .filter(|outlook| fits(outlook.size().x.max(line.size().x)));
    let shown_outlook = outlook.is_some();
    let rect = match outlook {
        Some(outlook) => {
            let top = y - ABOVE - line.size().y;
            let second = pos2(right - outlook.size().x, y + BELOW);
            let rect = Rect::from_min_max(
                pos2(right - line.size().x.max(outlook.size().x), top),
                pos2(right, second.y + outlook.size().y),
            );
            painter.galley(pos2(right - line.size().x, top), line, theme::FG_SOFT());
            painter.galley(second, outlook, theme::FG_DIM());
            rect
        }
        None if fits(line.size().x) => {
            let rect = Rect::from_min_size(pos2(right - line.size().x, y - line.size().y / 2.0), line.size());
            painter.galley(rect.min, line, theme::FG_SOFT());
            rect
        }
        None => return None,
    };
    let spoken = std::iter::once(spend.line.as_str())
        .chain(spend.outlook.as_deref().filter(|_| shown_outlook))
        .chain([spend.explanation.as_str()])
        .collect::<Vec<_>>()
        .join(". ");
    ui.interact(rect, ui.id().with("spend"), Sense::hover())
        .on_hover_text(&spend.explanation)
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, spoken.trim_end_matches(['.', ' '])));
    Some(rect)
}
