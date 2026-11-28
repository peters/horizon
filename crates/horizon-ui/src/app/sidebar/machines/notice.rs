//! The one short notice of the sidebar and the line that stays until a person
//! closes it: the two answers that the machine section shows below its rows.
//!
//! A notice of an action ends after [`NOTICE`]; a line names what an opened session
//! left out, keeps its own close control and no timer.

use egui::{CursorIcon, Sense, Stroke, Vec2};

use crate::app::HorizonApp;
use crate::theme;

use super::super::text;
use super::{NOTICE, SidebarNoticeTone};

/// The side of the close control of the line that stays until a person closes it.
const CLOSE_SIDE: f32 = 18.0;
/// The corner radius of that control.
const CLOSE_RADIUS: u8 = 5;

impl HorizonApp {
    /// Says why an action of the person opened nothing, in the machine section.
    ///
    /// It is the one short notice of this window: a machine action, a panel
    /// that cannot start and a browser engine that ends at once all say their
    /// reason here, in the danger tone, for [`NOTICE`].
    pub(in crate::app) fn set_sidebar_notice(&mut self, line: String) {
        self.set_sidebar_notice_in(SidebarNoticeTone::Error, line);
    }

    /// Says one line that answers a key press of the person and is not an error.
    pub(in crate::app) fn set_sidebar_plain_notice(&mut self, line: String) {
        self.set_sidebar_notice_in(SidebarNoticeTone::Plain, line);
    }

    fn set_sidebar_notice_in(&mut self, tone: SidebarNoticeTone, line: String) {
        self.sidebar_notice = Some(line);
        self.sidebar_notice_until = Some(self.notice_clock.now() + NOTICE);
        self.sidebar_notice_tone = tone;
    }

    /// Says one line that stays until the person closes it, in the machine section.
    ///
    /// It has its own close control and no timer, because it names something that
    /// is gone for good: the panels that an opened session left out. A short notice
    /// of an action beside it neither replaces it nor ends with it.
    pub(in crate::app) fn set_sidebar_sticky_notice(&mut self, line: String) {
        self.sidebar_sticky_notice = Some(line);
    }

    /// Forgets the line that stays until the person closes it, which is what its
    /// close control asks for.
    pub(in crate::app) fn dismiss_sidebar_sticky_notice(&mut self) {
        self.sidebar_sticky_notice = None;
    }
}

/// The line that stays until the person closes it, and its close control. Answers
/// whether the person closed it.
///
/// The control is a press target of its own, as a machine row is, so a keyboard
/// reaches it with Tab and ends it with Space or Enter, and the ring of the focus
/// says where the keyboard is.
pub(super) fn render_sticky_notice(ui: &mut egui::Ui, note: &str, id: egui::Id) -> bool {
    let mut closed = false;
    ui.horizontal_top(|ui| {
        ui.add_space(18.0);
        let width = (ui.available_width() - CLOSE_SIDE - 4.0).max(40.0);
        ui.label(text::paragraph(
            ui,
            note,
            11.0,
            text::REGULAR,
            theme::PALETTE_YELLOW(),
            width,
        ));
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(CLOSE_SIDE), Sense::hover());
        let response = ui.interact(rect, id, Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, CLOSE_SPOKEN));
        if response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
            ui.painter()
                .rect_filled(rect, CLOSE_RADIUS, theme::alpha(theme::PANEL_BG_ALT(), 170));
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect,
                CLOSE_RADIUS,
                Stroke::new(1.0_f32, theme::ACCENT()),
                egui::StrokeKind::Outside,
            );
        }
        let color = if response.hovered() || response.has_focus() {
            theme::FG()
        } else {
            text::secondary()
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            CLOSE_MARK,
            egui::FontId::proportional(11.0),
            color,
        );
        if response.clicked() {
            closed = true;
        }
    });
    closed
}
/// The mark of the close control, and what a screen reader calls it.
const CLOSE_MARK: &str = "\u{00d7}";
const CLOSE_SPOKEN: &str = "Close this line";
