//! Agent requests to delete one cloud's provider resources, carried by the provider usage
//! queue. The host authorizes the caller's own cloud, on this machine's own credentials,
//! from its own records; nothing here holds an authority of its own.
use serde::{Deserialize, Serialize};

/// What an agent asks of one cloud. `submit` starts the delete; `status` and `cancel` act
/// on the operation that `operation_id` names.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloudDeleteOperation {
    Submit,
    Status,
    Cancel,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CloudDeleteRequest {
    /// Chosen by the caller before queueing, so a lost answer is polled with `status`
    /// instead of deleting a second time.
    pub operation_id: String,
    pub operation: CloudDeleteOperation,
    /// `submit` only: the cloud's ID from `cloud_list`, in the caller's workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<String>,
}

impl CloudDeleteRequest {
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
            CloudDeleteOperation::Submit => self.cloud.as_deref().is_some_and(cloud),
            CloudDeleteOperation::Status | CloudDeleteOperation::Cancel => self.cloud.is_none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "27d497e2-1f54-4bc4-b746-ab2a3d9efad5";

    fn request(value: serde_json::Value) -> Option<CloudDeleteRequest> {
        serde_json::from_value(value).ok()
    }

    #[test]
    fn a_submission_names_one_cloud_of_the_callers_workspace() {
        let submit = serde_json::json!({"operation": "submit", "operation_id": ID, "cloud": "cloud-1"});
        assert!(request(submit.clone()).unwrap().valid());
        for (field, value) in [
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
        // No caller chooses a provider, forces a delete or names a repository.
        for field in ["provider", "force", "repository"] {
            let mut widened = submit.clone();
            widened[field] = serde_json::json!("x");
            assert!(request(widened).is_none(), "{field} is a different request");
        }
    }

    #[test]
    fn status_and_cancel_carry_only_a_caller_chosen_uuid() {
        for operation in ["status", "cancel"] {
            let value = serde_json::json!({"operation": operation, "operation_id": ID});
            assert!(request(value).unwrap().valid());
            let mut widened = serde_json::json!({"operation": operation, "operation_id": ID});
            widened["cloud"] = serde_json::json!("cloud-1");
            assert!(!request(widened).unwrap().valid());
        }
        assert!(
            request(serde_json::json!({"operation": "remove", "operation_id": ID})).is_none(),
            "an unknown operation is a different request"
        );
    }
}
