//! Where a new workspace runs: **+ New cloud** makes one with a cloud, and **New** in the
//! sidebar header makes an empty one on this computer.
use egui::Context;

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
        let name = format!("Workspace {}", self.board.workspaces.len() + 1);
        let workspace = match canvas_pos {
            Some(position) => self.create_workspace_at_visible(ctx, &name, position),
            None => self.create_workspace_visible(ctx, &name),
        };
        #[cfg(feature = "cloud-workspaces")]
        if choice.is_cloud() && self.cloud_launch_ready() {
            self.open_cloud_for_new_workspace(ctx, workspace, choice == NewWorkspace::CloudGpu);
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
