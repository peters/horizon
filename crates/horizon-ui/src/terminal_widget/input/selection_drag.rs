use std::time::Duration;

use egui::Pos2;
use horizon_core::PanelId;

#[derive(Default)]
pub(crate) struct TerminalSelectionDragState {
    active: Option<ActiveSelectionDrag>,
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
}
