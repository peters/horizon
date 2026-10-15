//! Nonsecret destination requests. Only the host supplies repository and settings authority.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImageDestinationRequest {
    /// Caller-chosen lowercase UUID. Keep it after a lost answer.
    pub operation_id: String,
    /// Named working-copy profile. Omit for status/cancel.
    pub profile: Option<String>,
    pub action: Action,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    List {},
    Select {
        shown: Option<Shown>,
        destination: RegistryChoice,
    },
    Reset {
        shown: Option<Shown>,
    },
    Authenticate {
        shown: Option<Shown>,
        #[serde(default)]
        switch: bool,
    },
    Status {},
    Cancel {},
}

/// Exact nonsecret snapshot returned as `shown_override`; it is never authorization.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Shown {
    pub repository: PathBuf,
    pub profile: String,
    pub destination: String,
    pub account: Option<Account>,
    pub binding: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub id: u64,
    pub login: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RegistryChoice {
    Registry { destination: Destination },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    pub repository: String,
    pub publisher: Publisher,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Publisher {
    Registry { binding: String },
}

impl ImageDestinationRequest {
    #[must_use]
    pub fn valid(&self) -> bool {
        let id = uuid::Uuid::parse_str(&self.operation_id).is_ok_and(|id| id.to_string() == self.operation_id);
        let control = matches!(self.action, Action::Status {} | Action::Cancel {});
        let profile = self.profile.as_ref().is_some_and(|name| !name.is_empty());
        id && (if control { self.profile.is_none() } else { profile })
            && serde_json::to_vec(self).is_ok_and(|bytes| bytes.len() <= 8192)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_nonsecret_actions_and_caller_receipt_metadata_are_accepted() {
        let base = serde_json::json!({"operation_id":"27d497e2-1f54-4bc4-b746-ab2a3d9efad5","profile":"cpu","action":{"operation":"list"}});
        assert!(
            serde_json::from_value::<ImageDestinationRequest>(base.clone())
                .unwrap()
                .valid()
        );
        for key in ["root", "repository", "settings", "token", "credential"] {
            let mut wrong = base.clone();
            wrong[key] = "synthetic-private".into();
            assert!(serde_json::from_value::<ImageDestinationRequest>(wrong).is_err());
        }
        for operation in ["list", "authenticate", "select", "reset", "status", "cancel"] {
            let mut wrong = base.clone();
            wrong["action"] = serde_json::json!({"operation":operation,"token":"synthetic-private"});
            assert!(serde_json::from_value::<ImageDestinationRequest>(wrong).is_err());
        }
        let mut status = base;
        status["action"] = serde_json::json!({"operation":"status"});
        assert!(
            !serde_json::from_value::<ImageDestinationRequest>(status.clone())
                .unwrap()
                .valid()
        );
        status.as_object_mut().unwrap().remove("profile");
        assert!(
            serde_json::from_value::<ImageDestinationRequest>(status)
                .unwrap()
                .valid()
        );
    }
}
