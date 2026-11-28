//! The rows of the mirrored sessions under the name of a publishing computer in the
//! group `COMPUTERS` of the sidebar.
//!
//! A row is a row of a terminal of another computer: the tree guide, the mark, the
//! title and one word at the right. The mark is the two letters of the program
//! (`CC`, `CX`, `PI`, `AG`), and the word at the right is the time of the last change
//! of the session, or a dot in `ACCENT` while a process runs it. A computer shows at
//! most [`MAX_ROWS`] rows and then one row `and <N> more`, which shows the rest.
//!
//! Every text comes from the store of another computer: a title is one line that the
//! frame cut at its bound. No part of a row takes a press other than the row itself.

use egui::{CursorIcon, Rect, Sense, Vec2};
use horizon_core::session_pickup::{MAX_TITLE_CHARS, relative_time};
use horizon_sync::{MirrorComputer, MirrorSession};

use crate::app::session_mirror::MirrorKey;
use crate::theme;

use super::panel_rows::{self, PANEL_ROW_HEIGHT, STATE_SIZE, title_and_state};
use super::{rows, text};

/// The most rows of one computer before the row `and <N> more`.
pub(super) const MAX_ROWS: usize = 8;
/// The size of the two letters of the mark, which is the size of the kind icon of a
/// panel row.
const MARK_SIZE: f32 = 9.5;
/// The least room that the mark takes, so a mark and a kind icon start their title at
/// the same place.
const MARK_ROOM: f32 = 18.0;
/// The air between the mark and the title.
const MARK_GAP: f32 = 6.0;
/// The radius of the dot of a session that a process runs.
const LIVE_DOT: f32 = 3.0;

/// What a press on the rows of one computer asks the sidebar to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Press {
    /// Show this session in the dock.
    Session(MirrorKey),
    /// Show every session of this computer.
    More,
}

/// Draws the rows of the sessions of `computer`. `now_millis` is the clock of this
/// window, which the words of the times count from.
pub(super) fn render(ui: &mut egui::Ui, computer: &MirrorComputer, expanded: bool, now_millis: i64) -> Option<Press> {
    let mut pressed = None;
    for program in &computer.unreadable {
        note(ui, &format!("{} is not mirrored", program.name()));
    }
    let shown = computer.sessions.len().min(MAX_ROWS);
    let count = if expanded { computer.sessions.len() } else { shown };
    let last = computer.sessions.len().saturating_sub(1);
    for (index, session) in computer.sessions.iter().take(count).enumerate() {
        if row(ui, &computer.device, session, index == last, now_millis) {
            pressed = Some(Press::Session(MirrorKey {
                device: computer.device.clone(),
                program: session.program,
                session: session.session.clone(),
            }));
        }
    }
    if !expanded && computer.sessions.len() > shown {
        let rest = computer.sessions.len() - shown;
        if more(ui, rest) {
            pressed = Some(Press::More);
        }
    }
    pressed
}

/// One row of one session. Answers whether the person pressed it.
fn row(ui: &mut egui::Ui, device: &str, session: &MirrorSession, last: bool, now_millis: i64) -> bool {
    let rect = ui.allocate_space(Vec2::new(ui.available_width(), PANEL_ROW_HEIGHT)).1;
    let hovered = ui.rect_contains_pointer(rect);
    let card = rows::card(rect);
    let guide_x = panel_rows::guide_x(card);
    let painter = ui.painter();
    let center_y = rect.center().y;

    let guide = egui::Stroke::new(1.0_f32, theme::alpha(theme::BORDER_STRONG(), 200));
    let bottom = if last { center_y } else { rect.max.y };
    painter.line_segment([egui::pos2(guide_x, rect.min.y), egui::pos2(guide_x, bottom)], guide);
    painter.line_segment(
        [egui::pos2(guide_x, center_y), egui::pos2(guide_x + 9.0, center_y)],
        guide,
    );
    let left = guide_x + 13.0;
    if hovered {
        let highlight = Rect::from_min_max(
            egui::pos2(left - 6.0, rect.min.y + 1.0),
            egui::pos2(card.max.x, rect.max.y - 1.0),
        );
        painter.rect_filled(highlight, 8, theme::alpha(theme::PANEL_BG_ALT(), 170));
    }

    let mark = text::mono(ui, session.program.mark(), MARK_SIZE, theme::ACCENT());
    let title_left = left + mark.size().x.max(MARK_ROOM) + MARK_GAP;
    painter.galley(egui::pos2(left, center_y - mark.size().y / 2.0), mark, theme::ACCENT());

    // The word at the right is the time of the last change, or the dot of a session
    // that a process runs now.
    let word = (!session.live).then(|| relative_time(now_millis, session.updated_at));
    let state_width = word.as_ref().map_or(0.0, |word| {
        let galley = text::line(ui, word, STATE_SIZE, text::REGULAR, theme::FG_DIM(), 120.0);
        galley.size().x
    });
    let state_room = title_and_state(card.max.x - 26.0, title_left, state_width);
    let title_right = state_room.map_or(card.max.x - 26.0, |room| room.0);
    let title: String = session.title.chars().take(MAX_TITLE_CHARS).collect();
    let galley = text::line(
        ui,
        &title,
        12.5,
        text::MEDIUM,
        theme::FG_SOFT(),
        title_right - title_left,
    );
    painter.galley(
        egui::pos2(title_left, center_y - galley.size().y / 2.0),
        galley,
        theme::FG_SOFT(),
    );
    match (word, state_room) {
        (Some(word), Some((_, state_left, room))) => {
            let galley = text::line(ui, &word, STATE_SIZE, text::REGULAR, theme::FG_DIM(), room);
            painter.galley(
                egui::pos2(state_left, center_y - galley.size().y / 2.0),
                galley,
                theme::FG_DIM(),
            );
        }
        _ if session.live => {
            painter.circle_filled(egui::pos2(card.max.x - 13.0, center_y), LIVE_DOT, theme::ACCENT());
        }
        _ => {}
    }

    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let id = ui.make_persistent_id(("sidebar_mirror_session", device, session.session.as_str()));
    let response = ui.interact(rect, id, Sense::click());
    let spoken = format!(
        "{}, {} session on {device}, {}",
        title,
        session.program.name(),
        if session.live { "live" } else { "read only" }
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &spoken));
    response.on_hover_text(spoken).clicked()
}

