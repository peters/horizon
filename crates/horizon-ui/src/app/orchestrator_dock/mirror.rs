//! One mirrored session in the dock: the header of the session, the rows of its
//! conversation, and the line in place of the field.
//!
//! The dock shows the session of another computer while the person reads it. There is
//! no field and no chip of a program: the session runs on its own computer, and this
//! window reads it. The header reads the title of the session and, under it, the
//! program and the computer with `read only`; its chip reads `live` while a process
//! runs the session and `idle` otherwise. In place of the composer the dock reads one
//! centred line.
//!
//! Nothing here sends a key or a text to the session: the mirror is read only.

use egui::{Rect, Ui, pos2};

use crate::app::session_mirror::{MirrorKey, MirrorView};
use crate::theme;
use crate::typeface::Weight;

use super::list::{self, Refs};
use super::text;
use super::{header, layout};

/// The size of the line in place of the field, which is the size of a title of a row.
const LINE_SIZE: f32 = 12.5;
/// The air above and below that line, which is the padding of the header of the dock.
const LINE_PADDING: f32 = 12.0;
/// The text in place of the field, which says where the session runs.
pub(super) fn line(computer: &str) -> String {
    format!("This session runs on {computer}. You read it here.")
}

/// The second line of the header.
pub(super) fn header_line(program: &str, computer: &str) -> String {
    format!("{program} on {computer} · read only")
}

impl header::Form<'_> {
    /// The header of a mirrored session of `view`.
    pub(super) fn of_mirror<'a>(view: &'a MirrorView, key: &'a MirrorKey) -> Self {
        Self::Mirror {
            title: view
                .session(key)
                .map_or_else(|| key.session.clone(), |session| session.title.clone()),
            line: header_line(key.program.name(), &view.computer(&key.device)),
        }
    }
}

/// Draws the header, the rows and the line in place of the field of the session of
/// `key` in `content`.
pub(super) fn render(ui: &mut Ui, content: Rect, view: &MirrorView, key: &MirrorKey) {
    let form = header::Form::of_mirror(view, key);
    let live = view.session(key).is_some_and(|session| session.live);
    let state = header::State::mirrored(live);
    let header_height = header::height(ui, content.width(), &form, state);
    let words = line(&view.computer(&key.device));
    let galley = text::line(
        ui,
        &words,
        LINE_SIZE,
        Weight::Regular,
        theme::FG_SOFT(),
        content.width() - 32.0,
    );
    let composer_height = galley.size().y + 2.0 * LINE_PADDING;
    let parts = layout::parts(content, header_height, composer_height);
    header::render(ui, parts.header, &form, state);
    let entries = view.entries(key);
    let scroll = key.scroll_key();
    let _ = list::render(ui, parts.body, &scroll, entries, false, &Refs::default(), &|_| false);
    ui.painter().galley(
        pos2(
            parts.composer.center().x - galley.size().x / 2.0,
            parts.composer.center().y - galley.size().y / 2.0,
        ),
        galley,
        theme::FG_SOFT(),
    );
}
