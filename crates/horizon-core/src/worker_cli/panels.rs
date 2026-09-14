//! Save additional task intent with a caller-owned idempotency key before dispatch.

use super::{
    Error, input,
    storage::{self, Context},
};
use horizon_core::remote_workspace::{
    RemotePanelCommand,
    panels::{self, RemoteShellPanelDraft},
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    operation_id: uuid::Uuid,
    command: RemotePanelCommand,
    directory: String,
}

pub(super) fn add(context: &Context) -> Result<Value, Error> {
    let request: Request = serde_json::from_str(&input(65_536)?).map_err(|_| Error::Input)?;
    if request.operation_id.is_nil() {
        return Err(Error::Input);
    }
    let store = context.store()?;
    let expected = context.saved()?.environment_summary();
    let prepared = panels::prepare_remote_shell_panel(
        &store,
        &context.receipt.session,
        &expected,
        RemoteShellPanelDraft {
            command: request.command,
            working_directory: Some(request.directory),
        },
    )
    .map_err(|error| Error::Remote(error.to_string()))?;
    let path = context.receipt.root.join(format!("add-{}.json", request.operation_id));
    storage::write_new(
        &path,
        &serde_json::to_vec(&json!({"panel": prepared.panel().panel_local_id,
        "operation_id": request.operation_id}))
        .map_err(|_| Error::Storage)?,
    )
    .map_err(|_| Error::Claimed)?;
    let added = panels::add_remote_shell_panel(&store, &context.receipt.session, &expected, prepared)
        .map_err(|error| Error::Remote(error.to_string()))?;
    Ok(json!({"panel": added.panel_id, "revision": added.environment.revision, "status": "saved_not_started"}))
}
