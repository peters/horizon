//! Slots of a workspace preset. Every visible panel takes one slot of the same size.
use crate::WorkspaceLayout;
use crate::layout::point_in_panel;
use crate::panel::PanelId;
use crate::workspace::WorkspaceId;

use super::super::Board;
use super::arranged_panel_layout;

/// One cell of a workspace preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    Panel(PanelId),
}

impl Board {
    /// The slots of `id` in preset order.
    pub(crate) fn arranged_slots(&self, id: WorkspaceId) -> Vec<Slot> {
        let Some(workspace) = self.workspace(id) else {
            return Vec::new();
        };
        workspace
            .panels
            .iter()
            .filter(|panel_id| self.panel_follows_workspace_layout(**panel_id))
            .map(|panel_id| Slot::Panel(*panel_id))
            .collect()
    }

    /// Where a slot is and how large, as `(position, size)`.
    pub(crate) fn slot_rect(&self, slot: Slot) -> Option<([f32; 2], [f32; 2])> {
        match slot {
            Slot::Panel(id) => self.panel(id).map(|panel| (panel.layout.position, panel.layout.size)),
        }
    }

    /// Place every slot of a preset at `size`. Returns the largest frame the slots
    /// need.
    pub(super) fn place_slots(
        &mut self,
        workspace: WorkspaceId,
        origin: [f32; 2],
        layout: WorkspaceLayout,
        slots: &[Slot],
        size: [f32; 2],
    ) -> [f32; 2] {
        let needed = size;
        let count = slots.len();
        let _ = workspace;
        for (index, slot) in slots.iter().enumerate() {
            let (position, cell) = arranged_panel_layout(origin, layout, index, count, size);
            match *slot {
                Slot::Panel(id) => {
                    if let Some(panel) = self.panel_mut(id) {
                        panel.move_to(position);
                        panel.resize_layout(cell);
                    }
                }
            }
        }
        needed
    }

    /// Reorder an arranged workspace by moving the slot that holds `source` onto the slot
    /// under `point`. Returns whether the order changed.
    pub(crate) fn swap_slot_at(&mut self, workspace: WorkspaceId, source: Slot, point: [f32; 2]) -> bool {
        if !point[0].is_finite() || !point[1].is_finite() {
            return false;
        }
        let Some(layout) = self.workspace_layout_value(workspace) else {
            return false;
        };
        let slots = self.arranged_slots(workspace);
        let Some(from) = slots.iter().position(|slot| *slot == source) else {
            return false;
        };
        let Some(to) = slots.iter().enumerate().position(|(index, slot)| {
            index != from
                && self
                    .slot_rect(*slot)
                    .is_some_and(|(position, size)| point_in_panel(point, position, size))
        }) else {
            return false;
        };
        let mut order = slots;
        order.swap(from, to);
        if !self.write_slot_order(workspace, &order) {
            return false;
        }
        self.apply_workspace_layout(workspace, layout);
        true
    }

    /// Store a slot order in the workspace's panel order, which is what is saved.
    /// Panels outside the preset keep their relative order after them.
    fn write_slot_order(&mut self, workspace: WorkspaceId, order: &[Slot]) -> bool {
        let Some(current) = self.workspace(workspace).map(|ws| ws.panels.clone()) else {
            return false;
        };
        let mut panels: Vec<PanelId> = Vec::with_capacity(current.len());
        for (position, slot) in order.iter().enumerate() {
            let _ = position;
            match *slot {
                Slot::Panel(id) => panels.push(id),
            }
        }
        let ordered_set: Vec<PanelId> = panels.clone();
        // Only the places of the panels the slots stand for change; hidden panels keep
        // theirs, so they return where they were.
        let mut ordered = panels.into_iter();
        let reordered: Vec<PanelId> = current
            .iter()
            .map(|id| {
                if ordered_set.contains(id) {
                    ordered.next().unwrap_or(*id)
                } else {
                    *id
                }
            })
            .collect();
        let Some(ws) = self.workspace_mut(workspace) else {
            return false;
        };
        ws.panels = reordered;
        true
    }
}
