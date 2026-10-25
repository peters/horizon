//! Agent requests to rent a cloud without a person, carried by the provider usage
//! queue. The host authorizes the machine's policy, the repository and its own
//! workspace, and owns the deployment; nothing here holds an authority of its own.
use serde::{Deserialize, Serialize};

/// What an agent asks of a cloud it rents. `submit` rents one cloud; `status` and
/// `cancel` act on the operation that `operation_id` names.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloudCreateOperation {
    Submit,
    Status,
    Cancel,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CloudCreateRequest {
    /// Chosen by the caller before queueing, so a lost answer is polled with `status`
    /// instead of renting a second cloud.
    pub operation_id: String,
    pub operation: CloudCreateOperation,
    /// `submit` only: the repository directory as a path on the Horizon host. The host
    /// resolves it to the Git repository of its commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// `submit` only: the name the cloud's card shows. The repository folder's name
    /// otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `submit` only: the built-in profile on the public base image, which is the only
    /// choice until committed `.horizon/cloud.yml` profiles are offered. It must be true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quick_start: Option<bool>,
}

impl CloudCreateRequest {
    /// The operation this request belongs to.
    #[must_use]
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    /// Whether the fields this operation needs are present and well formed. The host
    /// still authorizes the repository and the machine's policy against its own records.
    #[must_use]
    pub fn valid(&self) -> bool {
        let lower_uuid = |value: &str| {
            value.len() == 36 && uuid::Uuid::parse_str(value).is_ok_and(|parsed| parsed.to_string() == value)
        };
        let name = |value: &str| {
            !value.is_empty()
                && value.len() <= 128
                && !value.chars().any(|c| c.is_control() || matches!(c, '/' | '\\'))
                && value.trim() == value
        };
        let path = |value: &str| !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control);
        if !lower_uuid(&self.operation_id) {
            return false;
        }
        match self.operation {
            CloudCreateOperation::Submit => {
                self.quick_start == Some(true)
                    && self.repository.as_deref().is_some_and(path)
                    && self.name.as_deref().is_none_or(name)
                    && serde_json::to_vec(self).is_ok_and(|bytes| bytes.len() <= 8192)
            }
            CloudCreateOperation::Status | CloudCreateOperation::Cancel => {
                self.repository.is_none() && self.name.is_none() && self.quick_start.is_none()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "27d497e2-1f54-4bc4-b746-ab2a3d9efad5";

    fn request(value: serde_json::Value) -> Option<CloudCreateRequest> {
        serde_json::from_value(value).ok()
    }

    #[test]
    fn only_quick_start_of_one_repository_is_a_valid_submission() {
        let submit = serde_json::json!({
            "operation": "submit", "operation_id": ID,
            "repository": "/home/person/shop", "quick_start": true,
        });
        assert!(request(submit.clone()).unwrap().valid());
        let named = serde_json::json!({
            "operation": "submit", "operation_id": ID,
            "repository": "/home/person/shop", "name": "shop", "quick_start": true,
        });
        assert!(request(named).unwrap().valid());
        for (field, value) in [
            ("quick_start", serde_json::json!(null)),
            ("quick_start", serde_json::json!(false)),
            ("repository", serde_json::json!(null)),
            ("repository", serde_json::json!("")),
            ("repository", serde_json::json!("/home/person/\u{7f}")),
            ("name", serde_json::json!(" shop")),
            ("name", serde_json::json!("sh/op")),
            ("name", serde_json::json!("")),
            ("operation_id", serde_json::json!(ID.to_uppercase())),
            ("operation_id", serde_json::json!("not-an-id")),
        ] {
            let mut invalid = submit.clone();
            invalid[field] = value;
            assert!(!request(invalid).unwrap().valid(), "{field}");
        }
    }

    #[test]
    fn status_and_cancel_carry_only_a_caller_chosen_uuid() {
        for operation in ["status", "cancel"] {
            let value = serde_json::json!({"operation": operation, "operation_id": ID});
            assert!(request(value).unwrap().valid());
            let mut upper = serde_json::json!({"operation": operation, "operation_id": ID.to_uppercase()});
            assert!(!request(upper.clone()).unwrap().valid());
            // Neither control operation may carry a deploy input.
            upper["repository"] = serde_json::json!("/home/person/shop");
            assert!(!request(upper).unwrap().valid());
        }
        assert!(
            request(serde_json::json!({"operation": "delete", "operation_id": ID})).is_none(),
            "an unknown operation is a different request"
        );
    }
}
