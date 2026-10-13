//! Draws the finder's results: a mark for what each one is, the name with the typed letters
//! lit, where it lives, and badges for GitHub and this computer.
use super::{Kind, Pick, Row, is_path, pick};
use crate::{
    app::{
        cloud_panel::production::creation::{selector::widgets, source::State},
        util::chrome_button,
    },
    theme,
};
use std::fmt::Write as _;

use egui::{
    Align2, Color32, FontId, Painter, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
    text::{LayoutJob, TextFormat},
};

const ROW: f32 = 44.0;
const LIST_HEIGHT: f32 = 300.0;

/// The account's state, the results and the keys that work them. Returns a result clicked.
pub(super) fn show(ui: &mut Ui, state: &mut State) -> Option<Pick> {
    let typed = state.input.trim().to_owned();
    state.finder.poll(ui.ctx(), &typed);
    let refreshing = account_status(ui, state);
    let repositories = state
        .account
        .as_ref()
        .and_then(|account| account.repositories.as_deref())
        .unwrap_or_default();
    let github_count = repositories.len();
    let rows = state.finder.rows(&typed, repositories).to_vec();
    let path = is_path(&typed);
    let busy = refreshing || state.finder.scanning();
    ui.add_space(2.0);
    caption_row(ui, state, &rows, &typed, github_count, busy);
    if rows.is_empty() {
        let text = if path {
            "No folders here. Keep typing, or go up a level."
        } else if typed.is_empty() && busy {
            "Looking for repositories…"
        } else if typed.is_empty() {
            "No repositories here yet. Type a path or paste a link."
        } else {
            "Nothing matches. Paste a link to clone any repository."
        };
        ui.label(RichText::new(text).size(13.0).color(theme::FG_DIM()));
        return None;
    }
    let clicked = list(ui, state, &rows);
    hint_row(ui, state, path);
    clicked
}

/// The connected account's sign-in, errors and out-of-date list. True while a quiet
/// refresh runs.
fn account_status(ui: &mut Ui, state: &mut State) -> bool {
    let Some(account) = state.account.as_mut() else {
        return false;
    };
    account.refresh_once(ui.ctx());
    if account.repositories.is_none() {
        // A quiet refresh shows nothing until it has a list: the button stays, and a
        // click on it takes over.
        if account.busy() {
            account.sign_in_box(ui);
        } else if ui.add(chrome_button("Show your GitHub repositories")).clicked() {
            account.request(super::super::github::Purpose::List, ui.ctx());
        }
    }
    if let Some(failed) = &account.failed {
        ui.label(RichText::new(failed).size(13.0).color(theme::PALETTE_RED()));
    }
    // A list that could not be refreshed says so, with a way to sign in again, shown here.
    if account.repositories.is_some() {
        if let Some(stale) = account.stale.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("This list may be out of date: {stale}"))
                        .size(12.5)
                        .color(theme::PALETTE_YELLOW()),
                );
                if ui.add(chrome_button("Refresh from GitHub")).clicked() {
                    account.request(super::super::github::Purpose::List, ui.ctx());
                }
            });
        }
        if account.busy() {
            account.sign_in_box(ui);
        }
    }
    account.refreshing()
}

/// What the results are, and how many there are of each kind.
fn caption_row(ui: &mut Ui, state: &State, rows: &[Row], typed: &str, github_count: usize, busy: bool) {
    ui.horizontal(|ui| {
        let count = |one: &str, many: &str| format!("{} {}", rows.len(), if rows.len() == 1 { one } else { many });
        let caption = if is_path(typed) {
            count("FOLDER", "FOLDERS")
        } else if typed.is_empty() && state.account.is_some() {
            "RECENT ON THIS COMPUTER, THEN GITHUB".to_owned()
        } else if typed.is_empty() {
            "RECENT ON THIS COMPUTER".to_owned()
        } else {
            count("MATCH", "MATCHES")
        };
        widgets::caption(ui, &caption);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut counts = format!("{} on this computer", state.finder.local_count());
            if state.account.is_some() {
                let _ = write!(counts, "  ·  {github_count} on GitHub");
            }
            ui.label(RichText::new(counts).size(11.0).color(theme::FG_DIM()));
            if busy {
                ui.add(egui::Spinner::new().size(11.0));
            }
        });
    });
}