/// The row `and <N> more`, which shows every session of a computer.
fn more(ui: &mut egui::Ui, rest: usize) -> bool {
    let row = ui.allocate_space(Vec2::new(ui.available_width(), PANEL_ROW_HEIGHT)).1;
    let hovered = ui.rect_contains_pointer(row);
    let card = rows::card(row);
    if hovered {
        let highlight = Rect::from_min_max(
            egui::pos2(card.min.x + 13.0, row.min.y + 1.0),
            egui::pos2(card.max.x, row.max.y - 1.0),
        );
        ui.painter()
            .rect_filled(highlight, 8, theme::alpha(theme::PANEL_BG_ALT(), 170));
    }
    let words = format!("and {rest} more");
    let galley = text::line(
        ui,
        &words,
        STATE_SIZE,
        text::REGULAR,
        theme::FG_DIM(),
        (card.width() - 40.0).max(1.0),
    );
    let left = card.min.x + 13.0 + 18.0 + MARK_GAP;
    ui.painter().galley(
        egui::pos2(left, row.center().y - galley.size().y / 2.0),
        galley,
        theme::FG_DIM(),
    );
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let response = ui.interact(row, ui.make_persistent_id("sidebar_mirror_more"), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &words));
    response.clicked()
}

/// One line of the section for a program that this publisher cannot read: the person
/// reads that the program is there and not mirrored.
fn note(ui: &mut egui::Ui, words: &str) {
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        let width = ui.available_width() - 18.0;
        ui.label(text::paragraph_rows(
            ui,
            words,
            11.0,
            text::REGULAR,
            text::secondary(),
            width,
            1,
        ));
    });
}

#[cfg(test)]
mod tests {
    use egui::FullOutput;
    use horizon_sync::MirrorProgram;
    use horizon_sync::{MirrorComputer, MirrorSession};

    use super::{MAX_ROWS, Press, render};
    use crate::test_egui::DiscardTextures;

    fn session(index: u32, live: bool) -> MirrorSession {
        MirrorSession {
            program: MirrorProgram::Claude,
            session: format!("00000000-0000-0000-0000-{index:012}"),
            title: format!("made-up session {index}"),
            folder: "repo".to_owned(),
            parent: "~/github".to_owned(),
            updated_at: 1_700_000_000_000,
            live,
        }
    }

    fn computer(sessions: Vec<MirrorSession>) -> MirrorComputer {
        MirrorComputer {
            device: "laptop-a".to_owned(),
            label: "Laptop A".to_owned(),
            sessions,
            unreadable: vec![MirrorProgram::Antigravity],
            own: false,
        }
    }

    /// The texts that one pass of the rows painted, and the press that it took.
    fn draw(computer: &MirrorComputer, expanded: bool) -> (Vec<String>, Option<Press>) {
        let ctx = egui::Context::default();
        let mut pressed = None;
        let output: FullOutput = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                pressed = render(ui, computer, expanded, 1_700_000_100_000);
            })
            .discard_textures();
        let texts = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect();
        (texts, pressed)
    }

    #[test]
    fn a_computer_shows_eight_rows_and_then_the_row_of_the_rest() {
        assert_eq!(MAX_ROWS, 8);
        let many = computer((0..12).map(|index| session(index, index == 1)).collect());

        let (texts, _) = draw(&many, false);
        assert!(texts.contains(&"made-up session 0".to_owned()), "{texts:?}");
        assert!(texts.contains(&"made-up session 7".to_owned()), "{texts:?}");
        assert!(
            !texts.contains(&"made-up session 8".to_owned()),
            "the ninth row waits for the press on the rest: {texts:?}"
        );
        assert!(texts.contains(&"and 4 more".to_owned()), "{texts:?}");
        assert!(texts.contains(&"CC".to_owned()), "the mark of the program: {texts:?}");
        assert!(
            texts.contains(&"Antigravity is not mirrored".to_owned()),
            "a program that this publisher cannot read is named: {texts:?}"
        );

        let (texts, _) = draw(&many, true);
        assert!(texts.contains(&"made-up session 11".to_owned()), "{texts:?}");
        assert!(!texts.iter().any(|text| text.starts_with("and ")), "{texts:?}");
    }

    #[test]
    fn a_row_that_no_press_touches_takes_no_press() {
        let (_, pressed) = draw(&computer(vec![session(1, false)]), false);
        assert!(pressed.is_none(), "a still frame takes no press");
    }

    #[test]
    fn the_word_at_the_right_is_the_time_of_a_quiet_session() {
        let (texts, _) = draw(&computer(vec![session(1, false)]), false);
        assert!(
            texts.iter().any(|text| text.contains("ago") || text == "just now"),
            "the time of the last change stands at the right: {texts:?}"
        );
        let (live, _) = draw(&computer(vec![session(1, true)]), false);
        assert!(
            !live.iter().any(|text| text.contains("ago")),
            "a session that a process runs shows the dot and no time: {live:?}"
        );
    }
}
