//! The rows of the session pickup, as the first start page and the window draw
//! them.
//!
//! A row shows the title of a session, its program, its folder, the time of its
//! last activity and, when it has one, the reason that it does not open. The
//! press `Open` stands on the right of every row. The whole row is a control as
//! well: Tab reaches it and Enter opens it.

use egui::{
    Button, Color32, CornerRadius, CursorIcon, FontId, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2,
    text::LayoutJob,
};

use horizon_core::{PickupFolder, PickupRow};

use crate::theme;
use crate::typeface::{self, Weight};

/// The height of one row, in whole points.
pub(in crate::app) const ROW_HEIGHT: f32 = 44.0;
/// The space between two rows.
pub(in crate::app) const ROW_GAP: f32 = 4.0;
/// The space on both sides inside a row.
const ROW_PAD: f32 = 12.0;
/// The corner radius of a row.
const ROW_RADIUS: u8 = 10;
/// The width of the outline of a row.
const ROW_OUTLINE: f32 = 1.0;
/// The width of the ring that says where the keyboard is.
const FOCUS_RING: f32 = 2.0;
/// The font of the title of a row.
const TITLE_FONT: f32 = 13.5;
/// The line height of that title.
const TITLE_LINE_HEIGHT: f32 = 17.0;
/// The font of the line under the title.
const DETAIL_FONT: f32 = 11.5;
/// The line height of that line.
const DETAIL_LINE_HEIGHT: f32 = 15.0;
/// The width of the press of a row.
const OPEN_WIDTH: f32 = 74.0;
/// The height of that press.
const OPEN_HEIGHT: f32 = 26.0;
/// The font of that press.
const OPEN_FONT: f32 = 12.0;
/// The text of the press of a row that opens at once.
pub(in crate::app) const OPEN: &str = "Open";
/// The text of the press of a row that another program has open now.
pub(in crate::app) const OPEN_ANYWAY: &str = "Open anyway";
/// The text of the press of a folder that opens more than one row.
pub(in crate::app) const OPEN_ALL: &str = "Open all";
/// The words between the parts of the line under the title.
const SEPARATOR: &str = " · ";

/// What a row says on its two lines.
///
/// The first line is the title of the session. The second line names the program,
/// the folder when the row stands on its own, the time and the state. A row that
/// waits for the second confirmation names that in the same line.
#[must_use]
pub(in crate::app) fn row_lines(row: &PickupRow, folder: Option<&PickupFolder>) -> (String, String) {
    let (program, folder, tail) = detail_parts(row, folder);
    let mut parts = vec![program];
    if !folder.is_empty() {
        parts.push(folder);
    }
    parts.push(tail);
    (row.title.clone(), parts.join(SEPARATOR))
}

/// The three parts of the line under the title of a row: the program, the folder
/// of the session and the time with the state.
///
/// The parts are kept apart because a row that cannot hold the whole line cuts the
/// folder in its middle and keeps the program and the end of the line, which are
/// what a person reads first.
fn detail_parts(row: &PickupRow, folder: Option<&PickupFolder>) -> (String, String, String) {
    let mut tail = row.last_active.clone();
    if let Some(says) = row.state.says() {
        tail = format!("{tail}{SEPARATOR}{says}");
    }
    let folder = folder.map(folder_text).unwrap_or_default();
    (row_text(row), folder, tail)
}

/// The program of a row.
fn row_text(row: &PickupRow) -> String {
    row.kind.display_name().to_string()
}

/// The folder of a row, as its name and the path that holds it.
#[must_use]
pub(in crate::app) fn folder_text(folder: &PickupFolder) -> String {
    if folder.parent.is_empty() {
        folder.name.clone()
    } else {
        format!("{} in {}", folder.name, folder.parent)
    }
}

/// The words of the press of a row.
#[must_use]
pub(in crate::app) fn open_label(confirming: bool) -> &'static str {
    if confirming { OPEN_ANYWAY } else { OPEN }
}

/// What a press of the person on the pickup asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum PickupPress {
    /// Open one row of one folder.
    Open { folder: usize, row: usize },
    /// Open the rows of one folder.
    OpenAll { folder: usize },
    /// Open the window that holds every row.
    ShowAll,
}

