//! The button `+ Rent a machine` at the end of the machine section of the
//! sidebar: one press asks the orchestrator of the conversation that the dock
//! shows for a machine of its own, and the person confirms the type, the place and
//! the price first.
//!
//! The button stands there only while the orchestrator dock is on, because the
//! message goes to the conversation of that dock: a window whose dock is off has
//! no conversation to send it to. The message is the one constant of this module,
//! so the words of the request stand in one place.
//!
//! The press takes the path of the composer of the dock: the message enters the
//! conversation as the message of the person, and the program of that conversation
//! starts when it has none. The button is drawn disabled, and sends nothing, while
//! the dock shows the front desk, while no workspace is active and while that
//! orchestrator works: a machine belongs to one work item.

use egui::{CursorIcon, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, vec2};

use crate::theme;

use super::super::HorizonApp;
use super::SidebarActions;
use super::text;

/// The label of the button.
pub(in crate::app) const LABEL: &str = "+ Rent a machine";
/// The message that a press sends to the orchestrator of the conversation that the
/// dock shows.
pub(in crate::app) const MESSAGE: &str =
    "Rent a machine for this workspace. Show me the card with the type, the place and the price first.";
/// The hover text of the button while the dock shows the front desk, while no
/// workspace is active, and while the orchestrator of that workspace works.
pub(in crate::app) const FRONT_DESK: &str = "Open a work item first";
pub(in crate::app) const NO_WORKSPACE: &str = "Open a workspace first";
pub(in crate::app) const BUSY: &str = "The orchestrator works";
/// The height of the button, the room at each side of the section that it keeps,
/// and its corner radius.
pub(in crate::app) const HEIGHT: f32 = 30.0;
pub(in crate::app) const INSET: f32 = 12.0;
const RADIUS: u8 = 10;
/// The air between the last row of the section and the button, and the air that
/// the label keeps at each side of it.
const GAP_ABOVE: f32 = 6.0;
const LABEL_INSET: f32 = 6.0;
const LABEL_SIZE: f32 = 12.5;

/// What a press on the button does now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum RentMachine {
    /// A press sends the message to the orchestrator of the active workspace.
    Ready,
    /// The dock shows the front desk, so no workspace is the subject of a press.
    FrontDesk,
    /// No workspace is active, so there is no orchestrator to ask.
    NoWorkspace,
    /// The orchestrator of the active workspace runs a turn.
    Busy,
}

impl RentMachine {
    /// Whether the button takes a press.
    #[must_use]
    pub(in crate::app) const fn enabled(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// What the hover of the button says while it is disabled.
    #[must_use]
    pub(in crate::app) const fn disabled_hover(self) -> Option<&'static str> {
        match self {
            Self::Ready => None,
            Self::FrontDesk => Some(FRONT_DESK),
            Self::NoWorkspace => Some(NO_WORKSPACE),
            Self::Busy => Some(BUSY),
        }
    }
}

impl HorizonApp {
    /// Draws the button at the end of the machine section, while the orchestrator
    /// dock is on.
    pub(super) fn render_rent_machine(&mut self, ui: &mut Ui, actions: &mut SidebarActions) {
        if !self.orchestrator_dock.visible() {
            return;
        }
        if render(ui, self.rent_machine_state()) {
            actions.rent_machine = true;
        }
    }

    /// What the button does in this frame: the conversation that the dock shows is
    /// the one that a press speaks to, and a machine belongs to one work item.
    fn rent_machine_state(&self) -> RentMachine {
        if self.orchestrator_dock.front_desk().shows(&self.board) {
            return RentMachine::FrontDesk;
        }
        let Some(local_id) = self
            .board
            .active_workspace
            .and_then(|id| self.board.workspace(id))
            .map(|workspace| workspace.local_id.as_str())
        else {
            return RentMachine::NoWorkspace;
        };
        if self.orchestrator_dock.busy(local_id) {
            return RentMachine::Busy;
        }
        RentMachine::Ready
    }

    /// The press: one message to the conversation that the dock shows, which
    /// starts its program when it has none.
    pub(super) fn rent_a_machine(&mut self, ctx: &egui::Context) {
        self.say_to_shown_orchestrator(ctx, MESSAGE);
    }
}

/// Draws the button in `state` and answers whether the person pressed it. It
/// takes the full width of the section less [`INSET`] at each side, and a press
/// is a click of the enabled button.
fn render(ui: &mut Ui, state: RentMachine) -> bool {
    ui.add_space(GAP_ABOVE);
    let row = ui.allocate_space(Vec2::new(ui.available_width(), HEIGHT)).1;
    let rect = row.shrink2(vec2(INSET, 0.0));
    let enabled = state.enabled();
    let response = ui.interact(rect, ui.make_persistent_id("sidebar_rent_machine"), Sense::click());
    let hovered = enabled && response.hovered();
    let color = if enabled { theme::FG() } else { theme::FG_DIM() };
    let outline = if hovered {
        theme::FG_DIM()
    } else {
        theme::BORDER_SUBTLE()
    };
    let label = text::line(
        ui,
        LABEL,
        LABEL_SIZE,
        text::REGULAR,
        color,
        (rect.width() - 2.0 * LABEL_INSET).max(1.0),
    );
    let painter = ui.painter();
    painter.rect_filled(rect, RADIUS, theme::PANEL_BG_ALT());
    painter.rect_stroke(rect, RADIUS, Stroke::new(1.0_f32, outline), StrokeKind::Inside);
    painter.galley(rect.center() - label.size() / 2.0, label, color);

    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, enabled, LABEL));
    let response = match state.disabled_hover() {
        Some(hover) => response.on_hover_text(hover),
        None => response,
    };
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    enabled && response.clicked()
}

#[cfg(test)]
mod tests {
    use super::{BUSY, FRONT_DESK, LABEL, MESSAGE, NO_WORKSPACE, RentMachine};

    #[test]
    fn only_a_window_with_an_idle_orchestrator_takes_a_press() {
        assert!(RentMachine::Ready.enabled());
        assert_eq!(RentMachine::Ready.disabled_hover(), None);

        assert!(!RentMachine::FrontDesk.enabled());
        assert_eq!(RentMachine::FrontDesk.disabled_hover(), Some(FRONT_DESK));
        assert_eq!(FRONT_DESK, "Open a work item first");
        assert!(!RentMachine::NoWorkspace.enabled());
        assert_eq!(RentMachine::NoWorkspace.disabled_hover(), Some(NO_WORKSPACE));
        assert!(!RentMachine::Busy.enabled());
        assert_eq!(RentMachine::Busy.disabled_hover(), Some(BUSY));
    }

    /// The words of the design, which the dock shows as the message of the person.
    #[test]
    fn the_button_and_its_message_carry_the_words_of_the_design() {
        assert_eq!(LABEL, "+ Rent a machine");
        assert_eq!(
            MESSAGE,
            "Rent a machine for this workspace. Show me the card with the type, the place and the price first."
        );
        assert_eq!(NO_WORKSPACE, "Open a workspace first");
        assert_eq!(BUSY, "The orchestrator works");
        assert!(
            MESSAGE.contains("type") && MESSAGE.contains("place") && MESSAGE.contains("price"),
            "the message asks for the three facts of the card: {MESSAGE}"
        );
    }
}
