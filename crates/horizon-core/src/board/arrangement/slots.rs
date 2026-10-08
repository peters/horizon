//! Slots of a workspace preset. Ordinary panels and production clouds each take one
//! slot of the same size; a cloud fits its frame to the slot and arranges its own
//! members inside it.
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
    /// Index into the board's cloud groups.
    #[cfg(feature = "cloud-workspaces")]
    Cloud(usize),
}

impl Board {
    /// The slots of `id` in preset order. A cloud takes its slot where its first member
    /// stands in the workspace's panel order; a cloud without visible members comes last.
    pub(crate) fn arranged_slots(&self, id: WorkspaceId) -> Vec<Slot> {
        let Some(workspace) = self.workspace(id) else {
            return Vec::new();
        };
        let mut slots: Vec<Slot> = Vec::new();
        for panel_id in &workspace.panels {
            if self.panel_follows_workspace_layout(*panel_id) {
                slots.push(Slot::Panel(*panel_id));
                continue;
            }
            #[cfg(feature = "cloud-workspaces")]
            if let Some(index) = self.slot_cloud_of(*panel_id, id)
                && !slots.contains(&Slot::Cloud(index))
            {
                slots.push(Slot::Cloud(index));
            }
        }
        #[cfg(feature = "cloud-workspaces")]
        {
            // Clouds without members keep the place they were last given.
            let mut empty: Vec<usize> = self
                .slot_clouds(id)
                .into_iter()
                .filter(|index| !slots.contains(&Slot::Cloud(*index)))
                .collect();
            empty.sort_by_key(|index| (self.cloud_groups.0[*index].slot.unwrap_or(usize::MAX), *index));
            for index in empty {
                let at = self.cloud_groups.0[index].slot.unwrap_or(usize::MAX).min(slots.len());
                slots.insert(at, Slot::Cloud(index));
            }
        }
        slots
    }

    /// Where a slot is and how large, as `(position, size)`.
    pub(crate) fn slot_rect(&self, slot: Slot) -> Option<([f32; 2], [f32; 2])> {
        match slot {
            Slot::Panel(id) => self.panel(id).map(|panel| (panel.layout.position, panel.layout.size)),
            #[cfg(feature = "cloud-workspaces")]
            Slot::Cloud(index) => self.cloud_groups.0.get(index).map(|group| (group.position, group.size)),
        }
    }

