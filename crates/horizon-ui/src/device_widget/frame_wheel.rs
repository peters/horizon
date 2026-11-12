//! The wheel of a person as the steps of the frame source contract.
//!
//! A frame source program counts the wheel in steps: a `dy` of 2 is two presses of
//! the wheel button, a positive `dy` is down and a positive `dx` is right. egui
//! reports the travel of the wheel in points, lines or pages, and a touchpad sends
//! many small travels, so the viewer adds them up and sends whole steps, one step
//! for one notch, and keeps the rest for the next frame.
use horizon_vnc::SCROLL_NOTCH_POINTS;

/// The most steps that one event may hold, so a program reads a step count and not
/// a burst. The steps above it are dropped rather than queued.
pub(super) const MAX_STEPS: f32 = 10.0;

/// The wheel travel of a viewer that is not yet a whole step.
#[derive(Debug, Default)]
pub(super) struct WheelSteps {
    points: [f32; 2],
}

/// The wheel buttons of one frame.
///
/// winit's X11 backend turns the press and the release of a wheel button into one
/// wheel event each, with the same delta: it maps the buttons 4 to 7 of an `XInput2`
/// button event and reads no state of the button (`xinput2_button_input` of
/// winit 0.30). A wheel without scroll valuators, and the XTEST click of the
/// fixture, is therefore one notch that arrives twice, and a viewer that counts both
/// scrolls two notches. A wheel with scroll valuators reports its notch as one axis
/// motion instead, and its emulated button events are dropped before they reach
/// this UI.
///
/// The two events of one notch are equal and adjacent in the frame, so an event that
/// repeats the one before it is the release of that press. A fast wheel of a device
/// with valuators can put two real notches of one direction in one frame as well,
/// and it then counts as one: the loss is one step of a spin, and every notch of a
/// button wheel keeps its one step.
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

impl WheelSteps {
    /// The whole steps of this event's travel. A positive answer is down and right,
    /// and the rest of the travel waits for the next frame.
    pub(super) fn steps(&mut self, unit: egui::MouseWheelUnit, delta: egui::Vec2) -> (i32, i32) {
        let [x, y] = super::capture::wheel_points(unit, delta);
        self.points[0] += x;
        self.points[1] += y;
        let mut steps = [0_i32; 2];
        let mut axis = 0;
        while axis < 2 {
            let whole = (self.points[axis] / SCROLL_NOTCH_POINTS).trunc();
            if whole != 0.0 {
                self.points[axis] -= whole * SCROLL_NOTCH_POINTS;
                // The travel of the wheel of this UI is positive up and left, and
                // the contract counts down and right.
                #[allow(clippy::cast_possible_truncation)]
                {
                    steps[axis] = -(whole.clamp(-MAX_STEPS, MAX_STEPS) as i32);
                }
            }
            axis += 1;
        }
        (steps[0], steps[1])
    }

    /// Forget the travel that is not a step, for a viewer that loses the pointer.
    pub(super) fn clear(&mut self) {
        self.points = [0.0; 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(unit: egui::MouseWheelUnit, delta: egui::Vec2) -> (i32, i32) {
        WheelSteps::default().steps(unit, delta)
    }

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

    #[test]
    fn one_notch_is_one_step_and_a_positive_step_is_down() {
        assert_eq!(steps(egui::MouseWheelUnit::Line, egui::vec2(0.0, -1.0)), (0, 1));
        assert_eq!(steps(egui::MouseWheelUnit::Line, egui::vec2(0.0, 1.0)), (0, -1));
        assert_eq!(
            steps(egui::MouseWheelUnit::Point, egui::vec2(0.0, -SCROLL_NOTCH_POINTS)),
            (0, 1)
        );
        assert_eq!(
            steps(egui::MouseWheelUnit::Line, egui::vec2(0.0, -1.0)),
            steps(egui::MouseWheelUnit::Point, egui::vec2(0.0, -SCROLL_NOTCH_POINTS)),
            "a line is the same notch as its points"
        );
    }

    #[test]
    fn the_travel_below_a_whole_step_waits_for_the_next_frame() {
        let mut wheel = WheelSteps::default();
        assert_eq!(wheel.steps(egui::MouseWheelUnit::Point, egui::vec2(0.0, -20.0)), (0, 0));
        assert_eq!(wheel.steps(egui::MouseWheelUnit::Point, egui::vec2(0.0, -20.0)), (0, 1));
        assert_eq!(wheel.steps(egui::MouseWheelUnit::Point, egui::vec2(0.0, -20.0)), (0, 0));
        assert_eq!(wheel.steps(egui::MouseWheelUnit::Point, egui::vec2(0.0, -20.0)), (0, 1));
    }

    #[test]
    fn a_scroll_sideways_counts_its_own_axis() {
        assert_eq!(steps(egui::MouseWheelUnit::Line, egui::vec2(-1.0, 0.0)), (1, 0));
        assert_eq!(steps(egui::MouseWheelUnit::Line, egui::vec2(1.0, 0.0)), (-1, 0));
        assert_eq!(
            steps(
                egui::MouseWheelUnit::Point,
                egui::vec2(SCROLL_NOTCH_POINTS, SCROLL_NOTCH_POINTS)
            ),
            (-1, -1),
            "one event can hold both axes"
        );
    }

    #[test]
    fn one_page_and_a_long_travel_stay_inside_the_bound_of_one_event() {
        assert_eq!(
            steps(egui::MouseWheelUnit::Page, egui::vec2(0.0, -1.0)),
            (0, 3),
            "a page is three notches"
        );
        assert_eq!(
            steps(egui::MouseWheelUnit::Line, egui::vec2(0.0, -30.0)),
            (0, 10),
            "one event holds at most ten steps"
        );
        let mut wheel = WheelSteps::default();
        assert_eq!(
            wheel.steps(egui::MouseWheelUnit::Point, egui::vec2(0.0, -20_000.0)),
            (0, 10)
        );
        assert_eq!(
            wheel.steps(egui::MouseWheelUnit::Point, egui::vec2(0.0, -1.0)),
            (0, 0),
            "the steps above the bound are dropped, not queued"
        );
    }
}
