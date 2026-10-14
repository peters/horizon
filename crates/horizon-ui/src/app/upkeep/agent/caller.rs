//! Who asks through `cloud_upkeep`, and whether the caller still stands where it asked
//! from when its request is about to change something.
use horizon_core::WorkspaceId;
use horizon_core::browser::manifest::{
    self,
    provider_usage::{UpkeepRequest, UsageRequest},
};

use super::Caller;
use crate::app::HorizonApp;
use crate::app::browser_requests::actor_panel;

const NOT_AN_AGENT: &str = "upkeep_unavailable: requires a Horizon agent panel on this computer";

impl HorizonApp {
    /// Who asks: an agent panel of this Horizon, and the clouds of its workspace.
    pub(super) fn caller(&self, request: &UsageRequest) -> Result<(UpkeepRequest, Caller), String> {
        let upkeep = request
            .upkeep
            .clone()
            .filter(UpkeepRequest::valid)
            .ok_or("upkeep_invalid_request")?;
        let (_, local_id) = self.agent_workspace(request)?;
        let clouds: Vec<String> = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .filter(|group| group.workspace == local_id)
            .filter_map(|group| Some(group.remote.as_ref()?.id.clone()))
            .collect();
        let loops = clouds
            .iter()
            .map(|cloud| {
                let running = self
                    .upkeep
                    .syncs
                    .get(cloud)
                    .and_then(crate::app::upkeep::state::Sync::loop_running);
                (cloud.clone(), running)
            })
            .collect();
        let ambiguous = clouds
            .iter()
            .filter(|cloud| {
                let groups = &self.cloud_prototype.groups.0;
                groups
                    .iter()
                    .filter(|group| group.remote.as_ref().is_some_and(|remote| remote.id == **cloud))
                    .count()
                    != 1
            })
            .cloned()
            .collect();
        Ok((
            upkeep,
            Caller {
                home: local_id,
                clouds,
                loops,
                ambiguous,
            },
        ))
    }

    /// The workspace of the agent panel that sent `request`, by id and local id. A cloud's
    /// panel shows an agent that runs on its worker, such as the Upkeep agent: only an agent
    /// on this computer may ask.
    fn agent_workspace(&self, request: &UsageRequest) -> Result<(WorkspaceId, String), String> {
        if request.host_instance != manifest::host_instance() {
            return Err(NOT_AN_AGENT.into());
        }
        let panel = actor_panel(&self.board, &request.actor).ok_or(NOT_AN_AGENT)?;
        let member = self.board.panel(panel.panel_id).is_none_or(|held| {
            self.cloud_prototype
                .groups
                .0
                .iter()
                .any(|group| group.panels.contains(&held.local_id))
        });
        if member {
            return Err(NOT_AN_AGENT.into());
        }
        let workspace = self.board.workspace(panel.workspace_id).ok_or(NOT_AN_AGENT)?;
        Ok((panel.workspace_id, workspace.local_id.clone()))
    }

    /// The workspace the caller asked from, if the caller is still an agent panel there.
    pub(super) fn still_caller(&self, request: &UsageRequest, home: &str) -> Result<WorkspaceId, String> {
        match self.agent_workspace(request)? {
            (workspace, local_id) if local_id == home => Ok(workspace),
            _ => Err("upkeep_unavailable: the agent panel left the workspace it asked from; ask again".into()),
        }
    }

    /// Whether `cloud` is still in the workspace with the local id `home`.
    pub(super) fn cloud_in(&self, home: &str, cloud: &str) -> Result<(), String> {
        let found = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .any(|group| group.workspace == home && group.remote.as_ref().is_some_and(|remote| remote.id == cloud));
        if found {
            Ok(())
        } else {
            Err("upkeep_unavailable: the Upkeep is no longer in this workspace".into())
        }
    }
}
