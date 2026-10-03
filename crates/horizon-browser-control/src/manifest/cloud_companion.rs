//! Agent requests for a source cloud's companion clouds, carried by the provider
//! usage queue. The host authorizes every field against its own records.
use serde::{Deserialize, Serialize};

/// What an agent asks of a source cloud's companions. Ensure Ready and Stop are
/// explicit lifecycle requests; `list` and `status` only read.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompanionAction {
    List,
    EnsureReady,
    Stop,
    Status,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CompanionRequest {
    pub action: CompanionAction,
    /// The source cloud's ID, in the caller's workspace. Omitted only by `list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<String>,
    /// The companion's alias in the source repository's `.horizon/cloud.yml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// Chosen by the caller before queueing, so a lost answer is polled with
    /// `status` instead of submitted again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    /// Saved tailnet ID at provisioning. "none" selects no network; omission preserves the choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailnet: Option<String>,
}

impl CompanionRequest {
    /// Whether the fields this action needs are present and well formed. The host
    /// still authorizes every value against its own records.
    #[must_use]
    pub fn valid(&self) -> bool {
        let name = |value: &Option<String>| {
            value.as_ref().is_some_and(|value| {
                !value.is_empty()
                    && value.len() <= 128
                    && value
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            })
        };
        let operation = self.operation_id.as_ref().is_some_and(|id| {
            id.len() == 36
                && id.chars().enumerate().all(|(i, c)| {
                    if matches!(i, 8 | 13 | 18 | 23) {
                        c == '-'
                    } else {
                        c.is_ascii_hexdigit()
                    }
                })
        });
        if self.tailnet.as_ref().is_some_and(|id| {
            self.action != CompanionAction::EnsureReady
                || !name(&Some(id.clone()))
                || id.len() > 64
                || id.starts_with("tskey-")
        }) {
            return false;
        }
        match self.action {
            CompanionAction::List => self.cloud.is_none() && self.alias.is_none() && self.operation_id.is_none(),
            CompanionAction::EnsureReady | CompanionAction::Stop | CompanionAction::Status => {
                name(&self.cloud) && name(&self.alias) && operation
            }
        }
    }
}

/// A fresh operation ID for an Ensure Ready or Stop request. Keep it: a caller whose
/// answer is lost polls `status` with it instead of sending the request again.
#[must_use]
pub fn new_operation_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_ensure_ready_selects_a_nonsecret_network_and_never_accepts_keys() {
        let value = serde_json::json!({"action":"ensure_ready","cloud":"source","alias":"worker",
            "operation_id":new_operation_id(),"tailnet":"work"});
        assert!(
            serde_json::from_value::<CompanionRequest>(value.clone())
                .unwrap()
                .valid()
        );
        for action in ["stop", "status", "list"] {
            let mut invalid = value.clone();
            invalid["action"] = action.into();
            assert!(!serde_json::from_value::<CompanionRequest>(invalid).unwrap().valid());
        }
        for field in ["auth_key", "key", "oauth_secret"] {
            let mut invalid = value.clone();
            invalid[field] = "synthetic".into();
            assert!(serde_json::from_value::<CompanionRequest>(invalid).is_err());
        }
        let mut invalid = value;
        invalid["tailnet"] = "tskey-auth-synthetic12345678901234567890".into();
        assert!(!serde_json::from_value::<CompanionRequest>(invalid).unwrap().valid());
    }
}
