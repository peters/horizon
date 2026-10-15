//! Thin host authorization for the shared destination manager. No filesystem I/O here.
use super::HorizonApp;
use crate::app::browser_requests::actor_panel;
use horizon_core::{
    Board,
    browser::manifest::{self, provider_usage::UsageRequest},
    cloud_panel::CloudGroups,
    cloud_runtime::publishing::operations::LaunchOwner,
};
use std::{path::Path, sync::Arc};

fn repository<'a>(board: &'a Board, groups: &CloudGroups, actor: &str) -> Option<&'a Path> {
    let actor = actor_panel(board, actor)?;
    let panel = board.panel(actor.panel_id)?;
    if panel.remote_workspace().is_some()
        || panel.cloud_wait().is_some()
        || panel.ssh_connection.is_some()
        || groups.0.iter().any(|group| group.panels.contains(&panel.local_id))
    {
        return None;
    }
    panel
        .launch_cwd
        .as_deref()
        .or_else(|| board.workspace(actor.workspace_id)?.cwd.as_deref())
}

impl HorizonApp {
    pub(in crate::app) fn poll_image_destination_operations(&mut self, ctx: &egui::Context) {
        if self.browser_create_host.provider_usage.image_destinations.running() {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
        let board = &self.board;
        let root = self.cloud_prototype.root.as_deref();
        let groups = &self.cloud_prototype.groups;
        self.browser_create_host
            .provider_usage
            .image_destinations
            .poll(|owner| {
                owner.host == manifest::host_instance()
                    && root == Some(owner.root.as_path())
                    && repository(board, groups, &owner.actor) == Some(owner.repository.as_path())
            });
    }

    pub(in crate::app) fn answer_image_destination(&mut self, request: &UsageRequest, ctx: &egui::Context) {
        let result = self.image_destination_result(request, ctx);
        self.publish_host_usage_result(result);
    }

    fn image_destination_result(
        &mut self,
        request: &UsageRequest,
        ctx: &egui::Context,
    ) -> horizon_core::browser::manifest::provider_usage::UsageResult {
        let answer = (|| -> Result<serde_json::Value, String> {
            if request.host_instance != manifest::host_instance()
                || manifest::now_millis() >= request.deadline_at_millis
            {
                return Err("image_destination_unavailable: caller or request expired".into());
            }
            let input = request
                .image_destination
                .as_ref()
                .filter(|input| input.valid())
                .ok_or("image_destination_invalid_request")?;
            let repository = repository(&self.board, &self.cloud_prototype.groups, &request.actor)
                .ok_or("image_destination_unavailable: a local agent checkout is required")?;
            let root = self
                .cloud_prototype
                .root
                .as_ref()
                .ok_or("image_destination_unavailable: host cloud storage is unavailable")?;
            let owner = LaunchOwner {
                actor: request.actor.clone(),
                host: request.host_instance.clone(),
                root: root.clone(),
                directory: repository.to_path_buf(),
            };
            let ctx = ctx.clone();
            let action = serde_json::to_value(&input.action).map_err(|_| "image_destination_invalid_request")?;
            let status = self
                .browser_create_host
                .provider_usage
                .image_destinations
                .dispatch_launch(
                    owner,
                    &input.operation_id,
                    input.profile.clone(),
                    action,
                    Arc::new(move || ctx.request_repaint()),
                )
                .map_err(|error| error.to_string())?;
            serde_json::to_value(status).map_err(|_| "image_destination_result_unavailable".into())
        })();
        let mut result = request.result(Vec::new(), None);
        match answer {
            Ok(value) => result.image_destination = Some(value),
            Err(error) => result.error = Some(error),
        }
        result
    }
}

#[cfg(test)]
mod tests;
