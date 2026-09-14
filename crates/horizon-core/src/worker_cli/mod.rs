//! One task-owned home per worker. Observation never creates or replays work.

mod management;
mod manifest;
mod operations;
mod panels;
mod storage;
#[cfg(test)]
mod tests;

use horizon_core::{
    cloud_run::{GitSource, WorkerTarget},
    remote_provider_config::RemoteProviderConfig,
    remote_workspace::RemotePanelCommand,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{io::Read, path::PathBuf};
use storage::Context;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Intent {
    pub config: RemoteProviderConfig,
    pub target: WorkerTarget,
    pub repository: GitSource,
    pub command: RemotePanelCommand,
    pub working_directory: String,
    /// Setup authorization only; persistent execution and disk billing outlive it.
    pub setup_expires_at_millis: i64,
    pub issue: String,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum Error {
    #[error(
        "usage: horizon-worker <create|add-panel|check|git-prepare|git-install|git-status|start|status|snapshot|manifest|management-preview|stop|stop-check|compute-start|delete|delete-check> <private-new-or-existing-directory> [panel-id for start/status/snapshot]; create reads JSON, git-install reads a PAT; manifest reads repository YAML from the specified path"
    )]
    Usage,
    #[error("invalid or oversized input; no input values are echoed")]
    Input,
    #[error("private task storage is unavailable, busy, changed, or already exists")]
    Storage,
    #[error("operation was already claimed; inspect the original task instead of replaying")]
    Claimed,
    #[error("operation failed or is unconfirmed; retain the task directory and use check/status without replay")]
    Operation,
    #[error("remote operation failed or is unconfirmed: {0}; retain the original task directory")]
    Remote(String),
}

pub(super) fn input(limit: u64) -> Result<String, Error> {
    let mut value = String::new();
    std::io::stdin()
        .take(limit + 1)
        .read_to_string(&mut value)
        .map_err(|_| Error::Input)?;
    if value.len() as u64 > limit {
        return Err(Error::Input);
    }
    Ok(value)
}

pub fn run() -> Result<Value, Error> {
    let mut args = std::env::args_os().skip(1);
    let operation = args.next().ok_or(Error::Usage)?;
    let root = PathBuf::from(args.next().ok_or(Error::Usage)?);
    let selected_panel = args.next();
    if args.next().is_some() {
        return Err(Error::Usage);
    }
    let operation = operation.to_str().ok_or(Error::Usage)?;
    if selected_panel.is_some() && !matches!(operation, "start" | "status" | "snapshot") {
        return Err(Error::Usage);
    }
    if operation == "manifest" {
        return manifest::read(&root);
    }
    if !matches!(
        operation,
        "add-panel"
            | "create"
            | "check"
            | "git-prepare"
            | "git-install"
            | "git-status"
            | "start"
            | "status"
            | "snapshot"
            | "management-preview"
            | "stop"
            | "stop-check"
            | "compute-start"
            | "delete"
            | "delete-check"
    ) {
        return Err(Error::Usage);
    }
    if operation == "create" {
        let intent = serde_json::from_str(&input(131_072)?).map_err(|_| Error::Input)?;
        return operations::create(&root, intent);
    }
    let mut context = Context::open(&root)?;
    if let Some(panel) = selected_panel {
        let panel = panel.to_str().ok_or(Error::Input)?;
        if !context
            .saved()?
            .state()
            .spec
            .panels
            .iter()
            .any(|saved| saved.panel_local_id == panel)
        {
            return Err(Error::Input);
        }
        context.receipt.panel = panel.into();
    }
    let result = match operation {
        "management-preview" | "stop" | "stop-check" | "compute-start" | "delete" | "delete-check" => {
            management::run(&context, operation)
        }
        "add-panel" => panels::add(&context),
        "check" => operations::check(&context),
        "git-prepare" => operations::git(&context, false, false),
        "git-install" => operations::git(&context, true, false),
        "git-status" => operations::git(&context, false, true),
        "start" => operations::panel(&context, true),
        "status" => operations::panel(&context, false),
        "snapshot" => operations::terminal(&context),
        _ => Err(Error::Usage),
    }?;
    Ok(json!({"task": context.receipt.workspace, "result": result}))
}
