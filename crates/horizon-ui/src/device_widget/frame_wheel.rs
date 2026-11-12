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
