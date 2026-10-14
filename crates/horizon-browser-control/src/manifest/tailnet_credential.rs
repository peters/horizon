//! Nonsecret requests for native-owner credential removal. Actor identity is attribution,
//! never authority to mutate a desktop-global saved binding.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialOperation {
    Remove,
    Status,
    Cancel,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialRequest {
    pub operation: CredentialOperation,
    /// Exact saved ID from `cloud_companions`. No name, root or OS reference is accepted.
    pub tailnet_id: String,
    /// Caller-chosen UUID; retain it when an answer is lost and read status.
    pub request_id: String,
}
impl CredentialRequest {
    #[must_use]
    pub fn valid(&self) -> bool {
        !self.tailnet_id.is_empty()
            && self.tailnet_id.len() <= 64
            && !self.tailnet_id.starts_with("tskey-")
            && self.tailnet_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && self.request_id.len() == 36
            && uuid::Uuid::parse_str(&self.request_id).is_ok()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secret_owner_and_path_fields_are_never_accepted() {
        let value = serde_json::json!({"operation":"remove","tailnet_id":"work", "request_id":uuid::Uuid::new_v4().to_string()});
        assert!(
            serde_json::from_value::<CredentialRequest>(value.clone())
                .unwrap()
                .valid()
        );
        for field in [
            "token",
            "client_secret",
            "client_id",
            "root",
            "owner",
            "credential_reference",
        ] {
            let mut extra = value.clone();
            extra[field] = "untrusted".into();
            assert!(serde_json::from_value::<CredentialRequest>(extra).is_err());
        }
        for id in ["../work", "tskey-auth-secret", "", "work_name"] {
            let mut malformed = value.clone();
            malformed["tailnet_id"] = id.into();
            assert!(!serde_json::from_value::<CredentialRequest>(malformed).unwrap().valid());
        }
    }
    #[test]
    fn entry_operations_are_refused_until_native_owner_entry_is_available() {
        for operation in ["set_personal_token", "set_oauth"] {
            let request = serde_json::json!({"operation":operation,"tailnet_id":"work","request_id":uuid::Uuid::new_v4().to_string()});
            assert!(serde_json::from_value::<CredentialRequest>(request).is_err());
        }
    }
}
