//! The route to one cloud's worker, and the record check that proves it is ready.
//!
//! Two agent tools reach a worker: one runs a command, and one types into a session's agent
//! terminal. Both answer only for a cloud of the caller's own workspace that is ready, so
//! they resolve the route here rather than twice. Each tool names its own code prefix, so a
//! caller reads one error code per tool and never the code of the other.
use super::HorizonApp;
use crate::app::browser_requests::actor_panel;
use horizon_core::browser::manifest::provider_usage::UsageRequest;
use horizon_core::{
    browser::manifest,
    cloud_panel::Placement,
    cloud_runtime::{self, Stage, state::Store},
};
use std::path::PathBuf;

/// The route one request takes: a cloud's own record, which holds the host key Horizon
/// pinned for its worker.
#[derive(Debug)]
pub(super) struct Route {
    pub directory: PathBuf,
    pub settings: PathBuf,
    pub placement: Placement,
}

/// The codes and sentences of one cloud tool, so its refusals are its own.
#[derive(Clone, Copy, Debug)]
pub(super) struct Codes {
    /// The tool's code prefix, such as `cloud_exec`.
    pub prefix: &'static str,
    /// What the request carries, for an invalid one.
    pub request: &'static str,
    /// What a request that expired before Horizon read it never started.
    pub started: &'static str,
}

impl Route {
    /// The route to `cloud` in the caller's workspace, or the code that stops the call.
    pub(super) fn of(app: &HorizonApp, request: &UsageRequest, cloud: &str, codes: &Codes) -> Result<Self, String> {
        let workspace = (request.host_instance == manifest::host_instance())
            .then(|| actor_panel(&app.board, &request.actor))
            .flatten()
            .and_then(|panel| app.board.workspace(panel.workspace_id))
            .map(|workspace| workspace.local_id.clone())
            .ok_or_else(|| format!("{}_unavailable: requires a Horizon agent panel", codes.prefix))?;
        // Two saved clouds with one ID, in any workspace, make the ID ambiguous.
        let same_id = app
            .cloud_prototype
            .groups
            .0
            .iter()
            .filter(|group| group.remote.as_ref().is_some_and(|remote| remote.id == cloud))
            .count();
        let group = app
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.workspace == workspace && group.remote.as_ref().is_some_and(|r| r.id == cloud))
            .ok_or_else(|| {
                format!(
                    "{}_unknown_cloud: no cloud with this ID in your workspace; read cloud_list first",
                    codes.prefix
                )
            })?;
        if same_id != 1 {
            return Err(format!(
                "{}_ambiguous_cloud: more than one cloud has this ID; use the sidebar",
                codes.prefix
            ));
        }
        let root = app
            .cloud_prototype
            .root
            .clone()
            .ok_or_else(|| format!("{}_not_ready: Horizon has no cloud state directory", codes.prefix))?;
        // A caller that stopped waiting gets nothing that runs.
        if manifest::now_millis() >= request.deadline_at_millis {
            return Err(format!(
                "{}_expired: the request expired before Horizon started {}; nothing ran",
                codes.prefix, codes.started
            ));
        }
        let directory = cloud_runtime::state::cloud_directory(&root, cloud).map_err(|error| error.to_string())?;
        let placement = group
            .remote
            .as_ref()
            .map(|launch| launch.placement.clone())
            .unwrap_or_default();
        Ok(Self {
            directory,
            settings: root.join("settings.json"),
            placement,
        })
    }

    /// Whether the cloud's own record says that its worker is ready. A call changes a running
    /// worker, so a cloud that is deploying, stopped or deleted takes none.
    pub(super) fn ready(&self, codes: &Codes) -> Result<(), String> {
        // A removed folder is not made again: a call creates nothing on the cloud's disk.
        let Ok(store) = Store::lock_existing(&self.directory) else {
            return Err(format!("{}_not_ready: no deployment recorded", codes.prefix));
        };
        let stage = store
            .load()
            .map_err(|error| format!("{}_not_ready: {error}", codes.prefix))?
            .map(|state| state.stage);
        match stage {
            Some(Stage::Ready) => Ok(()),
            Some(stage) => Err(format!("{}_not_ready: {}", codes.prefix, stage.label())),
            None => Err(format!("{}_not_ready: no deployment recorded", codes.prefix)),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use horizon_core::browser::manifest::provider_usage::UsageRequest;

    const CODES: Codes = Codes {
        prefix: "cloud_exec",
        request: "one command for one cloud ID from cloud_list",
        started: "the command",
    };

    fn request(actor: &str, deadline_at_millis: i64) -> UsageRequest {
        serde_json::from_value(serde_json::json!({
            "request_id": "route", "actor": actor, "host_instance": manifest::host_instance(),
            "deadline_at_millis": deadline_at_millis, "claimed": true,
        }))
        .unwrap()
    }

    #[test]
    fn each_refusal_names_the_tool_that_made_it() {
        let (temp, app) = crate::app::test_support::test_app();
        let mut app = app;
        app.cloud_prototype.root = Some(temp.path().into());
        let codes = Codes {
            prefix: "cloud_agent",
            ..CODES
        };
        let refused = Route::of(&app, &request("horizon:nobody", i64::MAX), "cloud-1", &codes);
        assert_eq!(
            refused.unwrap_err(),
            "cloud_agent_unavailable: requires a Horizon agent panel"
        );
        let expired = request("horizon:nobody", manifest::now_millis() - 1);
        // The actor is checked first, so a stranger never learns that the request expired.
        assert_eq!(
            Route::of(&app, &expired, "cloud-1", &codes).unwrap_err(),
            "cloud_agent_unavailable: requires a Horizon agent panel"
        );
    }
}
