//! Immutable, non-secret worker startup data persisted with the creation spec.
use crate::CloudError;
use serde::{Deserialize, Serialize};

pub(crate) const ENVIRONMENT_KEY: &str = "HORIZON_WORKER_STARTUP";
const MAX_BYTES: usize = 8 * 1024;

/// Opaque application data, visible to the provider and worker environment.
/// Persist it before creation; later requests must use the same saved value.
/// This conveys no provider credentials or application-specific authority.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct StartupMetadata(String);

impl StartupMetadata {
    /// # Errors
    /// Rejects empty, oversized or control-character-bearing data without echoing it.
    pub fn new(value: String) -> Result<Self, CloudError> {
        if value.is_empty() || value.len() > MAX_BYTES || value.chars().any(char::is_control) {
            return Err(CloudError::Invalid("Invalid worker startup metadata"));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for StartupMetadata {
    type Error = CloudError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<StartupMetadata> for String {
    fn from(value: StartupMetadata) -> Self {
        value.0
    }
}

impl std::fmt::Debug for StartupMetadata {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("StartupMetadata([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing_is_bounded_and_errors_and_debug_do_not_echo_data() {
        for value in [
            String::new(),
            "x".repeat(MAX_BYTES + 1),
            "sensitive\nvalue".into(),
            "\0".into(),
        ] {
            let encoded = serde_json::to_string(&value).unwrap();
            let error = serde_json::from_str::<StartupMetadata>(&encoded).unwrap_err();
            assert!(!error.to_string().contains("sensitive"));
        }
        let value = StartupMetadata::new(r#"{"version":1,"binding":"synthetic"}"#.into()).unwrap();
        assert!(!format!("{value:?}").contains("synthetic"));
        assert_eq!(
            serde_json::from_str::<StartupMetadata>(&serde_json::to_string(&value).unwrap()).unwrap(),
            value
        );
        assert!(StartupMetadata::new("x".repeat(MAX_BYTES)).is_ok());
    }
}
