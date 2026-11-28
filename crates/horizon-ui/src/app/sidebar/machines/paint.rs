//! The painting of one machine row: its box and state glyph, its name, its second
//! line, its price, its fold control and the press that it takes.
//!
//! The geometry and the words of the row come from `sidebar::machines`; this file
//! only draws them.

use egui::{CursorIcon, Rect, Sense, Stroke, Vec2, pos2};

use crate::theme;

use super::super::HorizonApp;
use super::super::{HALF_FILL, SidebarActions, fold, text};
use super::agents::fold_spoken;
use super::{
    BOX_RADIUS, MachineRow, MachineState, PADDING, PENDING_MARK, ROW_HEIGHT, box_rect, machine_click, price_line,
    spoken,
};

/// The two rounded bars of the glyph of a machine, the gap between them and the
/// stroke that draws them.
const BAR_WIDTH: f32 = 9.0;
const BAR_HEIGHT: f32 = 3.2;
const BAR_GAP: f32 = 1.6;
const BAR_STROKE: f32 = 1.3;
/// The size of the name of a machine row and of the line under it.
const NAME_SIZE: f32 = 12.5;
const LINE_SIZE: f32 = 10.5;
/// The side of the fold control at the right end of a machine row.
pub(super) const FOLD_SIDE: f32 = 18.0;
/// The mark of the fold control: the agents are open, then folded.
const FOLD_MARKS: [&str; 2] = ["\u{25BE}", "\u{25B8}"];
/// The circular radius of a machine row behind the pointer.
const CARD_RADIUS: u8 = 10;

impl HorizonApp {
    /// Draws one machine row and turns a press on it into an action.
    pub(super) fn render_sidebar_machine(&mut self, ui: &mut egui::Ui, row: &MachineRow, actions: &mut SidebarActions) {
        let rect = ui.allocate_space(Vec2::new(ui.available_width(), ROW_HEIGHT)).1;
        let card = super::card(rect);
        let hovered = ui.rect_contains_pointer(rect);
        if hovered {
            ui.painter()
                .rect_filled(card, CARD_RADIUS, theme::alpha(theme::PANEL_BG_ALT(), HALF_FILL));
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        // The agents hang under an open row, and a folded row says in one line who
        // waits. A machine with no line under its row has nothing to fold, so it
        // has no mark. The mark stands at the right end of the row, because the box
        // of the machine takes the left one.
        let foldable = !row.watch.agents.is_empty() || row.watch.quiet.is_some();
        let fold = Rect::from_min_size(
            pos2(rect.max.x - fold::INSET - FOLD_SIDE, card.center().y - FOLD_SIDE / 2.0),
            Vec2::splat(FOLD_SIDE),
        );
        let folded = self.machine_watch.folded(&row.name);
        if foldable && render_fold_control(ui, fold, &row.name, folded, hovered) {
            self.machine_watch.toggle_fold(&row.name);
        }
        let elided = paint_machine_row(ui, card, row, folded, foldable, fold);

        // The press of the fold control is no press on the machine, so the row ends
        // before it.
        let click_rect = Rect::from_min_max(rect.min, pos2(fold.min.x, rect.max.y));
        let response = ui.interact(
            click_rect,
            ui.make_persistent_id(("sidebar_machine", row.name.as_str())),
            Sense::click(),
        );
        let spoken = spoken(row);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &spoken));
        let response = if elided {
            response.on_hover_text(format!("{}\n{spoken}", row.name))
        } else {
            response.on_hover_text(spoken)
        };
        if response.clicked() {
            machine_click(row, actions);
        }
        self.show_machine_context_menu(&response, row, actions);
        if !folded {
            row.watch.paint_rows(ui, row, actions);
        }
    }
}

