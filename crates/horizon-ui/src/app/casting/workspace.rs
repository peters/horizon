//! The cast button in a workspace's toolbar.
use super::super::HorizonApp;
use super::Anchor;
use egui::Context;
use horizon_core::{WorkspaceId, browser::manifest::cast::CastSource};

impl HorizonApp {
    /// Whether a workspace's cast button shows as on: one of its casts runs, or the
    /// button opened the picker.
    pub(in crate::app) fn workspace_cast_highlighted(&self, workspace: WorkspaceId) -> bool {
        self.casting
            .picker
            .as_ref()
            .is_some_and(|picker| picker.workspace == workspace && picker.anchor == Anchor::Workspace)
            || self
                .casting
                .sessions
                .iter()
                .any(|session| session.workspace == workspace && !session.worker.finished())
    }

    /// Opens the Cast picker on `workspace` from its toolbar, or closes it when that
    /// button opened it.
    pub(in crate::app) fn toggle_workspace_cast_picker(&mut self, workspace: WorkspaceId, ctx: &Context) {
        let Some(id) = self.board.workspace(workspace).map(|value| value.local_id.clone()) else {
            return;
        };
        self.casting
            .toggle_picker(Anchor::Workspace, workspace, CastSource::Workspace { id }, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{editor_workspace_state, test_app_with_startup};
    use horizon_core::{RuntimeState, StartupDecision};

    #[test]
    fn the_toolbar_button_opens_and_closes_a_workspace_picker() {
        let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState {
                workspaces: vec![editor_workspace_state("synthetic", [0.0, 0.0])],
                ..RuntimeState::default()
            }),
        });
        // A pending discovery keeps the picker from searching the real network.
        let (_send, receive) = std::sync::mpsc::channel();
        app.casting.discovery = Some(receive);
        let workspace = app.board.workspaces[0].id;
        let local = app.board.workspaces[0].local_id.clone();
        assert!(!app.workspace_cast_highlighted(workspace));
        app.toggle_workspace_cast_picker(workspace, &ctx);
        let picker = app.casting.picker.as_ref().expect("the button opens the picker");
        assert_eq!(picker.source, CastSource::Workspace { id: local });
        assert_eq!(picker.anchor, Anchor::Workspace);
        assert!(app.workspace_cast_highlighted(workspace));
        app.toggle_workspace_cast_picker(workspace, &ctx);
        assert!(app.casting.picker.is_none(), "the same button closes it");
        assert!(!app.workspace_cast_highlighted(workspace));
    }
}
