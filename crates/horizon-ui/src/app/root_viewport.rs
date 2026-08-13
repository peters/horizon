use std::time::{Duration, Instant};

use egui::{Align2, Context, FontId, ViewportId};

use super::HorizonApp;
use super::util::viewport_local_rect;

const VIEWPORT_SIZE_TOLERANCE: f32 = 1.0;
const VIEWPORT_SETTLE_TIME: Duration = Duration::from_millis(48);
const VIEWPORT_STABILIZATION_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug)]
pub(super) struct RootViewportStabilizer {
    expected_size: [f32; 2],
    expected_position: Option<[f32; 2]>,
    require_expected_geometry: bool,
    last_observed_size: Option<[f32; 2]>,
    stable_since: Option<Instant>,
    started_at: Option<Instant>,
    fallback_attempted: bool,
}

impl RootViewportStabilizer {
    pub(super) fn new(
        expected_size: [f32; 2],
        expected_position: Option<[f32; 2]>,
        require_expected_geometry: bool,
    ) -> Self {
        Self {
            expected_size,
            expected_position,
            require_expected_geometry,
            last_observed_size: None,
            stable_since: None,
            started_at: None,
            fallback_attempted: false,
        }
    }

    fn poll(
        &mut self,
        observed_size: [f32; 2],
        observed_position: Option<[f32; 2]>,
        now: Instant,
    ) -> ViewportStability {
        let started_at = *self.started_at.get_or_insert(now);
        if now.duration_since(started_at) >= VIEWPORT_STABILIZATION_TIMEOUT {
            return ViewportStability::TimedOut;
        }

        let expected_geometry_mismatch = self.require_expected_geometry
            && (!vector_near(observed_size, self.expected_size)
                || self.expected_position.zip(observed_position).is_some_and(
                    |(expected_position, observed_position)| !vector_near(observed_position, expected_position),
                ));
        if expected_geometry_mismatch {
            self.last_observed_size = None;
            self.stable_since = None;
            return ViewportStability::Waiting;
        }

        if self
            .last_observed_size
            .is_none_or(|previous_size| !vector_near(previous_size, observed_size))
        {
            self.last_observed_size = Some(observed_size);
            self.stable_since = Some(now);
            return ViewportStability::Waiting;
        }

        if self
            .stable_since
            .is_some_and(|stable_since| now.duration_since(stable_since) >= VIEWPORT_SETTLE_TIME)
        {
            ViewportStability::Stable
        } else {
            ViewportStability::Waiting
        }
    }

