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
}

impl TerminalSelectionDragState {
    pub(crate) fn start(&mut self, panel_id: PanelId, start_pos: Pos2) {
        self.active = Some(ActiveSelectionDrag {
            panel_id,
            start_pos,
            dragged: false,
        });
    }

    pub(crate) fn active_for(&self, panel_id: PanelId) -> bool {
        self.active.as_ref().is_some_and(|drag| drag.panel_id == panel_id)
    }

    pub(crate) fn mark_dragged(&mut self, panel_id: PanelId, pos: Pos2) {
        let Some(active) = self.active.as_mut().filter(|drag| drag.panel_id == panel_id) else {
            return;
        };
        if active.start_pos.distance_sq(pos) > f32::EPSILON {
            active.dragged = true;
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
    pub(super) claimed_primary_pointer: bool,
}