/// Paints one row inside `rect` and answers the press of the person.
///
/// `folder` names the folder of the row when the row is not under the header of
/// its own group. `confirming` says that the person already pressed this row
/// once and that the row waits for the second confirmation.
pub(in crate::app) fn paint_row(
    ui: &mut Ui,
    rect: Rect,
    folder_index: usize,
    row_index: usize,
    row: &PickupRow,
    folder: Option<&PickupFolder>,
    confirming: bool,
) -> Option<PickupPress> {
    let id = ui.make_persistent_id(("pickup_row", folder_index, row.kind, &row.session_id));
    let whole = ui.interact(rect, id, Sense::click());
    let navigate = whole.has_focus() && press_enter(ui);
    if whole.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    whole.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, spoken(row)));
    paint_row_frame(ui, rect, whole.hovered(), whole.has_focus());

    let (title, detail) = row_lines(row, folder);
    // The whole line, and the two parts of it that a row keeps when it has to cut
    // the folder in its middle.
    let (_, folder_text, tail) = detail_parts(row, folder);
    let button_width = if confirming { OPEN_WIDTH + 34.0 } else { OPEN_WIDTH };
    let text_width = (rect.width() - 2.0 * ROW_PAD - button_width - 8.0).max(1.0);
    let painter = ui.painter();
    let title_top = rect.min.y + (ROW_HEIGHT - TITLE_LINE_HEIGHT - DETAIL_LINE_HEIGHT) * 0.5;
    let title = painter.layout_job(one_line_job(
        &title,
        TITLE_FONT,
        theme::FG(),
        Weight::Semibold,
        text_width,
    ));
    painter.galley(Pos2::new(rect.min.x + ROW_PAD, title_top), title, theme::FG());
    let detail = painter.layout_job(detail_job(painter, &detail, &folder_text, &tail, text_width));
    painter.galley(
        Pos2::new(rect.min.x + ROW_PAD, title_top + TITLE_LINE_HEIGHT),
        detail,
        theme::FG_SOFT(),
    );

    let button_rect = Rect::from_min_size(
        Pos2::new(rect.max.x - ROW_PAD - button_width, rect.center().y - OPEN_HEIGHT * 0.5),
        Vec2::new(button_width, OPEN_HEIGHT - 2.0),
    );
    let enabled = row.state.opens();
    let open = ui
        .put(
            button_rect,
            Button::new(
                egui::RichText::new(open_label(confirming))
                    .size(OPEN_FONT)
                    .color(if enabled { theme::FG() } else { theme::FG_DIM() }),
            )
            .fill(theme::alpha(theme::ACCENT(), if enabled { 26 } else { 8 }))
            .stroke(Stroke::new(
                1.0_f32,
                theme::alpha(theme::ACCENT(), if enabled { 120 } else { 40 }),
            ))
            .corner_radius(8.0),
        )
        .on_hover_text(open_hint(row, confirming));
    if !enabled {
        // A row that cannot open says why in its own line, and its press is off.
        let open = open.clone();
        open.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                false,
                format!("{} is not possible: {}", OPEN, row.state.says().unwrap_or_default()),
            )
        });
    }
    if (open.clicked() || navigate) && enabled {
        return Some(PickupPress::Open {
            folder: folder_index,
            row: row_index,
        });
    }
    None
}

/// The tooltip of the press of a row.
fn open_hint(row: &PickupRow, confirming: bool) -> String {
    if !row.state.opens() {
        return format!(
            "This session does not open: {}.",
            row.state.says().unwrap_or("the state of the session is unknown")
        );
    }
    if confirming {
        return "This program may open this session in two places at once. Press again to open it here.".to_string();
    }
    format!("Open this session of {} in its folder.", row.kind.display_name())
}

/// What a screen reader hears for one row.
#[must_use]
pub(in crate::app) fn spoken(row: &PickupRow) -> String {
    let mut parts = vec![
        row.title.clone(),
        row.kind.display_name().to_string(),
        row.last_active.clone(),
    ];
    if let Some(says) = row.state.says() {
        parts.push(says.to_string());
    }
    parts.push(OPEN.to_string());
    parts.join(". ")
}

/// Paints the header of one folder group and answers the press of `Open all`.
pub(in crate::app) fn paint_folder_header(
    ui: &mut Ui,
    rect: Rect,
    folder: &PickupFolder,
    index: usize,
) -> Option<PickupPress> {
    let painter = ui.painter();
    let name = painter.layout_job(one_line_job(
        &folder.name,
        13.0,
        theme::FG(),
        Weight::Semibold,
        (rect.width() - ROW_PAD * 2.0).max(1.0),
    ));
    painter.galley(Pos2::new(rect.min.x + ROW_PAD, rect.min.y + 4.0), name, theme::FG());
    let parent = painter.layout_job(one_line_job(
        &folder.parent,
        DETAIL_FONT,
        theme::FG_SOFT(),
        Weight::Regular,
        (rect.width() - ROW_PAD * 2.0).max(1.0),
    ));
    painter.galley(
        Pos2::new(rect.min.x + ROW_PAD, rect.min.y + 4.0 + TITLE_LINE_HEIGHT),
        parent,
        theme::FG_SOFT(),
    );
    let count = folder.open_all_count();
    if count == 0 {
        return None;
    }
    let button_rect = Rect::from_min_size(
        Pos2::new(rect.max.x - ROW_PAD - OPEN_WIDTH - 20.0, rect.min.y + 8.0),
        Vec2::new(OPEN_WIDTH + 20.0, OPEN_HEIGHT),
    );
    let open_all = ui
        .put(
            button_rect,
            Button::new(
                egui::RichText::new(format!("{OPEN_ALL} ({count})"))
                    .size(OPEN_FONT)
                    .color(theme::FG()),
            )
            .fill(theme::alpha(theme::ACCENT(), 26))
            .stroke(Stroke::new(1.0_f32, theme::alpha(theme::ACCENT(), 120)))
            .corner_radius(8.0),
        )
        .on_hover_text(format!("Open {count} sessions of this folder at one press."));
    if open_all.clicked() {
        return Some(PickupPress::OpenAll { folder: index });
    }
    None
}

