//! Agent requests for the cloud list: the clouds in the caller's workspace, and
//! attach, park and stop of one of them, as the sidebar and the cloud card do.
use super::super::{Stage, bulk_stop::IdleStop};
use super::HorizonApp;
use crate::app::browser_requests::actor_panel;
use horizon_core::{
    browser::manifest::{
        self,
        provider_usage::{CloudListOperation, UsageRequest},
    },
    cloud_list::CloudFacts,
    cloud_panel::park::Sight,
};
use serde_json::{Value, json};
use std::time::{Instant, SystemTime};

impl HorizonApp {
    pub(in crate::app) fn queue_cloud_list_request(&mut self, request: UsageRequest) {
        self.cloud_prototype.production.list_requests.push(request);
    }

    /// Answers the queued cloud list requests.
    pub(in crate::app::cloud_panel) fn answer_cloud_list_requests(&mut self, ctx: &egui::Context) {
        for request in std::mem::take(&mut self.cloud_prototype.production.list_requests) {
            let answer = self.answer_cloud_list(&request, ctx);
            let mut result = request.result(Vec::new(), None);
            match answer {
                Ok(value) => result.cloud_list = Some(value),
                Err(error) => result.error = Some(error),
            }
            if let Err(error) = manifest::provider_usage::complete_provider_usage(&result) {
                tracing::warn!(kind = ?error.kind(), "could not answer a cloud list request");
            }
        }
    }

    fn answer_cloud_list(&mut self, request: &UsageRequest, ctx: &egui::Context) -> Result<Value, String> {
        let list = request
            .cloud_list
            .as_ref()
            .filter(|list| list.valid())
            .ok_or("cloud_list_invalid_request")?;
        let workspace = (request.host_instance == manifest::host_instance())
            .then(|| actor_panel(&self.board, &request.actor))
            .flatten()
            .and_then(|panel| self.board.workspace(panel.workspace_id))
            .map(|workspace| workspace.local_id.clone())
            .ok_or("cloud_list_unavailable: requires a Horizon agent panel")?;
        let now = SystemTime::now();
        let in_workspace = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .enumerate()
            .filter(|(_, group)| group.workspace == workspace)
            .filter_map(|(index, group)| Some((index, group.remote.as_ref()?.id.clone())));
        if list.operation == CloudListOperation::List {
            let clouds: Vec<Value> = in_workspace
                .map(|(index, id)| entry(&id, &self.cloud_facts(&self.cloud_prototype.groups.0[index], now)))
                .collect();
            return Ok(json!({ "clouds": clouds }));
        }
        // A caller that stopped waiting gets no change it did not see.
        if manifest::now_millis() >= request.deadline_at_millis {
            return Err("cloud_list_expired: the request expired before Horizon answered it; nothing changed".into());
        }
        let wanted = list.cloud.as_deref().unwrap_or_default();
        let index = in_workspace
            .filter(|(_, id)| id == wanted)
            .map(|(index, _)| index)
            .next()
            .ok_or("cloud_list_unknown_cloud: no cloud with this ID in your workspace; read list first")?;
        // Two saved clouds with one ID, in any workspace, make the ID ambiguous.
        let same_id = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .filter(|group| group.remote.as_ref().is_some_and(|remote| remote.id == wanted))
            .count();
        if same_id != 1 {
            return Err("cloud_list_ambiguous_cloud: more than one cloud has this ID; use the sidebar".into());
        }
        let group = &self.cloud_prototype.groups.0[index];
        let issue = group.issue;
        match list.operation {
            CloudListOperation::List => unreachable!("answered above"),
            CloudListOperation::Attach => {
                self.attach_listed_cloud(index, ctx);
                Ok(json!({ "cloud": wanted, "attach": "in_view" }))
            }
            CloudListOperation::Park => {
                let runtime = self.cloud_prototype.production.runtimes.get(&issue);
                let parking = runtime.map(|runtime| &runtime.parking);
                // A parked cloud in view attaches in the next frames, so it is refused too.
                if parking.is_some_and(super::super::park::Parking::is_parked) {
                    if self.cloud_sight(index) != Sight::Hidden {
                        return Err(
                            "cloud_list_in_view: a cloud in view attaches again; it parks when out of view".into(),
                        );
                    }
                    return Ok(json!({ "cloud": wanted, "park": "parked" }));
                }
                // A cloud whose terminals still attach after Ready, or that is in another
                // operation, is not tracked.
                if runtime.is_none_or(|runtime| {
                    runtime.needs_attach
                        || !runtime.pending_session_attachments.is_empty()
                        || !runtime.pending_member_attachments.is_empty()
                        || runtime.stage != Some(Stage::Ready)
                        || runtime.state.as_ref().is_none_or(|state| state.stage != Stage::Ready)
                }) || !parking.is_some_and(super::super::park::Parking::attached)
                {
                    return Err("cloud_list_not_ready: only a ready cloud parks".into());
                }
                if self.cloud_sight(index) != Sight::Hidden {
                    return Err("cloud_list_in_view: a cloud in view attaches again; it parks when out of view".into());
                }
                if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&issue) {
                    runtime.parking.request_park();
                }
                ctx.request_repaint();
                Ok(json!({ "cloud": wanted, "park": "parking" }))
            }
            CloudListOperation::Stop => {
                if !self.cloud_facts(group, now).idle() {
                    return Err(
                        "cloud_list_not_idle: only an idle cloud stops; it is busy, an agent works on it, or it waits for the person"
                            .into(),
                    );
                }
                let stop = match self.stop_idle_cloud(issue, ctx, Instant::now()) {
                    IdleStop::Stopping => "stopping",
                    IdleStop::Waiting => "waiting_for_status",
                    IdleStop::Busy => return Err("cloud_list_not_idle: the cloud became busy".into()),
                };
                Ok(json!({ "cloud": wanted, "stop": stop }))
            }
        }
    }

    /// Moves the view to cloud `index` as a click on its row does: to its first
    /// member, which takes the focus so that a parked cloud attaches at once.
    fn attach_listed_cloud(&mut self, index: usize, ctx: &egui::Context) {
        let group = &self.cloud_prototype.groups.0[index];
        let member = group
            .panels
            .iter()
            .find_map(|local| self.board.panel_id_by_local_id(local));
        let workspace = self
            .board
            .workspaces
            .iter()
            .find(|workspace| workspace.local_id == group.workspace)
            .map(|workspace| workspace.id);
        if let Some(workspace) = workspace
            && self.focus_workspace_window(ctx, workspace)
        {
            if let Some(member) = member {
                self.board.focus(member);
            }
            return;
        }
        match (member, workspace) {
            (Some(member), _) => self.reveal_panel_visible(ctx, member),
            (None, Some(workspace)) => {
                self.focus_workspace_visible(ctx, workspace, true);
            }
            (None, None) => {}
        }
        ctx.request_repaint();
    }
}

/// One cloud of a `list` answer.
fn entry(id: &str, facts: &CloudFacts) -> Value {
    json!({
        "cloud": id,
        "name": facts.name,
        "group": facts.condition.group().key(),
        "line": facts.line,
        "hourly_rate": facts.hourly_rate,
        "working": facts.working,
        "idle": facts.idle(),
    })
}

#[cfg(all(test, unix))]
mod tests;
