//! The wheel buttons of one frame.
//!
//! winit's X11 backend turns the press and the release of a wheel button into one
//! wheel event each, with the same delta: it maps the buttons 4 to 7 of an `XInput2`
//! button event and reads no state of the button (`xinput2_button_input` of
//! winit 0.30). A wheel without scroll valuators, and the XTEST click of the
//! fixture, is therefore one notch that arrives twice, and a viewer that counts both
//! scrolls two notches. A wheel with scroll valuators reports its notch as one axis
//! motion instead, and its emulated button events are dropped before they reach
//! this UI.
//!
//! The two events of one notch are equal and adjacent in the frame, so an event that
//! repeats the one before it is the release of that press. A fast wheel of a device
//! with valuators can put two real notches of one direction in one frame as well,
//! and it then counts as one: the loss is one step of a spin, and every notch of a
//! button wheel keeps its one step.

/// The wheel buttons of one frame.
#[derive(Debug, Default)]
pub(super) struct WheelButtons {
    previous: Option<(egui::MouseWheelUnit, egui::Vec2)>,
}

impl WheelButtons {
    /// Forget the event before this frame: the pair of one press is inside one frame.
    pub(super) fn begin_frame(&mut self) {
        self.previous = None;
    }

    /// True when this event is the release that repeats the press before it, so the
    /// two of them are one notch and not two.
    pub(super) fn repeats_previous(&mut self, unit: egui::MouseWheelUnit, delta: egui::Vec2) -> bool {
        let repeats = unit == egui::MouseWheelUnit::Line && self.previous == Some((unit, delta));
        self.previous = if repeats { None } else { Some((unit, delta)) };
        repeats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_press_and_the_release_of_one_wheel_button_are_one_notch() {
        let mut buttons = WheelButtons::default();
        buttons.begin_frame();
        assert!(
            !buttons.repeats_previous(egui::MouseWheelUnit::Line, egui::vec2(0.0, -1.0)),
            "the press of the button is the notch"
        );
        assert!(
            buttons.repeats_previous(egui::MouseWheelUnit::Line, egui::vec2(0.0, -1.0)),
            "the release repeats the press and is no second notch"
        );
        assert!(
            !buttons.repeats_previous(egui::MouseWheelUnit::Line, egui::vec2(0.0, -1.0)),
            "a second click of the wheel is a notch again"
        );
        assert!(buttons.repeats_previous(egui::MouseWheelUnit::Line, egui::vec2(0.0, -1.0)));
    }

    #[test]
    fn the_notch_of_the_next_frame_is_no_release_of_the_frame_before() {
        let mut buttons = WheelButtons::default();
        assert!(!buttons.repeats_previous(egui::MouseWheelUnit::Line, egui::vec2(0.0, -1.0)));
        buttons.begin_frame();
        assert!(
            !buttons.repeats_previous(egui::MouseWheelUnit::Line, egui::vec2(0.0, -1.0)),
            "one notch a frame keeps one step a frame"
        );
    }

    #[test]
    fn a_point_travel_is_no_wheel_button_and_keeps_every_event() {
        let mut buttons = WheelButtons::default();
        let travel = egui::vec2(0.0, -8.0);
        assert!(!buttons.repeats_previous(egui::MouseWheelUnit::Point, travel));
        assert!(
            !buttons.repeats_previous(egui::MouseWheelUnit::Point, travel),
            "a smooth scroll repeats a travel without a button to release"
        );
    }

    #[test]
    fn a_notch_of_another_direction_is_its_own_notch() {
        let mut buttons = WheelButtons::default();
        assert!(!buttons.repeats_previous(egui::MouseWheelUnit::Line, egui::vec2(0.0, -1.0)));
        assert!(!buttons.repeats_previous(egui::MouseWheelUnit::Line, egui::vec2(0.0, 1.0)));
    }
}
