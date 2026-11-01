//! Where a new workspace runs: **+ New cloud** makes one with a cloud, and **New** in the
//! sidebar header makes an empty one on this computer.
use egui::Context;
use horizon_core::WorkspaceId;

use super::super::HorizonApp;

/// Why New cloud is dimmed, on hover.
pub(in crate::app) const NEW_CLOUD_NOT_READY: &str =
    "Cloud workspaces are not ready on this computer. Open Menu › Cloud settings.";

/// Where a new workspace runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum NewWorkspace {
    /// A cloud on a CPU worker: the default.
    Cloud,
    /// A cloud on a worker with a GPU. Only the canvas picker offers it.
    CloudGpu,
    /// This computer, as before clouds.
    ThisPc,
}

impl NewWorkspace {
    /// The cloud choices of the canvas picker.
    #[cfg(feature = "cloud-workspaces")]
    pub(in crate::app) const CLOUDS: [Self; 2] = [Self::Cloud, Self::CloudGpu];

    pub(in crate::app) fn label(self) -> &'static str {
        match self {
            Self::Cloud => "Cloud",
            Self::CloudGpu => "Cloud GPU",
            Self::ThisPc => "This PC",
        }
    }

    pub(in crate::app) fn detail(self) -> &'static str {
        match self {
            Self::Cloud => "Agents run on a cloud worker, also while this computer sleeps.",
            Self::CloudGpu => "Runs on a cloud worker with a GPU.",
            Self::ThisPc => "Terminals and agents run on this computer.",
        }
    }

    pub(in crate::app) fn is_cloud(self) -> bool {
        self != Self::ThisPc
    }
}

impl HorizonApp {
    /// Whether New cloud can start a cloud on this computer.
    pub(in crate::app) fn new_cloud_workspace_ready(&self) -> bool {
        #[cfg(feature = "cloud-workspaces")]
        {
            self.cloud_launch_ready()
        }
        #[cfg(not(feature = "cloud-workspaces"))]
        {
            false
        }
    }

    /// The workspace a new cloud may fill instead of a new one: the active workspace when it
    /// holds nothing, or else the first empty workspace on the board.
    #[cfg(feature = "cloud-workspaces")]
    fn empty_workspace_for_a_cloud(&self) -> Option<WorkspaceId> {
        if let Some(active) = self.board.active_workspace
            && self.board.workspace_holds_nothing(active)
        {
            return Some(active);
        }
        self.board
            .workspaces
            .iter()
            .map(|workspace| workspace.id)
            .find(|workspace| self.board.workspace_holds_nothing(*workspace))
    }

    /// Makes a workspace where `choice` says, at `canvas_pos` or where new workspaces go. A
    /// cloud workspace opens New cloud for it; a draft a dismissal parked comes back first.
    pub(in crate::app) fn create_new_workspace(
        &mut self,
        ctx: &Context,
        choice: NewWorkspace,
        canvas_pos: Option<[f32; 2]>,
    ) {
        #[cfg(feature = "cloud-workspaces")]
        if choice.is_cloud() && self.cloud_launch_ready() && self.reopen_cloud_draft(ctx) {
            return;
        }
        // A cloud takes an empty workspace that is already on the board before it makes
        // another one, so a cancelled draft never leaves a row behind. A workspace the person
        // made is theirs: the draft points at it without the intent that would remove it.
        #[cfg(feature = "cloud-workspaces")]
        let reused = if choice.is_cloud() && self.cloud_launch_ready() && canvas_pos.is_none() {
            self.empty_workspace_for_a_cloud()
        } else {
            None
        };
        // A build without clouds has nothing to fill, and the same workspace code runs.
        #[cfg(not(feature = "cloud-workspaces"))]
        let reused: Option<WorkspaceId> = None;
        let workspace = if let Some(empty) = reused {
            self.board.active_workspace = Some(empty);
            self.focus_workspace_visible(ctx, empty, false);
            empty
        } else {
            let name = format!("Workspace {}", self.board.workspaces.len() + 1);
            match canvas_pos {
                Some(position) => self.create_workspace_at_visible(ctx, &name, position),
                None => self.create_workspace_visible(ctx, &name),
            }
        };
        #[cfg(feature = "cloud-workspaces")]
        if choice.is_cloud() && self.cloud_launch_ready() {
            let gpu = choice == NewWorkspace::CloudGpu;
            if reused.is_some() {
                self.open_cloud_in_existing_workspace(ctx, workspace, gpu);
            } else {
                self.open_cloud_for_new_workspace(ctx, workspace, gpu);
            }
        }
        #[cfg(not(feature = "cloud-workspaces"))]
        let _ = (choice, workspace);
        self.mark_runtime_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_pc_is_not_a_cloud() {
        assert!(NewWorkspace::Cloud.is_cloud() && NewWorkspace::CloudGpu.is_cloud());
        assert!(!NewWorkspace::ThisPc.is_cloud());
    }
}
