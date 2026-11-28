//! One item of the group `Needs you`: the box of a question, the name of its
//! workspace, the text of the callout and the buttons that answer it.
//!
//! The geometry of an item is a function of the width of the sidebar and the
//! height of its two lines, so a test reads the rectangles of an item without a
//! window. A press on a button is a press on the button of the card and goes to
//! the one path of [`crate::app::block_clicks`]; a press beside the buttons opens
//! the card.

use std::sync::Arc;

use egui::{Color32, CornerRadius, Galley, Rect, Sense, Stroke, StrokeKind, Vec2, WidgetInfo, pos2, vec2};

use crate::theme;

use super::super::text;
use super::{MAX_ANSWERS, OPEN, Question};

/// The inset of the box of an item at each side of the sidebar.
const ITEM_INSET: f32 = 8.0;
/// The radius of the box of an item.
const ITEM_RADIUS: u8 = 12;
/// The padding of the box of an item, on all four sides.
const ITEM_PADDING: f32 = 10.0;
/// The air between two parts of an item.
const LINE_GAP: f32 = 7.0;
/// The size of the name of the workspace.
const NAME_SIZE: f32 = 12.0;
/// The size of the text of the callout and of the label of a button.
const TEXT_SIZE: f32 = 11.5;
/// The rows that the text of a callout takes before it ends in an ellipsis.
const TEXT_ROWS: usize = 2;
/// The height of a button of an item.
const BUTTON_HEIGHT: f32 = 26.0;
/// The radius of a button.
const BUTTON_RADIUS: u8 = 8;
/// The padding of a button at each side of its label.
const BUTTON_PADDING: f32 = 10.0;
/// The air between two buttons of one row.
const BUTTON_GAP: f32 = 6.0;
/// The alpha of the fill of an item and of its outline.
const FILL_ALPHA: u8 = 23;
const OUTLINE_ALPHA: u8 = 97;

/// The rectangles of one item of the group.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct ItemRects {
    /// The box of the item: the sidebar less [`ITEM_INSET`] at each side.
    pub(in crate::app) box_rect: Rect,
    /// The first line, which holds the name of the workspace.
    pub(in crate::app) name_rect: Rect,
    /// The second part, which holds the text of the callout in at most
    /// [`TEXT_ROWS`] rows.
    pub(in crate::app) text_rect: Rect,
    /// The buttons of the item, in its order, each with its rectangle.
    pub(in crate::app) buttons: Buttons,
}

/// The buttons of one item in a fixed room: at most [`MAX_ANSWERS`] answers and
/// the control [`OPEN`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct Buttons {
    rects: [Rect; MAX_ANSWERS + 1],
    count: usize,
}

impl Default for Buttons {
    fn default() -> Self {
        Self {
            rects: [Rect::ZERO; MAX_ANSWERS + 1],
            count: 0,
        }
    }
}

impl Buttons {
    /// The rectangle of the button at `index`, or `None` past the last button.
    #[must_use]
    pub(in crate::app) fn get(&self, index: usize) -> Option<Rect> {
        (index < self.count).then(|| self.rects[index])
    }

    /// How many buttons the item draws.
    #[must_use]
    pub(in crate::app) fn count(&self) -> usize {
        self.count
    }
}

/// What a press on one item asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Press {
    /// The id of the answer of the callout that the person pressed.
    pub(super) answer: Option<String>,
    /// The person pressed the body of the item or the control [`OPEN`].
    pub(super) open: bool,
}

impl ItemRects {
    /// The rectangles of one item, from its top edge. The left edge of the sidebar
    /// is zero, and `control_widths` holds the width of each button that the item
    /// draws, in its order.
    ///
    /// A button that does not fit beside the buttons of its row goes under them,
    /// so a narrow sidebar stacks the buttons instead of cutting them.
    #[must_use]
    pub(in crate::app) fn of(
        top: f32,
        sidebar: f32,
        name_height: f32,
        text_height: f32,
        control_widths: &[f32],
    ) -> Self {
        let box_width = (sidebar - 2.0 * ITEM_INSET).max(1.0);
        let content_left = ITEM_INSET + ITEM_PADDING;
        let content_right = (ITEM_INSET + box_width - ITEM_PADDING).max(content_left + 1.0);
        let content_width = content_right - content_left;
        let name_rect = Rect::from_min_size(pos2(content_left, top + ITEM_PADDING), vec2(content_width, name_height));
        let text_rect = Rect::from_min_size(
            pos2(content_left, name_rect.max.y + LINE_GAP),
            vec2(content_width, text_height),
        );

        let mut buttons = Buttons::default();
        let mut x = content_left;
        let mut y = text_rect.max.y + LINE_GAP;
        for width in control_widths.iter().take(MAX_ANSWERS + 1) {
            let width = width.clamp(1.0, content_width);
            if x > content_left && x + width > content_right {
                x = content_left;
                y += BUTTON_HEIGHT + BUTTON_GAP;
            }
            buttons.rects[buttons.count] = Rect::from_min_size(pos2(x, y), vec2(width, BUTTON_HEIGHT));
            buttons.count += 1;
            x += width + BUTTON_GAP;
        }

        let height = y + BUTTON_HEIGHT + ITEM_PADDING - top;
        Self {
            box_rect: Rect::from_min_size(pos2(ITEM_INSET, top), vec2(box_width, height)),
            name_rect,
            text_rect,
            buttons,
        }
    }

    /// The height that the item takes in the sidebar.
    #[must_use]
    pub(in crate::app) fn height(&self) -> f32 {
        self.box_rect.height()
    }
}

