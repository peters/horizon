//! The New workspace menu: a new workspace runs in the cloud unless the person picks This PC.
use egui::{Context, Popup, Response, RichText};

use crate::theme;

use super::super::HorizonApp;

/// Where a new workspace runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum NewWorkspace {
    /// A cloud on a CPU worker: the default.
    Cloud,
    /// A cloud on a worker with a GPU.
    CloudGpu,
    /// This computer, as before clouds.
    ThisPc,
}

impl NewWorkspace {
    /// In the order the menu shows them, the default first.
    pub(in crate::app) const ALL: [Self; 3] = [Self::Cloud, Self::CloudGpu, Self::ThisPc];

    pub(in crate::app) fn label(self) -> &'static str {
        match self {
            Self::Cloud => "Cloud",
            Self::CloudGpu => "Cloud GPU",
            Self::ThisPc => "This PC",
        }
    }

    pub(in crate::app) fn detail(self) -> &'static str {
        match self {
            Self::Cloud => "Default. Runs on a cloud worker.",
            Self::CloudGpu => "Runs on a cloud worker with a GPU.",
            Self::ThisPc => "Runs on this computer.",
        }
    }

    pub(in crate::app) fn is_cloud(self) -> bool {
        self != Self::ThisPc
    }
}

/// The menu that `button` opens. Without cloud workspaces ready, only This PC can be chosen.
pub(super) fn menu(button: &Response, cloud_ready: bool) -> Option<NewWorkspace> {
    let mut chosen = None;
    Popup::menu(button).show(|ui| {
        ui.set_min_width(230.0);
        for choice in NewWorkspace::ALL {
            let enabled = cloud_ready || !choice.is_cloud();
            let text = RichText::new(choice.label()).size(13.0).color(theme::FG());
            let row = ui
                .add_enabled(enabled, egui::Button::new(text).frame(false))
                .on_hover_text(choice.detail())
                .on_disabled_hover_text("Cloud workspaces are not ready on this computer.");
            if row.clicked() {
                chosen = Some(choice);
                ui.close();
            }
        }
    });
    chosen
}

impl HorizonApp {
    /// Whether the New workspace menu can offer the cloud.
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
    /// cloud workspace opens New cloud for it.
    pub(in crate::app) fn create_new_workspace(
        &mut self,
        ctx: &Context,
        choice: NewWorkspace,
        canvas_pos: Option<[f32; 2]>,
    ) {
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
    fn the_cloud_comes_first_and_this_pc_last() {
        assert_eq!(
            NewWorkspace::ALL.map(NewWorkspace::label),
            ["Cloud", "Cloud GPU", "This PC"]
        );
        assert!(NewWorkspace::Cloud.is_cloud() && NewWorkspace::CloudGpu.is_cloud());
        assert!(!NewWorkspace::ThisPc.is_cloud());
    }
}
