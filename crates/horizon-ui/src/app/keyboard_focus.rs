//! The keyboard of a panel that a person asks for.
//!
//! The board's focus says which panel the person works in, and the terminal body
//! of that panel takes the keys of the window. The second part needs the focus of
//! the widget, which a click on the body takes and nothing else does. An action
//! that creates or reveals one panel for the person therefore asks for it here:
//! the frame that draws the panel gives its body the focus, so the next key
//! reaches its program.
//!
//! An action of an agent never asks: a terminal that an agent opens leaves the
//! keyboard where the person had it.

use horizon_core::PanelId;

use super::HorizonApp;

impl HorizonApp {
    /// Gives `panel_id` the keyboard, because the person created or revealed it.
    pub(in crate::app) fn give_the_keyboard_to(&mut self, panel_id: PanelId) {
        self.board.focus(panel_id);
        self.keyboard_focus = Some(panel_id);
    }

    /// Whether the person asked for the keyboard of `panel_id` in this frame. The
    /// frame that draws the panel takes the request.
    pub(in crate::app) fn keyboard_focus_requested(&self, panel_id: PanelId) -> bool {
        self.keyboard_focus == Some(panel_id)
    }

    /// Drops the request of `panel_id`, which the frame that drew it answered.
    pub(in crate::app) fn keyboard_focus_answered(&mut self, panel_id: PanelId) {
        if self.keyboard_focus == Some(panel_id) {
            self.keyboard_focus = None;
        }
    }
}

#[cfg(test)]
mod tests;
