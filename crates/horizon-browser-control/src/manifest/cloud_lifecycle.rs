//! Agent requests to stop or resume one cloud's worker, carried by the provider usage
//! queue. The host authorizes the caller's own cloud, on this machine's own credentials,
//! from its own records; nothing here holds an authority of its own. A delete is
//! `cloud_delete`, which also proves what the provider removed.
use serde::{Deserialize, Serialize};

/// What an agent asks of one cloud. `submit` starts the action; `status` reads the
/// operation that `operation_id` names. A stop or a resume cannot be recalled once the
/// provider request is sent, so there is no cancel.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloudLifecycleOperation {
    Submit,
    Status,
}

/// The power action of one submission.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloudLifecycleAction {
    /// Ends the worker's compute and keeps what the provider keeps of it, so a resume
    /// finds the same workspace.
    Stop,
    /// Starts a stopped cloud's worker again.
    Resume,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CloudLifecycleRequest {
    /// Chosen by the caller before queueing, so a lost answer is polled with `status`
    /// instead of acting a second time.
    pub operation_id: String,
    pub operation: CloudLifecycleOperation,
    /// `submit` only: the action to carry out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<CloudLifecycleAction>,
    /// `submit` only: the cloud's ID from `cloud_list`, in the caller's workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<String>,
}

impl CloudLifecycleRequest {
    /// The operation this request belongs to.
    #[must_use]
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    /// Whether the fields this operation needs are present and well formed. The host
    /// still authorizes the cloud, and its creator, against its own records.
    #[must_use]
    pub fn valid(&self) -> bool {
        let lower_uuid = |value: &str| {
            value.len() == 36 && uuid::Uuid::parse_str(value).is_ok_and(|parsed| parsed.to_string() == value)
        };
        let cloud = |value: &str| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        };
        if !lower_uuid(&self.operation_id) {
            return false;
        }
        match self.operation {
            CloudLifecycleOperation::Submit => self.action.is_some() && self.cloud.as_deref().is_some_and(cloud),
            CloudLifecycleOperation::Status => self.action.is_none() && self.cloud.is_none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "27d497e2-1f54-4bc4-b746-ab2a3d9efad5";

    fn request(value: serde_json::Value) -> Option<CloudLifecycleRequest> {
        serde_json::from_value(value).ok()
    }

    #[test]
    fn a_submission_names_one_action_and_one_cloud_of_the_callers_workspace() {
        for action in ["stop", "resume"] {
            let submit =
                serde_json::json!({"operation": "submit", "operation_id": ID, "action": action, "cloud": "cloud-1"});
            assert!(request(submit.clone()).unwrap().valid(), "{action}");
        }
        let submit =
            serde_json::json!({"operation": "submit", "operation_id": ID, "action": "stop", "cloud": "cloud-1"});
        for (field, value) in [
            ("action", serde_json::json!(null)),
            ("cloud", serde_json::json!(null)),
            ("cloud", serde_json::json!("")),
            ("cloud", serde_json::json!("../cloud-1")),
            ("cloud", serde_json::json!("cloud 1")),
            ("operation_id", serde_json::json!(ID.to_uppercase())),
            ("operation_id", serde_json::json!("not-an-id")),
        ] {
            let mut invalid = submit.clone();
            invalid[field] = value;
            assert!(!request(invalid).unwrap().valid(), "{field}");
        }
        // No caller chooses a provider, forces a stop or names a repository.
        for field in ["provider", "force", "repository", "worker"] {
            let mut widened = submit.clone();
            widened[field] = serde_json::json!("x");
            assert!(request(widened).is_none(), "{field} is a different request");
        }
    }

    #[test]
    fn a_status_carries_only_a_caller_chosen_uuid() {
        let value = serde_json::json!({"operation": "status", "operation_id": ID});
        assert!(request(value).unwrap().valid());
        for (field, value) in [
            ("action", serde_json::json!("stop")),
            ("cloud", serde_json::json!("cloud-1")),
        ] {
            let mut widened = serde_json::json!({"operation": "status", "operation_id": ID});
            widened[field] = value;
            assert!(!request(widened).unwrap().valid(), "{field}");
        }
        assert!(
            request(serde_json::json!({"operation": "cancel", "operation_id": ID})).is_none(),
            "an action that cannot be recalled has no cancel"
        );
        assert!(
            request(serde_json::json!({"operation": "submit", "operation_id": ID, "action": "delete", "cloud": "c"}))
                .is_none(),
            "a delete is cloud_delete, which also proves absence"
        );
    }
}
