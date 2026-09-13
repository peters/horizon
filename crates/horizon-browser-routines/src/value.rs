use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::RoutineError;

const MAX_IDENTIFIER_BYTES: usize = 64;
const MAX_LITERAL_BYTES: usize = 64 * 1024;

/// Where a fill or select step gets its value. Secret bytes are never stored
/// here; password-classified fields must use [`ValueSource::CredentialField`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ValueSource {
    Literal { value: String },
    Variable { name: String },
    CredentialField { slot: Uuid, field: CredentialFieldKind },
}

/// Opt-in OS-store field inside one opaque slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialFieldKind {
    Username,
    Password,
}

/// How the recorder classified the focused control. This is metadata, not a
/// secret.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldClassification {
    #[default]
    Ordinary,
    Username,
    Password,
}

impl ValueSource {
    pub(crate) fn validate(&self, classification: FieldClassification) -> Result<(), RoutineError> {
        match self {
            Self::Literal { value } => {
                if matches!(
                    classification,
                    FieldClassification::Password | FieldClassification::Username
                ) {
                    return Err(RoutineError::SecretLiteral);
                }
                if value.len() > MAX_LITERAL_BYTES {
                    return Err(RoutineError::LiteralTooLarge);
                }
                if value.chars().any(char::is_control) {
                    return Err(RoutineError::LiteralControl);
                }
                Ok(())
            }
            Self::Variable { name } => {
                if matches!(
                    classification,
                    FieldClassification::Password | FieldClassification::Username
                ) {
                    return Err(RoutineError::SecretLiteral);
                }
                validate_identifier(name)
            }
            Self::CredentialField { field, .. } => match (classification, *field) {
                (FieldClassification::Password, CredentialFieldKind::Password)
                | (FieldClassification::Username | FieldClassification::Ordinary, CredentialFieldKind::Username) => {
                    Ok(())
                }
                _ => Err(RoutineError::CredentialFieldMismatch),
            },
        }
    }
}

pub(crate) fn validate_identifier(name: &str) -> Result<(), RoutineError> {
    if name.is_empty()
        || name.len() > MAX_IDENTIFIER_BYTES
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(RoutineError::InvalidIdentifier);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CredentialFieldKind, FieldClassification, ValueSource};
    use crate::RoutineError;
    use uuid::Uuid;

    #[test]
    fn password_classification_rejects_literals_and_variables() {
        let literal = ValueSource::Literal {
            value: "typed".to_string(),
        };
        assert_eq!(
            literal.validate(FieldClassification::Password),
            Err(RoutineError::SecretLiteral)
        );
        let variable = ValueSource::Variable {
            name: "report_month".to_string(),
        };
        assert_eq!(
            variable.validate(FieldClassification::Password),
            Err(RoutineError::SecretLiteral)
        );
        assert_eq!(
            literal.validate(FieldClassification::Username),
            Err(RoutineError::SecretLiteral)
        );
    }

    #[test]
    fn password_field_accepts_a_credential_placeholder() {
        let source = ValueSource::CredentialField {
            slot: Uuid::nil(),
            field: CredentialFieldKind::Password,
        };
        assert_eq!(source.validate(FieldClassification::Password), Ok(()));
        let encoded = serde_json::to_value(&source).expect("encode");
        assert_eq!(
            encoded,
            serde_json::json!({
                "type": "credential_field",
                "slot": "00000000-0000-0000-0000-000000000000",
                "field": "password"
            })
        );
    }

    #[test]
    fn unknown_value_source_fields_are_rejected() {
        let err = serde_json::from_value::<ValueSource>(serde_json::json!({
            "type": "literal",
            "value": "monthly",
            "extra": true
        }));
        assert!(err.is_err());
    }
}