    fn begin_observed_geometry_fallback(&mut self) -> bool {
        if self.fallback_attempted {
            return false;
        }

        self.fallback_attempted = true;
        self.require_expected_geometry = false;
        self.last_observed_size = None;
        self.stable_since = None;
        self.started_at = None;
        true
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ViewportStability {
    Waiting,
    Stable,
    TimedOut,
}

fn vector_near(left: [f32; 2], right: [f32; 2]) -> bool {
    (left[0] - right[0]).abs() <= VIEWPORT_SIZE_TOLERANCE && (left[1] - right[1]).abs() <= VIEWPORT_SIZE_TOLERANCE
}

impl HorizonApp {
    pub(super) fn arm_root_viewport_stabilizer(&mut self, require_expected_geometry: bool, expected_size: [f32; 2]) {
        self.root_viewport_stabilizer = Some(RootViewportStabilizer::new(
            expected_size,
            self.window_config.x.zip(self.window_config.y).map(|(x, y)| [x, y]),
            require_expected_geometry,
        ));
    }

    pub(super) fn root_viewport_stabilization_blocks_interaction(&self) -> bool {
        self.root_viewport_stabilizer.is_some() && self.startup_workspace_organization_pending
    }

    pub(super) fn suppress_root_viewport_interaction(&mut self, ctx: &Context) {
        ctx.input_mut(|input| {
            input.raw.events.clear();
            input.raw.hovered_files.clear();
            input.raw.dropped_files.clear();
            input.raw.modifiers = egui::Modifiers::NONE;
            input.events.clear();
            input.keys_down.clear();
            input.modifiers = egui::Modifiers::NONE;
            input.pointer = egui::PointerState::default();
            input.raw_scroll_delta = egui::Vec2::ZERO;
            input.smooth_scroll_delta = egui::Vec2::ZERO;
        });
        self.frame_keyboard_events.remove(&ViewportId::ROOT);
        self.terminal_keyboard_events.clear();
    }

    #[profiling::function]
    pub(super) fn poll_root_viewport_stabilizer(&mut self, ctx: &Context) -> bool {
        let Some(stabilizer) = self.root_viewport_stabilizer.as_mut() else {
            return true;
        };
        let viewport = viewport_local_rect(ctx);
        let observed_size = [viewport.width(), viewport.height()];
        let observed_position = ctx
            .input(|input| input.viewport().outer_rect)
            .map(|rect| [rect.min.x, rect.min.y]);
        match stabilizer.poll(observed_size, observed_position, Instant::now()) {
            ViewportStability::Stable => {
                self.root_viewport_stabilizer = None;
                ctx.request_repaint();
                return true;
            }
            ViewportStability::TimedOut => {
                if stabilizer.begin_observed_geometry_fallback() {
                    tracing::warn!(
                        observed_width = observed_size[0],
                        observed_height = observed_size[1],
                        "root viewport missed requested geometry; retrying against observed geometry"
                    );
                    ctx.request_repaint_after(Duration::from_millis(16));
                    return false;
                }
                tracing::warn!(
                    observed_width = observed_size[0],
                    observed_height = observed_size[1],
                    "continuing after root viewport stabilization timed out"
                );
                self.root_viewport_stabilizer = None;
                ctx.request_repaint();
                return true;
            }
            ViewportStability::Waiting => {}
        }
        ctx.request_repaint_after(Duration::from_millis(16));
        false
    }

    pub(super) fn render_root_viewport_stabilizing_overlay(ctx: &Context) {
        let viewport = viewport_local_rect(ctx);
        egui::Area::new(egui::Id::new("root-viewport-stabilizing"))
            .order(egui::Order::Debug)
            .default_size(viewport.size())
            .fade_in(false)
            .fixed_pos(viewport.min)
            .show(ctx, |ui| {
                ui.set_min_size(viewport.size());
                let rect = ui.max_rect();
                ui.painter()
                    .rect_filled(rect, 0.0, crate::theme::alpha(crate::theme::BG(), 200));
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "Preparing session view…",
                    FontId::proportional(15.0),
                    crate::theme::FG(),
                );
                ui.allocate_rect(rect, egui::Sense::click_and_drag());
            });
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{RootViewportStabilizer, VIEWPORT_SETTLE_TIME, VIEWPORT_STABILIZATION_TIMEOUT, ViewportStability};

    #[test]
    fn first_observation_waits_for_stability() {
        let now = Instant::now();
        let mut stabilizer = RootViewportStabilizer::new([1400.0, 900.0], None, false);

        assert_eq!(stabilizer.poll([1400.0, 900.0], None, now), ViewportStability::Waiting);
        assert_eq!(
            stabilizer.poll([1400.0, 900.0], None, now + VIEWPORT_SETTLE_TIME),
            ViewportStability::Stable
        );
    }

    #[test]
    fn stabilization_budget_starts_at_first_poll() {
        let armed_at = Instant::now();
        let first_poll = armed_at + VIEWPORT_STABILIZATION_TIMEOUT * 2;
        let mut stabilizer = RootViewportStabilizer::new([1400.0, 900.0], None, false);

        assert_eq!(
            stabilizer.poll([1400.0, 900.0], None, first_poll),
            ViewportStability::Waiting
        );
        assert_eq!(
            stabilizer.poll([1400.0, 900.0], None, first_poll + VIEWPORT_SETTLE_TIME),
            ViewportStability::Stable
        );
    }

    #[test]
    fn window_manager_size_must_settle_in_wall_clock_time() {
        let now = Instant::now();
        let mut stabilizer = RootViewportStabilizer::new([1400.0, 900.0], None, false);

        assert_eq!(stabilizer.poll([1400.0, 900.0], None, now), ViewportStability::Waiting);
        assert_eq!(
            stabilizer.poll([1400.0, 900.0], None, now + VIEWPORT_SETTLE_TIME / 2),
            ViewportStability::Waiting
        );
        assert_eq!(
            stabilizer.poll([1400.0, 900.0], None, now + VIEWPORT_SETTLE_TIME),
            ViewportStability::Stable
        );
    }

    #[test]
    fn intermediate_size_restarts_the_settle_window() {
        let now = Instant::now();
        let mut stabilizer = RootViewportStabilizer::new([2560.0, 1440.0], None, false);

        assert_eq!(stabilizer.poll([1100.0, 720.0], None, now), ViewportStability::Waiting);
        assert_eq!(
            stabilizer.poll([1800.0, 1000.0], None, now + VIEWPORT_SETTLE_TIME),
            ViewportStability::Waiting
        );
        assert_eq!(
            stabilizer.poll(
                [1800.0, 1000.0],
                None,
                now + VIEWPORT_SETTLE_TIME + Duration::from_millis(1)
            ),
            ViewportStability::Waiting
        );
        assert_eq!(
            stabilizer.poll([2560.0, 1440.0], None, now + VIEWPORT_SETTLE_TIME * 2),
            ViewportStability::Waiting
        );
        assert_eq!(
            stabilizer.poll([2560.0, 1440.0], None, now + VIEWPORT_SETTLE_TIME * 3),
            ViewportStability::Stable
        );
    }

    #[test]
    fn timeout_is_independent_of_update_count() {
        let now = Instant::now();
        let mut stabilizer = RootViewportStabilizer::new([900.0, 700.0], None, false);
        for _ in 0..100 {
            assert_eq!(stabilizer.poll([900.0, 700.0], None, now), ViewportStability::Waiting);
        }

        assert_eq!(
            stabilizer.poll([900.0, 700.0], None, now + VIEWPORT_STABILIZATION_TIMEOUT),
            ViewportStability::TimedOut
        );
    }

    #[test]
    fn requested_restore_does_not_accept_the_old_stable_size() {
        let now = Instant::now();
        let mut stabilizer = RootViewportStabilizer::new([1400.0, 900.0], None, true);

        assert_eq!(stabilizer.poll([900.0, 700.0], None, now), ViewportStability::Waiting);
        assert_eq!(
            stabilizer.poll([900.0, 700.0], None, now + VIEWPORT_SETTLE_TIME),
            ViewportStability::Waiting
        );
        assert_eq!(
            stabilizer.poll(
                [1400.0, 900.0],
                None,
                now + VIEWPORT_SETTLE_TIME + Duration::from_millis(1),
            ),
            ViewportStability::Waiting
        );
        assert_eq!(
            stabilizer.poll(
                [1400.0, 900.0],
                None,
                now + VIEWPORT_SETTLE_TIME * 2 + Duration::from_millis(1)
            ),
            ViewportStability::Stable
        );
    }

    #[test]
    fn requested_restore_waits_for_the_target_position_too() {
        let now = Instant::now();
        let mut stabilizer = RootViewportStabilizer::new([1400.0, 900.0], Some([120.0, 80.0]), true);

        assert_eq!(
            stabilizer.poll([1400.0, 900.0], Some([10.0, 20.0]), now),
            ViewportStability::Waiting
        );
        assert_eq!(
            stabilizer.poll([1400.0, 900.0], Some([10.0, 20.0]), now + VIEWPORT_SETTLE_TIME),
            ViewportStability::Waiting
        );
        assert_eq!(
            stabilizer.poll([1400.0, 900.0], Some([120.0, 80.0]), now + VIEWPORT_SETTLE_TIME * 2),
            ViewportStability::Waiting
        );
        assert_eq!(
            stabilizer.poll([1400.0, 900.0], Some([120.0, 80.0]), now + VIEWPORT_SETTLE_TIME * 3),
            ViewportStability::Stable
        );
    }

    #[test]
    fn requested_restore_accepts_an_unobservable_window_position() {
        let now = Instant::now();
        let mut stabilizer = RootViewportStabilizer::new([1400.0, 900.0], Some([120.0, 80.0]), true);

        assert_eq!(stabilizer.poll([1400.0, 900.0], None, now), ViewportStability::Waiting);
        assert_eq!(
            stabilizer.poll([1400.0, 900.0], None, now + VIEWPORT_SETTLE_TIME),
            ViewportStability::Stable
        );
    }

    #[test]
    fn timeout_retries_once_against_observed_geometry() {
        let now = Instant::now();
        let mut stabilizer = RootViewportStabilizer::new([1400.0, 900.0], None, true);

        assert_eq!(stabilizer.poll([900.0, 700.0], None, now), ViewportStability::Waiting);
        assert_eq!(
            stabilizer.poll([900.0, 700.0], None, now + VIEWPORT_STABILIZATION_TIMEOUT),
            ViewportStability::TimedOut
        );
        assert!(stabilizer.begin_observed_geometry_fallback());
        assert_eq!(
            stabilizer.poll([900.0, 700.0], None, now + VIEWPORT_STABILIZATION_TIMEOUT),
            ViewportStability::Waiting
        );
        assert_eq!(
            stabilizer.poll(
                [900.0, 700.0],
                None,
                now + VIEWPORT_STABILIZATION_TIMEOUT + VIEWPORT_SETTLE_TIME
            ),
            ViewportStability::Stable
        );
        assert!(!stabilizer.begin_observed_geometry_fallback());
    }

    #[test]
    fn requested_geometry_regression_restarts_the_settle_window() {
        let now = Instant::now();
        let mut stabilizer = RootViewportStabilizer::new([1400.0, 900.0], Some([120.0, 80.0]), true);

        assert_eq!(
            stabilizer.poll([1400.0, 900.0], Some([120.0, 80.0]), now),
            ViewportStability::Waiting
        );
        assert_eq!(
            stabilizer.poll([900.0, 700.0], Some([10.0, 20.0]), now + VIEWPORT_SETTLE_TIME),
            ViewportStability::Waiting
        );
        assert_eq!(
            stabilizer.poll([1400.0, 900.0], Some([120.0, 80.0]), now + VIEWPORT_SETTLE_TIME * 2),
            ViewportStability::Waiting
        );
        assert_eq!(
            stabilizer.poll([1400.0, 900.0], Some([120.0, 80.0]), now + VIEWPORT_SETTLE_TIME * 3),
            ViewportStability::Stable
        );
    }
}