/// Draws one item of the group and takes the press of the person on it.
pub(super) fn render(ui: &mut egui::Ui, question: &Question) -> Press {
    let sidebar = ui.available_width();
    let content_width = (sidebar - 2.0 * (ITEM_INSET + ITEM_PADDING)).max(1.0);
    let name = text::line(
        ui,
        &question.name,
        NAME_SIZE,
        text::SEMIBOLD,
        theme::FG(),
        content_width,
    );
    let body = text::paragraph_rows(
        ui,
        &question.text,
        TEXT_SIZE,
        text::REGULAR,
        theme::FG_SOFT(),
        content_width,
        TEXT_ROWS,
    );
    let label_width = (content_width - 2.0 * BUTTON_PADDING).max(1.0);
    let labels = labels_of(ui, question, label_width);
    let widths: Vec<f32> = labels
        .iter()
        .map(|label| label.size().x + 2.0 * BUTTON_PADDING)
        .collect();

    let top = ui.cursor().top();
    let rects = ItemRects::of(top, sidebar, name.size().y, body.size().y, &widths);
    ui.allocate_space(Vec2::new(sidebar, rects.height()));
    paint_item(ui, &rects, &name, &body, &labels);
    read_item(ui, question, &rects)
}

/// The label of each button of an item, in its order: the answers of the callout
/// and then the control [`OPEN`]. The first answer is semibold, in the form of the
/// primary button of a card.
fn labels_of(ui: &egui::Ui, question: &Question, max_width: f32) -> Vec<Arc<Galley>> {
    let mut labels = Vec::with_capacity(question.answers.len() + 1);
    for (index, answer) in question.answers.iter().enumerate() {
        let weight = if index == 0 { text::SEMIBOLD } else { text::REGULAR };
        labels.push(text::line(
            ui,
            &answer.label,
            TEXT_SIZE,
            weight,
            label_color(index),
            max_width,
        ));
    }
    if question.more {
        labels.push(text::line(ui, OPEN, TEXT_SIZE, text::REGULAR, theme::FG(), max_width));
    }
    labels
}

/// Paints the box of an item, its two lines and its buttons.
fn paint_item(ui: &egui::Ui, rects: &ItemRects, name: &Arc<Galley>, body: &Arc<Galley>, labels: &[Arc<Galley>]) {
    let needs_you = theme::PALETTE_YELLOW();
    let painter = ui.painter();
    painter.rect_filled(rects.box_rect, ITEM_RADIUS, theme::alpha(needs_you, FILL_ALPHA));
    painter.rect_stroke(
        rects.box_rect,
        ITEM_RADIUS,
        Stroke::new(1.0_f32, theme::alpha(needs_you, OUTLINE_ALPHA)),
        StrokeKind::Inside,
    );
    painter.galley(rects.name_rect.min, name.clone(), theme::FG());
    painter.galley(rects.text_rect.min, body.clone(), theme::FG_SOFT());
    for (index, label) in labels.iter().enumerate() {
        let Some(button) = rects.buttons.get(index) else {
            break;
        };
        paint_button(ui, index, button, label);
    }
}

/// Paints one button of an item. The first answer takes the accent colour, which
/// is the primary action of the card; a later answer and the control [`OPEN`]
/// take the quiet form.
fn paint_button(ui: &egui::Ui, index: usize, rect: Rect, label: &Arc<Galley>) {
    let painter = ui.painter();
    let radius = CornerRadius::same(BUTTON_RADIUS);
    let (fill, outline) = if index == 0 {
        (theme::ACCENT(), theme::ACCENT())
    } else {
        (theme::PANEL_BG_ALT(), theme::BORDER_SUBTLE())
    };
    painter.rect_filled(rect, radius, fill);
    painter.rect_stroke(rect, radius, Stroke::new(1.0_f32, outline), StrokeKind::Inside);
    let size = label.size();
    painter.galley(
        pos2(rect.center().x - size.x / 2.0, rect.center().y - size.y / 2.0),
        label.clone(),
        label_color(index),
    );
}

/// The colour of the label of the button at `index`: the colour of the window
/// background on the accent button, and the main text colour on a quiet one.
fn label_color(index: usize) -> Color32 {
    if index == 0 { theme::BG() } else { theme::FG() }
}

/// Takes the press of the person on one item, and shows the whole text of a line
/// that the item cut.
fn read_item(ui: &mut egui::Ui, question: &Question, rects: &ItemRects) -> Press {
    let base = ("sidebar_inbox", question.panel.0);
    let body = ui.interact(rects.box_rect, ui.make_persistent_id(base), Sense::click());
    body.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, &question.spoken));

    let mut press = Press::default();
    for index in 0..rects.buttons.count() {
        let Some(rect) = rects.buttons.get(index) else {
            break;
        };
        let response = ui.interact(rect, ui.make_persistent_id((base, "answer", index)), Sense::click());
        let spoken = match question.answers.get(index) {
            Some(answer) => &answer.label,
            None => OPEN,
        };
        response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, spoken));
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.clicked() {
            match question.answers.get(index) {
                Some(answer) => press.answer = Some(answer.id.clone()),
                None if question.more => press.open = true,
                None => {}
            }
        }
    }
    if press.answer.is_none() && body.clicked() {
        press.open = true;
    }
    if press.open || press.answer.is_some() || body.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    hover_text(
        ui,
        ui.make_persistent_id((base, "name")),
        rects.name_rect,
        &question.name,
    );
    hover_text(
        ui,
        ui.make_persistent_id((base, "text")),
        rects.text_rect,
        &question.text,
    );
    press
}

/// Shows `text` while the pointer is on `rect`, for a line that the item cut.
fn hover_text(ui: &egui::Ui, id: egui::Id, rect: Rect, text: &str) {
    let response = ui.interact(rect, id, Sense::hover());
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Label, true, text));
    response.on_hover_text(text);
}
