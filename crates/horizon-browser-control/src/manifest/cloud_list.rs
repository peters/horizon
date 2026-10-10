//! Agent requests for the cloud list, carried by the provider usage queue. The host
//! answers only for the clouds in the caller's workspace and authorizes every field
//! against its own records.
use serde::{Deserialize, Serialize};

/// What an agent asks of the cloud list. `list` only reads; the others act on one
/// cloud as the sidebar and the cloud card do.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloudListOperation {
    #[default]
    List,
    /// Moves the person's view to the cloud, as a click on its row does.
    Attach,
    /// Parks the terminals of a cloud that is out of view now.
    Park,
    /// Stops the worker of an idle cloud.
    Stop,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CloudListRequest {
    pub operation: CloudListOperation,
    /// The cloud's ID from `list`. Omitted only by `list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<String>,
}

impl CloudListRequest {
    /// Whether the fields this operation needs are present and well formed. The host
    /// still authorizes the cloud against its own records.
    #[must_use]
    pub fn valid(&self) -> bool {
        let cloud = self.cloud.as_ref().is_some_and(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        });
        match self.operation {
            CloudListOperation::List => self.cloud.is_none(),
            CloudListOperation::Attach | CloudListOperation::Park | CloudListOperation::Stop => cloud,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_list_goes_without_a_cloud_and_unknown_fields_are_refused() {
        let request = |value: serde_json::Value| serde_json::from_value::<CloudListRequest>(value);
        assert!(request(serde_json::json!({"operation": "list"})).unwrap().valid());
        assert!(
            !request(serde_json::json!({"operation": "list", "cloud": "a"}))
                .unwrap()
                .valid()
        );
        for operation in ["attach", "park", "stop"] {
            assert!(
                request(serde_json::json!({"operation": operation, "cloud": "cloud-1"}))
                    .unwrap()
                    .valid()
            );
            assert!(!request(serde_json::json!({"operation": operation})).unwrap().valid());
            assert!(
                !request(serde_json::json!({"operation": operation, "cloud": "a/b"}))
                    .unwrap()
                    .valid()
            );
        }
        assert!(request(serde_json::json!({"operation": "list", "workspace": "w"})).is_err());
        assert!(request(serde_json::json!({"operation": "delete", "cloud": "a"})).is_err());
    }
}
