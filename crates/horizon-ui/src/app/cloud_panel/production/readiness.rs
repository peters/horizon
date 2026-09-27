//! Presentation gates for adding panels to an existing cloud.
use super::{HorizonApp, Production, Stage};
use horizon_core::{WorkspaceId, cloud_panel::CloudGroup};

impl Production {
    pub(in crate::app::cloud_panel) fn accepts_panels(&self, group: &CloudGroup) -> bool {
        group.remote.is_none()
            || self.runtimes.get(&group.issue).is_some_and(|runtime| {
                runtime.stage == Some(Stage::Ready)
                    && !runtime.state_unavailable
                    && runtime.recovery_receiver.is_none()
                    && runtime.state.as_ref().is_some_and(|state| state.stage == Stage::Ready)
            })
    }
}

impl HorizonApp {
    pub(in crate::app) fn preset_target_accepts_cloud_panels(
        &self,
        workspace: Option<WorkspaceId>,
        position: [f32; 2],
    ) -> bool {
        workspace
            .and_then(|id| self.cloud_prototype.groups.at_position(&self.board, id, position))
            .is_none_or(|index| {
                self.cloud_prototype
                    .production
                    .accepts_panels(&self.cloud_prototype.groups.0[index])
            })
    }
}

#[cfg(test)]
mod tests;
