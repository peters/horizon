//! Exact-revision lifecycle confirmations; never coupled to detach or observation.

use super::{Error, input, storage::Context};
use horizon_core::{
    cloud_run::CloudProvider,
    remote_environment_delete as deletion,
    remote_workspace::{RemoteEnvironmentSummary, start, stop},
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirmation {
    workspace: String,
    revision: u64,
    resource_id: String,
    action: String,
    acknowledge_data_loss: bool,
}

fn confirm(text: &str, workspace: &str, revision: u64, resource_id: &str, action: &str) -> Result<(), Error> {
    let confirmation: Confirmation = serde_json::from_str(text).map_err(|_| Error::Input)?;
    if confirmation.workspace != workspace
        || confirmation.revision != revision
        || confirmation.resource_id != resource_id
        || confirmation.action != action
        || (matches!(action, "delete" | "delete-retry") && !confirmation.acknowledge_data_loss)
    {
        return Err(Error::Input);
    }
    Ok(())
}

fn phase(saved: &RemoteEnvironmentSummary) -> Value {
    json!({"revision": saved.revision, "phase": format!("{:?}", saved.saved_phase)})
}

pub(super) fn run(context: &Context, operation: &str) -> Result<Value, Error> {
    let expected = context.saved()?.environment_summary();
    let config = &context.receipt.intent.config;
    if operation == "management-preview" {
        let identity = expected.worker_identity.as_ref().ok_or(Error::Operation)?;
        return Ok(
            json!({"workspace": expected.workspace_local_id, "revision": expected.revision,
            "provider": expected.provider, "resource_id": identity.resource_id,
            "profile": expected.profile, "target": context.receipt.intent.target,
            "scope": "Azure Delete removes the owned resource group, worker and retained disk; unpushed data and running tasks are lost. Stop retains disk billing. Compute-start does not resume terminated tasks.",
            "supported": expected.provider == CloudProvider::Azure}),
        );
    }
    if expected.provider != CloudProvider::Azure {
        return Err(Error::Input);
    }
    let store = context.store()?;
    match operation {
        "stop-check" => {
            let result = stop::confirm_configured_remote_environment_stop(&store, config, &expected)
                .map_err(|error| Error::Remote(error.to_string()))?;
            Ok(json!({"saved": phase(&result.saved), "observation": format!("{:?}", result.observation)}))
        }
        "delete-check" => {
            let result = deletion::confirm_configured_remote_environment_deletion(&store, config, &expected)
                .map_err(|error| Error::Remote(error.to_string()))?;
            Ok(json!({"saved": phase(&result.saved), "absence_verified": result.absence_verified}))
        }
        "stop" | "compute-start" | "delete" | "delete-retry" => {
            confirm(
                &input(4096)?,
                &expected.workspace_local_id,
                expected.revision,
                &expected.worker_identity.as_ref().ok_or(Error::Operation)?.resource_id,
                operation,
            )?;
            // The configured coordinators journal lifecycle intent before dispatch.
            // A second local claim would strand proven pre-dispatch failures and
            // separately confirmed retries. Observation never enters these paths.
            match operation {
                "stop" => stop::stop_configured_azure_environment(&store, config, &expected)
                    .map(|saved| phase(&saved))
                    .map_err(|error| Error::Remote(error.to_string())),
                "compute-start" => start::start_configured_azure_environment(&store, config, &expected)
                    .map(|result| {
                        json!({"saved": phase(&result.saved), "lifecycle": format!("{:?}", result.lifecycle),
                        "already_running": result.already_running})
                    })
                    .map_err(|error| Error::Remote(error.to_string())),
                "delete-retry" => deletion::retry_configured_remote_environment_deletion(&store, config, &expected)
                    .map(|result| json!({"saved": phase(&result.saved), "absence_verified": result.absence_verified}))
                    .map_err(|error| Error::Remote(error.to_string())),
                "delete" => deletion::delete_configured_remote_environment(&store, config, &expected)
                    .map(|result| json!({"saved": phase(&result.saved), "absence_verified": result.absence_verified}))
                    .map_err(|error| Error::Remote(error.to_string())),
                _ => Err(Error::Usage),
            }
        }
        _ => Err(Error::Usage),
    }
}

#[cfg(test)]
mod tests {
    use super::confirm;
    use serde_json::json;

    #[test]
    fn lifecycle_confirmation_binds_action_identity_revision_and_data_loss() {
        let good = json!({"workspace":"task-a","revision":3,"resource_id":"resource-a","action":"delete","acknowledge_data_loss":true});
        assert!(confirm(&good.to_string(), "task-a", 3, "resource-a", "delete").is_ok());
        assert!(confirm(&good.to_string(), "task-b", 3, "resource-a", "delete").is_err());
        assert!(confirm(&good.to_string(), "task-a", 4, "resource-a", "delete").is_err());
        assert!(confirm(&good.to_string(), "task-a", 3, "resource-a", "stop").is_err());
        assert!(confirm(&good.to_string(), "task-a", 3, "resource-b", "delete").is_err());
        let mut denied = good;
        denied["acknowledge_data_loss"] = json!(false);
        assert!(confirm(&denied.to_string(), "task-a", 3, "resource-a", "delete").is_err());
        denied["action"] = json!("delete-retry");
        assert!(confirm(&denied.to_string(), "task-a", 3, "resource-a", "delete-retry").is_err());
        denied["acknowledge_data_loss"] = json!(true);
        assert!(confirm(&denied.to_string(), "task-a", 3, "resource-a", "delete-retry").is_ok());
    }
}
