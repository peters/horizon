use serde::{Deserialize, Serialize};

use crate::RoutineError;
use crate::fingerprint::TargetFingerprint;
use crate::origin::Origin;

const MAX_ASSERTION_BYTES: usize = 4 * 1024;

/// Observable page condition used as a precondition, postcondition, or
/// completion check. Page-supplied scripts are not assertions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Assertion {
    UrlPattern { value: String },
    Heading { value: String },
    ElementPresent { target: TargetFingerprint },
    ElementAbsent { target: TargetFingerprint },
    TextShape { value: String },
}

impl Assertion {
    pub(crate) fn validate(&self) -> Result<(), RoutineError> {
        match self {
            Self::UrlPattern { value } => validate_url_pattern(value),
            Self::Heading { value } | Self::TextShape { value } => validate_text(value),
            Self::ElementPresent { target } | Self::ElementAbsent { target } => target.validate(),
        }
    }
}

fn validate_url_pattern(value: &str) -> Result<(), RoutineError> {
    validate_text(value)?;
    if value.starts_with('/') {
        return Ok(());
    }
    Origin::parse(value)
        .map(|_| ())
        .map_err(|_| RoutineError::InvalidAssertion)
}

fn validate_text(value: &str) -> Result<(), RoutineError> {
    if value.is_empty() || value.len() > MAX_ASSERTION_BYTES || value.chars().any(char::is_control) {
        return Err(RoutineError::InvalidAssertion);
    }
    if value.contains('?') || value.contains('#') {
        return Err(RoutineError::InvalidAssertion);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Assertion;
    use crate::RoutineError;

    #[test]
    fn url_pattern_rejects_query_and_fragment() {
        assert_eq!(
            Assertion::UrlPattern {
                value: "https://reports.example/done?token=1".to_string()
            }
            .validate(),
            Err(RoutineError::InvalidAssertion)
        );
        assert_eq!(
            Assertion::UrlPattern {
                value: "/reports/done".to_string()
            }
            .validate(),
            Ok(())
        );
    }

    #[test]
    fn unknown_assertion_type_is_rejected() {
        let err = serde_json::from_value::<Assertion>(serde_json::json!({
            "type": "script",
            "value": "window.ok"
        }));
        assert!(err.is_err());
        let err = serde_json::from_value::<Assertion>(serde_json::json!({
            "type": "accessible_state",
            "value": "selected"
        }));
        assert!(err.is_err());
    }
}