/// The height of the header of one folder group, in whole points.
pub(in crate::app) const fn folder_header_height() -> f32 {
    38.0
}

/// Paints the background of a row and its focus ring.
fn paint_row_frame(ui: &Ui, rect: Rect, hovered: bool, focused: bool) {
    let painter = ui.painter();
    let radius = CornerRadius::same(ROW_RADIUS);
    let fill = if hovered {
        theme::blend(theme::PANEL_BG_ALT(), theme::ACCENT(), 0.10)
    } else {
        theme::PANEL_BG_ALT()
    };
    painter.rect_filled(rect, radius, fill);
    let outline = if hovered || focused {
        theme::ACCENT()
    } else {
        theme::BORDER_SUBTLE()
    };
    painter.rect_stroke(rect, radius, Stroke::new(ROW_OUTLINE, outline), StrokeKind::Inside);
    if focused {
        painter.rect_stroke(
            rect.expand(1.0),
            CornerRadius::same(ROW_RADIUS + 1),
            Stroke::new(FOCUS_RING, theme::FG()),
            StrokeKind::Outside,
        );
    }
}

/// A press of Enter on the widget that holds the keyboard.
fn press_enter(ui: &mut Ui) -> bool {
    ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
}

/// The line under the title of a row: the program at its start, the time and the
/// state at its end, and the folder between them cut in its middle when the row
/// cannot hold it whole.
fn detail_job(painter: &Painter, detail: &str, folder: &str, tail: &str, max_width: f32) -> LayoutJob {
    let width_of = |text: &str| {
        painter
            .layout_no_wrap(text.to_owned(), FontId::proportional(DETAIL_FONT), theme::FG_SOFT())
            .size()
            .x
    };
    // A row that holds no folder has nothing to cut in its middle: the program and
    // the time are what a person needs of it.
    let text = if folder.is_empty() || width_of(detail) <= max_width {
        detail.to_owned()
    } else {
        // The head of the line is what stands before the folder, separators and
        // all, and the end is the separator and the time with the state.
        let contains = format!("{folder}{SEPARATOR}{tail}");
        let head = detail.strip_suffix(contains.as_str()).unwrap_or(detail);
        let end = format!("{SEPARATOR}{tail}");
        let room = max_width - width_of(head) - width_of(&end);
        format!("{head}{}{end}", cut_in_the_middle(folder, room, width_of))
    };
    one_line_job(&text, DETAIL_FONT, theme::FG_SOFT(), Weight::Regular, max_width)
}

/// `text` with as much of its start and of its end as fit in `room` points, and
/// an ellipsis between them.
///
/// A folder is a path that a person recognizes by its start and by its end, so the
/// line keeps both and loses the middle. `width_of` measures a line in the font of
/// the row, and the loop removes the characters that the estimate says are too
/// many, so a row costs a few measurements and never a scan of the whole text.
fn cut_in_the_middle(text: &str, room: f32, width_of: impl Fn(&str) -> f32) -> String {
    let chars: Vec<char> = text.chars().collect();
    let join = |keep: usize| {
        let front = keep.div_ceil(2);
        let back = keep - front;
        let mut cut: String = chars[..front].iter().collect();
        cut.push('…');
        cut.extend(chars[chars.len() - back..].iter());
        cut
    };
    let mut keep = chars.len();
    while keep > 2 {
        let cut = join(keep);
        let width = width_of(&cut);
        if width <= room {
            return cut;
        }
        // The characters of this line are about as wide as the line is long, so the
        // part over the room names the count of characters that the middle loses.
        #[allow(clippy::cast_precision_loss)]
        let per_char = (width / (keep + 1) as f32).max(0.5);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let over = ((width - room) / per_char).ceil().max(1.0) as usize;
        keep = keep.saturating_sub(over).max(2);
    }
    join(keep)
}

/// One line of `text` at `size` and `weight`, cut to `max_width` with an ellipsis.
fn one_line_job(text: &str, size: f32, color: Color32, weight: Weight, max_width: f32) -> LayoutJob {
    let mut job = LayoutJob::single_section(
        text.to_owned(),
        typeface::format(&FontId::proportional(size), color, weight),
    );
    job.wrap = egui::text::TextWrapping {
        max_width,
        max_rows: 1,
        overflow_character: Some('…'),
        ..egui::text::TextWrapping::default()
    };
    job
}