/// The results in a scrolling frame. Returns a result clicked.
fn list(ui: &mut Ui, state: &mut State, rows: &[Row]) -> Option<Pick> {
    let mut clicked = None;
    let highlighted = state.finder.highlighted;
    let mut hovered = None;
    egui::Frame::new()
        .fill(theme::PANEL_BG_ALT())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(10)
        .inner_margin(4)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("cloud-source-finder")
                .max_height(LIST_HEIGHT)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (index, row) in rows.iter().enumerate() {
                        let response = row_widget(ui, row, index == highlighted);
                        if index == highlighted && state.finder.scroll_to_highlight {
                            response.scroll_to_me(None);
                        }
                        if response.hovered() {
                            hovered = Some(index);
                        }
                        // An earlier path's row is not this path's to choose.
                        if response.clicked() && state.finder.settled() {
                            clicked = Some(pick(row, false));
                        }
                    }
                });
        });
    state.finder.scroll_to_highlight = false;
    if let Some(index) = hovered
        && ui.input(|input| input.pointer.delta() != Vec2::ZERO)
    {
        // A row under the moving pointer is chosen as one picked with the arrows.
        state.finder.highlighted = index;
        state.finder.moved = true;
    }
    clicked
}

/// The keys that work the results, and the way to add repositories to the app.
fn hint_row(ui: &mut Ui, state: &State, path: bool) {
    ui.horizontal(|ui| {
        let hint = if path {
            "↑ ↓ move  ·  Enter uses a repository  ·  Tab opens a folder"
        } else {
            "↑ ↓ move  ·  Enter chooses  ·  a path opens folders  ·  a link clones"
        };
        ui.label(RichText::new(hint).size(11.5).color(theme::FG_DIM()));
        if let Some(account) = &state.account
            && account.repositories.is_some()
        {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new("Add repositories on GitHub")
                                .size(11.5)
                                .color(theme::ACCENT()),
                        )
                        .frame(false),
                    )
                    .on_hover_text("Choose which repositories your GitHub App can reach.")
                    .clicked()
                    && let Err(error) = horizon_core::open_url(&account.installation_url())
                {
                    tracing::warn!(%error, "could not open the GitHub App installation page");
                }
            });
        }
    });
}

/// A result row. It takes clicks but not focus: the arrow keys in the field move through
/// the results, so Tab goes on to the rest of the dialog.
fn row_widget(ui: &mut Ui, row: &Row, highlighted: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::CLICK);
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &row.title));
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter_at(rect);
    if highlighted {
        painter.rect_filled(rect.shrink(1.0), 8, theme::alpha(theme::ACCENT(), 34));
        painter.rect_stroke(
            rect.shrink(1.0),
            8,
            Stroke::new(1.0, theme::alpha(theme::ACCENT(), 110)),
            StrokeKind::Inside,
        );
    } else if response.hovered() {
        painter.rect_filled(rect.shrink(1.0), 8, theme::alpha(theme::FG(), 10));
    }
    let mark = Rect::from_center_size(Pos2::new(rect.left() + 22.0, rect.center().y), Vec2::splat(18.0));
    let mark_color = if highlighted { theme::ACCENT() } else { theme::FG_SOFT() };
    match &row.kind {
        Kind::Local { .. } | Kind::Folder { checkout: true, .. } => branch_mark(&painter, mark, mark_color),
        Kind::GitHub { .. } => cloud_mark(&painter, mark, mark_color),
        Kind::Folder { .. } => folder_mark(&painter, mark, mark_color),
    }
    let text_left = rect.left() + 44.0;
    let title = ui.fonts_mut(|fonts| fonts.layout_job(title_job(row)));
    painter.galley(Pos2::new(text_left, rect.top() + 6.0), title, theme::FG());
    painter.text(
        Pos2::new(text_left, rect.bottom() - 7.0),
        Align2::LEFT_BOTTOM,
        &row.detail,
        FontId::proportional(11.5),
        theme::FG_DIM(),
    );
    let mut right = rect.right() - 10.0;
    let badges: &[(&str, Color32)] = match &row.kind {
        Kind::Local { github: true, .. } => &[
            ("GitHub", theme::ACCENT()),
            ("on this computer", theme::PALETTE_GREEN()),
        ],
        Kind::Local { .. } | Kind::Folder { checkout: true, .. } => &[("on this computer", theme::PALETTE_GREEN())],
        Kind::GitHub { .. } => &[("GitHub", theme::ACCENT()), ("clone", theme::FG_SOFT())],
        Kind::Folder { .. } => &[("open  ⇥", theme::FG_DIM())],
    };
    for (text, color) in badges.iter().rev() {
        right = badge(ui, &painter, Pos2::new(right, rect.center().y), text, *color) - 6.0;
    }
    response
}