/// Paints the box, the name, the line and the summary of a machine row in `card`,
/// and answers whether the room of the row cuts the name.
fn paint_machine_row(ui: &egui::Ui, card: Rect, row: &MachineRow, folded: bool, foldable: bool, fold: Rect) -> bool {
    let machine_box = box_rect(card);
    paint_machine_box(ui.painter(), machine_box, row.state);

    let text_left = machine_box.max.x + PADDING;
    let text_right = if foldable {
        fold.min.x - 6.0
    } else {
        card.max.x - PADDING
    };
    // The price stands at the right end of the row, on the line of the name, and it
    // gives way last: the name ends before it, so the price of a record stays whole.
    // The second line keeps the whole width, so the size and the place stay whole.
    let price = row.price.as_deref().map(|price| {
        text::mono_line(
            ui,
            &price_line(price),
            LINE_SIZE,
            theme::FG_SOFT(),
            (text_right - text_left).max(1.0),
        )
    });
    let price_width = price.as_ref().map_or(0.0, |price| price.size().x + 6.0);
    let summary = row.watch.summary.as_deref().filter(|_| folded).map(|words| {
        text::line(
            ui,
            words,
            LINE_SIZE,
            text::REGULAR,
            text::secondary(),
            (text_right - text_left).max(1.0),
        )
    });
    let summary_width = summary.as_ref().map_or(0.0, |summary| summary.size().x + 6.0);
    let mark = row.pending.then(|| {
        text::line(
            ui,
            PENDING_MARK,
            10.0,
            text::REGULAR,
            text::secondary(),
            (text_right - text_left).max(1.0),
        )
    });
    let mark_width = mark.as_ref().map_or(0.0, |mark| mark.size().x + 6.0);
    let name = text::line(
        ui,
        &row.name,
        NAME_SIZE,
        text::SEMIBOLD,
        theme::FG(),
        (text_right - text_left - mark_width - price_width).max(1.0),
    );
    let second = row.second.as_deref().map(|second| {
        text::line(
            ui,
            second,
            LINE_SIZE,
            text::REGULAR,
            text::secondary(),
            (text_right - text_left - summary_width).max(1.0),
        )
    });
    let name_height = name.size().y;
    // The summary of a folded machine stands on the second line, and it is that
    // line when the row says nothing else.
    let second_height = second.as_ref().or(summary.as_ref()).map_or(0.0, |line| line.size().y);
    let gap = if second_height > 0.0 { 2.0 } else { 0.0 };
    let top = card.center().y - (name_height + gap + second_height) / 2.0;
    let painter = ui.painter();
    let name_elided = name.elided;
    painter.galley(pos2(text_left, top), name, theme::FG());
    if let Some(mark) = mark {
        ui.painter().galley(
            pos2(
                text_right - price_width - mark.size().x,
                top + (name_height - mark.size().y) / 2.0,
            ),
            mark,
            text::secondary(),
        );
    }
    if let Some(price) = price {
        ui.painter().galley(
            pos2(text_right - price.size().x, top + (name_height - price.size().y) / 2.0),
            price,
            theme::FG_SOFT(),
        );
    }
    let line_top = top + name_height + gap;
    if let Some(second) = second {
        ui.painter()
            .galley(pos2(text_left, line_top), second, text::secondary());
    }
    if let Some(summary) = summary {
        ui.painter().galley(
            pos2(text_right - summary.size().x, line_top),
            summary,
            text::secondary(),
        );
    }
    name_elided
}

/// Paints the fold control of a machine row at its right end, which a screen
/// reader names, and answers whether the person pressed it.
fn render_fold_control(ui: &egui::Ui, rect: Rect, machine: &str, folded: bool, lit: bool) -> bool {
    let spoken = fold_spoken(folded, machine);
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("sidebar_machine_fold", machine)),
        Sense::click(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &spoken));
    let color = if lit { theme::FG() } else { text::secondary() };
    let mark = text::line(
        ui,
        FOLD_MARKS[usize::from(folded)],
        10.0,
        text::REGULAR,
        color,
        FOLD_SIDE,
    );
    ui.painter().galley(rect.center() - mark.size() / 2.0, mark, color);
    response.on_hover_text(spoken).clicked()
}

/// The colour of the state of the terminal of a machine, when the row shows one.
fn state_color(state: MachineState) -> Option<egui::Color32> {
    match state {
        MachineState::Closed | MachineState::Connecting => None,
        MachineState::Reconnecting => Some(theme::PALETTE_YELLOW()),
        MachineState::Connected => Some(theme::PALETTE_GREEN()),
        MachineState::Disconnected => Some(theme::PALETTE_RED()),
    }
}

/// Paints the box of a machine row: its outline and the two bars of its glyph, in
/// the colour of the state of its terminal where that state has one.
fn paint_machine_box(painter: &egui::Painter, rect: Rect, state: MachineState) {
    painter.rect_stroke(
        rect,
        BOX_RADIUS,
        Stroke::new(1.0_f32, theme::BORDER_SUBTLE()),
        egui::StrokeKind::Inside,
    );
    let color = state_color(state).unwrap_or_else(theme::FG_SOFT);
    let stroke = Stroke::new(BAR_STROKE, color);
    let center = rect.center();
    let total = 2.0 * BAR_HEIGHT + BAR_GAP;
    let top = center.y - total / 2.0;
    for index in 0..2 {
        let bar = Rect::from_min_size(
            pos2(
                center.x - BAR_WIDTH / 2.0,
                top + usize_to_f32(index) * (BAR_HEIGHT + BAR_GAP),
            ),
            Vec2::new(BAR_WIDTH, BAR_HEIGHT),
        );
        painter.rect_stroke(bar, 1, stroke, egui::StrokeKind::Inside);
    }
}

/// `value` as a float, for the geometry of a row.
fn usize_to_f32(value: usize) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}