    /// Place every slot of a preset at `size`. Returns the largest frame a cloud needed:
    /// a cloud cannot shrink below its chrome and smallest members, so it may come back
    /// larger than the slot it was given.
    pub(super) fn place_slots(
        &mut self,
        workspace: WorkspaceId,
        origin: [f32; 2],
        layout: WorkspaceLayout,
        slots: &[Slot],
        size: [f32; 2],
    ) -> [f32; 2] {
        let mut needed = size;
        let count = slots.len();
        #[cfg(feature = "cloud-workspaces")]
        let mut groups = std::mem::take(&mut self.cloud_groups);
        #[cfg(feature = "cloud-workspaces")]
        let workspace_position = self.workspace(workspace).map_or(origin, |ws| ws.position);
        #[cfg(not(feature = "cloud-workspaces"))]
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
                #[cfg(feature = "cloud-workspaces")]
                Slot::Cloud(group) => {
                    if let Some(group) = groups.0.get_mut(group) {
                        group.fit_slot(self, workspace_position, position, cell);
                        needed = [needed[0].max(group.size[0]), needed[1].max(group.size[1])];
                    }
                }
            }
        }
        #[cfg(feature = "cloud-workspaces")]
        {
            self.cloud_groups = groups;
        }
        needed
    }

    /// Move every slot by `delta`; a cloud moves with its members.
    pub(super) fn translate_slots(&mut self, slots: &[Slot], delta: [f32; 2]) {
        for slot in slots {
            match *slot {
                Slot::Panel(id) => {
                    if let Some(panel) = self.panel_mut(id) {
                        let position = panel.layout.position;
                        panel.move_to([position[0] + delta[0], position[1] + delta[1]]);
                    }
                }
                #[cfg(feature = "cloud-workspaces")]
                Slot::Cloud(index) => {
                    let mut groups = std::mem::take(&mut self.cloud_groups);
                    if let Some(group) = groups.0.get_mut(index) {
                        group.shift_in_slot(self, delta);
                    }
                    self.cloud_groups = groups;
                }
            }
        }
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

    /// Store a slot order in the workspace's panel order, which is what is saved. A cloud's
    /// members stand together where its slot is; panels outside the preset keep their
    /// relative order after them. A cloud without members has no place in the panel order,
    /// so it remembers its slot number instead.
    fn write_slot_order(&mut self, workspace: WorkspaceId, order: &[Slot]) -> bool {
        let Some(current) = self.workspace(workspace).map(|ws| ws.panels.clone()) else {
            return false;
        };
        let mut panels: Vec<PanelId> = Vec::with_capacity(current.len());
        #[cfg(feature = "cloud-workspaces")]
        let mut places: Vec<(usize, Option<usize>)> = Vec::new();
        for (position, slot) in order.iter().enumerate() {
            #[cfg(not(feature = "cloud-workspaces"))]
            let _ = position;
            match *slot {
                Slot::Panel(id) => panels.push(id),
                #[cfg(feature = "cloud-workspaces")]
                Slot::Cloud(index) => {
                    let members: Vec<PanelId> = current
                        .iter()
                        .copied()
                        .filter(|id| self.slot_cloud_of(*id, workspace) == Some(index))
                        .collect();
                    // A cloud with members is placed by them; an empty one remembers its slot.
                    places.push((index, members.is_empty().then_some(position)));
                    panels.extend(members);
                }
            }
        }
        #[cfg(feature = "cloud-workspaces")]
        for (index, place) in places {
            self.cloud_groups.0[index].slot = place;
        }
        let rest: Vec<PanelId> = current.iter().copied().filter(|id| !panels.contains(id)).collect();
        panels.extend(rest);
        let Some(ws) = self.workspace_mut(workspace) else {
            return false;
        };
        ws.panels = panels;
        true
    }

    /// Apply each preset that has a cloud slot, so restored clouds stand in their slots
    /// even when they were saved elsewhere.
    #[cfg(feature = "cloud-workspaces")]
    pub(crate) fn place_slot_clouds(&mut self) {
        let workspaces: Vec<WorkspaceId> = self
            .workspaces
            .iter()
            .map(|workspace| workspace.id)
            .filter(|id| self.workspace_layout_value(*id).is_some() && !self.slot_clouds(*id).is_empty())
            .collect();
        for workspace in workspaces {
            self.reapply_workspace_layout_if_set(workspace);
        }
    }

    /// The cloud whose slot `panel` belongs to, when that cloud takes a slot in `workspace`.
    #[cfg(feature = "cloud-workspaces")]
    pub(crate) fn slot_cloud_of(&self, panel: PanelId, workspace: WorkspaceId) -> Option<usize> {
        let local = &self.panel(panel)?.local_id;
        let index = self
            .cloud_groups
            .0
            .iter()
            .position(|group| group.panels.contains(local))?;
        self.slot_clouds(workspace).contains(&index).then_some(index)
    }

    /// Clouds that take a slot in `workspace`'s preset.
    #[cfg(feature = "cloud-workspaces")]
    pub(crate) fn slot_clouds(&self, workspace: WorkspaceId) -> Vec<usize> {
        let Some(local) = self.workspace(workspace).map(|ws| ws.local_id.as_str()) else {
            return Vec::new();
        };
        self.cloud_groups
            .0
            .iter()
            .enumerate()
            .filter(|(_, group)| group.workspace == local && group.takes_workspace_slot())
            .map(|(index, _)| index)
            .collect()
    }
}
