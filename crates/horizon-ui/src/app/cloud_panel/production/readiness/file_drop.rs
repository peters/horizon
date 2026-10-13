//! The worker session that a file drop on a cloud agent or shell panel goes to.
use super::{HorizonApp, Stage};
use crate::app::ssh_upload::{CloudDrop, CloudDropTarget};
use horizon_core::cloud_runtime::file_drop::Destination;
use horizon_core::{CloudWait, Panel, PanelId, PanelKind, cloud_panel::CloudGroup};

impl HorizonApp {
    /// Where a drop on `panel_id` goes. `None` for a panel outside every cloud, which keeps
    /// the local behavior; otherwise the bound worker session, or why the drop must stop.
    pub(in crate::app) fn cloud_file_drop_target(&self, panel_id: PanelId) -> Option<CloudDropTarget> {
        let panel = self.board.panel(panel_id)?;
        if matches!(panel.kind, PanelKind::Browser | PanelKind::Device) {
            return None;
        }
        let group = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.remote.is_some() && group.panels.contains(&panel.local_id))?;
        Some(CloudDropTarget {
            label: format!("{} in {}", panel.display_title(), group.title),
            drop: self.cloud_drop(group, panel),
        })
    }

    fn cloud_drop(&self, group: &CloudGroup, panel: &Panel) -> Result<CloudDrop, String> {
        let disconnected = || {
            format!(
                "{} is not connected, so Horizon cannot send files to it. Reconnect the cloud, then drop the files again.",
                group.title
            )
        };
        let production = &self.cloud_prototype.production;
        let runtime = production.runtimes.get(&group.issue);
        if !production.accepts_panels(group) {
            let stopped = panel.cloud_wait() == Some(CloudWait::Stopped)
                || runtime.is_some_and(|runtime| runtime.stage == Some(Stage::Stopped));
            if stopped {
                return Err(format!(
                    "{} is stopped. Resume the worker, then drop the files again.",
                    group.title
                ));
            }
            return Err(disconnected());
        }
        if panel.cloud_wait().is_some() {
            return Err(format!(
                "This panel is not attached to its session on {} yet. Drop the files again when the panel shows its session.",
                group.title
            ));
        }
        let launch = group.remote.as_ref().ok_or_else(disconnected)?;
        let root = self.cloud_prototype.root.as_ref().ok_or_else(disconnected)?;
        let worker = runtime
            .and_then(|runtime| runtime.state.as_ref())
            .and_then(|state| state.worker.as_ref())
            .ok_or_else(disconnected)?;
        let destination =
            Destination::new(worker, root, &launch.id, &panel.local_id).map_err(|error| error.to_string())?;
        Ok(CloudDrop {
            cloud: group.issue,
            destination,
        })
    }

    /// The panel that shows the session of `drop` on the same worker now, if any. A panel
    /// of that session that reconnected counts; a detached panel or another worker does not.
    pub(in crate::app) fn cloud_file_drop_recipient(&self, drop: &CloudDrop) -> Option<PanelId> {
        let session = drop.destination.session();
        let group = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.issue == drop.cloud && group.remote.is_some())?;
        if !group.panels.iter().any(|local| local == session) {
            return None;
        }
        let worker = self
            .cloud_prototype
            .production
            .runtimes
            .get(&drop.cloud)?
            .state
            .as_ref()?
            .worker
            .as_ref()?;
        if worker.id != drop.destination.worker_id() {
            return None;
        }
        let panel_id = self.board.panel_id_by_local_id(session)?;
        let panel = self.board.panel(panel_id)?;
        (panel.cloud_wait().is_none() && panel.terminal().is_some()).then_some(panel_id)
    }
}

#[cfg(test)]
mod tests;