fn title_job(row: &Row) -> LayoutJob {
    let mut job = LayoutJob::default();
    let plain = TextFormat::simple(FontId::proportional(14.5), theme::FG());
    let lit = TextFormat {
        color: theme::ACCENT(),
        underline: Stroke::new(1.0, theme::alpha(theme::ACCENT(), 140)),
        ..TextFormat::simple(FontId::proportional(14.5), theme::ACCENT())
    };
    for (index, character) in row.title.chars().enumerate() {
        let format = if row.marks.contains(&index) {
            lit.clone()
        } else {
            plain.clone()
        };
        job.append(&character.to_string(), 0.0, format);
    }
    job
}

/// A small pill ending at `right`; returns where it starts.
fn badge(ui: &Ui, painter: &Painter, right_center: Pos2, text: &str, color: Color32) -> f32 {
    let galley = ui.fonts_mut(|fonts| fonts.layout_no_wrap(text.to_owned(), FontId::proportional(11.0), color));
    let size = galley.size() + Vec2::new(14.0, 4.0);
    let rect = Rect::from_min_size(Pos2::new(right_center.x - size.x, right_center.y - size.y / 2.0), size);
    painter.rect_filled(rect, 9, theme::alpha(color, 30));
    painter.galley(rect.min + Vec2::new(7.0, 2.0), galley, color);
    rect.left()
}

/// Two commits joined by a branch line: a repository.
fn branch_mark(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let top = Pos2::new(rect.left() + 5.0, rect.top() + 3.5);
    let bottom = Pos2::new(rect.left() + 5.0, rect.bottom() - 3.5);
    let side = Pos2::new(rect.right() - 4.0, rect.top() + 6.5);
    painter.line_segment([top, bottom], stroke);
    painter.line_segment(
        [side + Vec2::new(0.0, 2.5), Pos2::new(top.x, rect.center().y + 3.0)],
        stroke,
    );
    for point in [top, bottom, side] {
        painter.circle(point, 2.4, theme::PANEL_BG_ALT(), stroke);
    }
}

/// An arrow down into a tray: on GitHub, cloned once chosen.
fn cloud_mark(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let c = rect.center();
    let tip = c + Vec2::new(0.0, 3.0);
    painter.line_segment([c + Vec2::new(0.0, -7.0), tip], stroke);
    painter.line_segment([tip, tip + Vec2::new(-3.5, -3.5)], stroke);
    painter.line_segment([tip, tip + Vec2::new(3.5, -3.5)], stroke);
    let (left, right, bottom) = (c.x - 7.0, c.x + 7.0, c.y + 7.0);
    painter.line_segment([Pos2::new(left, c.y + 2.0), Pos2::new(left, bottom)], stroke);
    painter.line_segment([Pos2::new(left, bottom), Pos2::new(right, bottom)], stroke);
    painter.line_segment([Pos2::new(right, bottom), Pos2::new(right, c.y + 2.0)], stroke);
}

fn folder_mark(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    let body = Rect::from_min_max(rect.min + Vec2::new(1.0, 5.0), rect.max - Vec2::new(1.0, 3.0));
    painter.rect_stroke(body, 2, stroke, StrokeKind::Middle);
    painter.line_segment([body.left_top(), body.left_top() + Vec2::new(0.0, -2.5)], stroke);
    painter.line_segment(
        [
            body.left_top() + Vec2::new(0.0, -2.5),
            body.left_top() + Vec2::new(6.0, -2.5),
        ],
        stroke,
    );
}
