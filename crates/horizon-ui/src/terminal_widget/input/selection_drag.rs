use std::time::Duration;

use alacritty_terminal::term::TermMode;
use egui::{Modifiers, Pos2};
use horizon_core::PanelId;

#[derive(Default)]
pub(crate) struct TerminalSelectionDragState {
    active: Option<ActiveSelectionDrag>,
    primary_gestures: Vec<ActivePrimaryGesture>,
}

#[derive(Clone, Copy)]
pub(super) enum CapturedPrimaryGesture {
    Horizon,
    Pty {
        terminal_mode: TermMode,
        modifiers: Modifiers,
    },
}

struct ActivePrimaryGesture {
    panel_id: PanelId,
    route: CapturedPrimaryGesture,
    captured_at: PrimaryGestureEvent,
    released_at: Option<PrimaryGestureEvent>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PrimaryGestureEvent {
    frame: u64,
    index: usize,
}

struct ActiveSelectionDrag {
    panel_id: PanelId,
    start_pos: Pos2,
    dragged: bool,
    next_auto_scroll_at: Option<f64>,
}

pub(super) enum AutoScrollCadence {
    Ready,
    Waiting(Duration),
}

impl TerminalSelectionDragState {
    pub(super) fn capture_primary_gesture(
        &mut self,
        panel_id: PanelId,
        route: CapturedPrimaryGesture,
        frame: u64,
        index: usize,
    ) {
        let captured_at = PrimaryGestureEvent { frame, index };
        if self
            .primary_gestures
            .iter()
            .any(|gesture| gesture.captured_at == captured_at)
        {
            return;
        }
        self.primary_gestures.push(ActivePrimaryGesture {
            panel_id,
            route,
            captured_at,
            released_at: None,
        });
    }

    pub(super) fn expire_completed_primary_gesture(&mut self, frame: u64) {
        if self
            .primary_gestures
            .iter()
            .any(|gesture| gesture.captured_at.frame == frame)
        {
            return;
        }
        let active = self
            .primary_gestures
            .iter()
            .max_by_key(|gesture| gesture.captured_at)
            .filter(|gesture| gesture.released_at.is_none())
            .map(|gesture| gesture.captured_at);
        self.primary_gestures
            .retain(|gesture| Some(gesture.captured_at) == active);
    }

    pub(super) fn has_primary_gesture_for(&self, panel_id: PanelId) -> bool {
        self.primary_gestures
            .iter()
            .any(|gesture| gesture.panel_id == panel_id && gesture.released_at.is_none())
    }

    pub(super) fn primary_gesture_for_event(
        &self,
        panel_id: PanelId,
        frame: u64,
        index: usize,
    ) -> Option<CapturedPrimaryGesture> {
        let event = PrimaryGestureEvent { frame, index };
        self.primary_gestures
            .iter()
            .filter(|gesture| {
                gesture.captured_at < event && gesture.released_at.is_none_or(|released| event < released)
            })
            .max_by_key(|gesture| gesture.captured_at)
            .filter(|gesture| gesture.panel_id == panel_id)
            .map(|gesture| gesture.route)
    }

    #[cfg(test)]
    pub(super) fn has_primary_gesture(&self) -> bool {
        !self.primary_gestures.is_empty()
    }

    pub(crate) fn cancel_interrupted_primary_gesture(
        &mut self,
        primary_down: bool,
        release_pending: bool,
        pointer_moved: bool,
    ) {
        if !primary_down && !release_pending && pointer_moved {
            self.primary_gestures.retain(|gesture| gesture.released_at.is_some());
        }
    }

    pub(super) fn finish_primary_gesture(
        &mut self,
        panel_id: PanelId,
        frame: u64,
        index: usize,
    ) -> Option<CapturedPrimaryGesture> {
        let released_at = PrimaryGestureEvent { frame, index };
        let gesture = self
            .primary_gestures
            .iter_mut()
            .filter(|gesture| {
                gesture.captured_at < released_at && gesture.released_at.is_none_or(|released| released_at < released)
            })
            .max_by_key(|gesture| gesture.captured_at)?;
        if gesture.panel_id != panel_id {
            return None;
        }
        gesture.released_at = Some(released_at);
        Some(gesture.route)
    }

    pub(crate) fn start(&mut self, panel_id: PanelId, start_pos: Pos2) {
        self.active = Some(ActiveSelectionDrag {
            panel_id,
            start_pos,
            dragged: false,
            next_auto_scroll_at: None,
        });
    }

    pub(crate) fn active_for(&self, panel_id: PanelId) -> bool {
        self.active.as_ref().is_some_and(|drag| drag.panel_id == panel_id)
    }

    pub(crate) fn mark_dragged(&mut self, panel_id: PanelId, pos: Pos2, movement_threshold: f32) {
        let Some(active) = self.active.as_mut().filter(|drag| drag.panel_id == panel_id) else {
            return;
        };
        let movement_threshold = movement_threshold.max(0.0);
        if movement_threshold.is_finite() && active.start_pos.distance_sq(pos) > movement_threshold * movement_threshold
        {
            active.dragged = true;
        }
    }

    pub(super) fn auto_scroll_cadence(&self, panel_id: PanelId, frame_time: f64) -> Option<AutoScrollCadence> {
        let active = self.active.as_ref().filter(|drag| drag.panel_id == panel_id)?;
        let next_auto_scroll_at = active.next_auto_scroll_at?;
        if !frame_time.is_finite() || frame_time >= next_auto_scroll_at {
            return Some(AutoScrollCadence::Ready);
        }

        Some(AutoScrollCadence::Waiting(Duration::from_secs_f64(
            next_auto_scroll_at - frame_time,
        )))
    }

    pub(super) fn record_auto_scroll(&mut self, panel_id: PanelId, frame_time: f64, interval: Duration) {
        let Some(active) = self.active.as_mut().filter(|drag| drag.panel_id == panel_id) else {
            return;
        };
        active.next_auto_scroll_at = frame_time.is_finite().then_some(frame_time + interval.as_secs_f64());
    }

    pub(super) fn clear_auto_scroll_cadence(&mut self, panel_id: PanelId) {
        if let Some(active) = self.active.as_mut().filter(|drag| drag.panel_id == panel_id) {
            active.next_auto_scroll_at = None;
        }
    }

    pub(crate) fn finish(&mut self, panel_id: PanelId) -> bool {
        let Some(active) = self.active.take() else {
            return false;
        };
        if active.panel_id == panel_id {
            active.dragged
        } else {
            self.active = Some(active);
            false
        }
    }
}

#[derive(Default)]
pub(super) struct SelectionFrameOutcome {
    pub(super) copy_completed_selection: bool,
    pub(super) release_completes_drag: bool,
    pub(super) release_replay_index: Option<usize>,
    pub(super) osc8_replay_index: Option<usize>,
}
