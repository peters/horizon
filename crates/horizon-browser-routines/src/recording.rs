use horizon_browser_protocol::{SelectorState, redact_url};
use serde::ser::{SerializeStruct, Serializer};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::assertion::Assertion;
use crate::fingerprint::TargetFingerprint;
use crate::origin::Origin;
use crate::value::{FieldClassification, ValueSource, validate_identifier};
use crate::{RoutineError, SCHEMA_VERSION};

const MAX_ACTIONS: usize = 256;
const MAX_WAIT_SELECTOR_BYTES: usize = 16 * 1024;
const MAX_URL_PATTERN_BYTES: usize = 8 * 1024;

/// Versioned list of privacy-filtered semantic actions from one Teach session.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticRecording {
    pub schema_version: u32,
    pub recording_id: Uuid,
    pub actions: Vec<RecordedAction>,
}

/// One grouped user interaction, never a raw pointer-move sample.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedAction {
    pub action_id: String,
    pub recorded_at_millis: i64,
    pub kind: RecordedKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TargetFingerprint>,
    pub page_origin: Origin,
    pub url_pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navigation: Option<NavigationTemplate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_source: Option<ValueSource>,
    #[serde(default)]
    pub field_classification: FieldClassification,
    pub mutation_class: MutationClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precondition: Option<Assertion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postcondition: Option<Assertion>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecordedKind {
    Navigate,
    Click { count: u32 },
    Fill,
    Scroll { delta_x: f64, delta_y: f64 },
    Wait { selector: String, state: SelectorState },
    Reload,
    Back,
    Forward,
    Handoff { pause: PauseReason },
}

/// Durable pause a handoff step asks the runner to enter.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    NeedsLogin,
    NeedsUser,
    NeedsReteach,
}

/// Replayable navigation destination. Distinct from diagnostic `url_pattern`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavigationTemplate {
    pub origin: Origin,
    pub path: Vec<PathSegment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query: Vec<QueryComponent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fragment: Option<QueryComponent>,
}

/// One path segment, either a literal or a non-secret variable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathSegment {
    pub source: ValueSource,
}

/// Non-secret query or fragment component supplied as a literal or variable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryComponent {
    pub name: String,
    pub source: ValueSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationClass {
    ReadOnly,
    Idempotent,
    Mutating,
    Consequential,
}

impl NavigationTemplate {
    pub(crate) fn validate(&self) -> Result<(), RoutineError> {
        for segment in &self.path {
            match &segment.source {
                ValueSource::Literal { value } => {
                    if value.is_empty()
                        || value.contains('/')
                        || value.contains('?')
                        || value.contains('#')
                        || value.chars().any(char::is_control)
                    {
                        return Err(RoutineError::InvalidNavigation);
                    }
                    segment.source.validate(FieldClassification::Ordinary)?;
                }
                ValueSource::Variable { .. } => segment.source.validate(FieldClassification::Ordinary)?,
                ValueSource::CredentialField { .. } => return Err(RoutineError::InvalidNavigation),
            }
        }
        for component in self.query.iter().chain(self.fragment.iter()) {
            component.validate()?;
        }
        Ok(())
    }

    /// Origin plus literal path segments. Variable segments are omitted.
    #[must_use]
    pub fn origin_and_path(&self) -> String {
        let mut url = self.origin.as_str().to_string();
        for segment in &self.path {
            if let ValueSource::Literal { value } = &segment.source {
                url.push('/');
                url.push_str(value);
            }
        }
        url
    }
}

impl QueryComponent {
    fn validate(&self) -> Result<(), RoutineError> {
        validate_identifier(&self.name)?;
        match &self.source {
            ValueSource::Literal { .. } | ValueSource::Variable { .. } => {
                self.source.validate(FieldClassification::Ordinary)
            }
            ValueSource::CredentialField { .. } => Err(RoutineError::InvalidNavigation),
        }
    }
}

impl Serialize for SemanticRecording {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("SemanticRecording", 3)?;
        state.serialize_field("schema_version", &self.schema_version)?;
        state.serialize_field("recording_id", &self.recording_id)?;
        state.serialize_field("actions", &self.actions)?;
        state.end()
    }
}

impl SemanticRecording {
    /// Check schema version, action bounds, and redaction invariants.
    ///
    /// # Errors
    /// Returns a typed [`RoutineError`] for unknown versions, empty or oversized
    /// recordings, and any action that would persist a secret or undurable target.
    pub fn validate(&self) -> Result<(), RoutineError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(RoutineError::UnsupportedSchema(self.schema_version));
        }
        if self.actions.is_empty() || self.actions.len() > MAX_ACTIONS {
            return Err(RoutineError::InvalidRecording);
        }
        for action in &self.actions {
            action.validate()?;
        }
        Ok(())
    }

    /// Serialize only after [`Self::validate`] succeeds so typed secrets cannot
    /// enter JSON.
    ///
    /// # Errors
    /// Returns the validation error, or [`RoutineError::Json`] if encoding fails.
    pub fn to_redacted_json(&self) -> Result<String, RoutineError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|_| RoutineError::Json("malformed routine JSON".to_string()))
    }

    /// Decode and validate a recording document.
    ///
    /// # Errors
    /// Returns [`RoutineError::Json`] for malformed JSON (including unknown
    /// fields) or the validation error for a well-typed but illegal document.
    pub fn from_json(bytes: &str) -> Result<Self, RoutineError> {
        let recording: Self =
            serde_json::from_str(bytes).map_err(|_| RoutineError::Json("malformed routine JSON".to_string()))?;
        recording.validate()?;
        Ok(recording)
    }
}

impl RecordedAction {
    pub(crate) fn validate(&self) -> Result<(), RoutineError> {
        validate_identifier(&self.action_id)?;
        if self.recorded_at_millis < 0 {
            return Err(RoutineError::InvalidRecording);
        }
        if self.url_pattern.len() > MAX_URL_PATTERN_BYTES
            || self.url_pattern.is_empty()
            || self.url_pattern.contains('@')
            || self.url_pattern != redact_url(&self.url_pattern)
        {
            return Err(RoutineError::UnredactedUrl);
        }
        if let Some(navigation) = &self.navigation {
            navigation.validate()?;
        }
        if let Some(precondition) = &self.precondition {
            precondition.validate()?;
        }
        if let Some(postcondition) = &self.postcondition {
            postcondition.validate()?;
        }
        match &self.kind {
            RecordedKind::Click { count } => {
                if !(1..=3).contains(count) {
                    return Err(RoutineError::InvalidRecording);
                }
                reject_navigation(self.navigation.as_ref())?;
                required_target(self.target.as_ref())?;
                reject_value(self.value_source.as_ref())
            }
            RecordedKind::Fill => {
                reject_navigation(self.navigation.as_ref())?;
                required_target(self.target.as_ref())?;
                let source = self.value_source.as_ref().ok_or(RoutineError::MissingValueSource)?;
                source.validate(self.field_classification)?;
                if self.field_classification == FieldClassification::Password
                    && !matches!(source, ValueSource::CredentialField { .. })
                {
                    return Err(RoutineError::SecretLiteral);
                }
                Ok(())
            }
            RecordedKind::Scroll { delta_x, delta_y } => {
                if !delta_x.is_finite() || !delta_y.is_finite() {
                    return Err(RoutineError::InvalidRecording);
                }
                reject_navigation(self.navigation.as_ref())?;
                if let Some(target) = &self.target {
                    target.validate()?;
                }
                reject_value(self.value_source.as_ref())
            }
            RecordedKind::Wait { selector, .. } => {
                reject_navigation(self.navigation.as_ref())?;
                validate_wait_selector(selector)?;
                reject_value(self.value_source.as_ref())
            }
            RecordedKind::Navigate => {
                if self.target.is_some() || self.value_source.is_some() {
                    return Err(RoutineError::InvalidRecording);
                }
                self.navigation
                    .as_ref()
                    .ok_or(RoutineError::MissingNavigation)?
                    .validate()
            }
            RecordedKind::Reload | RecordedKind::Back | RecordedKind::Forward | RecordedKind::Handoff { .. } => {
                if self.target.is_some() || self.navigation.is_some() {
                    return Err(RoutineError::InvalidRecording);
                }
                reject_value(self.value_source.as_ref())
            }
        }
    }
}

fn required_target(target: Option<&TargetFingerprint>) -> Result<(), RoutineError> {
    let target = target.ok_or(RoutineError::UndurableTarget)?;
    target.validate()
}

fn reject_navigation(navigation: Option<&NavigationTemplate>) -> Result<(), RoutineError> {
    if navigation.is_some() {
        Err(RoutineError::InvalidRecording)
    } else {
        Ok(())
    }
}

pub(crate) fn validate_wait_selector(selector: &str) -> Result<(), RoutineError> {
    let selector = selector.trim();
    if selector.is_empty()
        || selector.len() > MAX_WAIT_SELECTOR_BYTES
        || selector.chars().any(char::is_control)
        || selector.contains('?')
    {
        return Err(RoutineError::InvalidRecording);
    }
    Ok(())
}

fn reject_value(value_source: Option<&ValueSource>) -> Result<(), RoutineError> {
    if value_source.is_some() {
        Err(RoutineError::UnexpectedValueSource)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{MutationClass, NavigationTemplate, PathSegment, RecordedAction, RecordedKind, SemanticRecording};
    use crate::assertion::Assertion;
    use crate::fingerprint::{FrameContext, RankedCandidate, TargetCandidate, TargetFingerprint};
    use crate::origin::Origin;
    use crate::value::{CredentialFieldKind, FieldClassification, ValueSource};
    use crate::{RoutineError, SCHEMA_VERSION};
    use uuid::Uuid;

    fn origin() -> Origin {
        Origin::parse("https://reports.example").expect("origin")
    }

    fn target() -> TargetFingerprint {
        TargetFingerprint {
            candidates: vec![RankedCandidate {
                identity: TargetCandidate::RoleName {
                    role: "button".to_string(),
                    name: "Generate report".to_string(),
                    reviewed: false,
                },
                match_count: 1,
                unique: true,
            }],
            selected: Some(0),
            frame: FrameContext {
                top_level: true,
                origin: origin(),
                chain: Vec::new(),
            },
            digest: "el-1".to_string(),
        }
    }

    fn click() -> RecordedAction {
        RecordedAction {
            action_id: "click-1".to_string(),
            recorded_at_millis: 1,
            kind: RecordedKind::Click { count: 1 },
            target: Some(target()),
            page_origin: origin(),
            url_pattern: "https://reports.example/app".to_string(),
            navigation: None,
            value_source: None,
            field_classification: FieldClassification::Ordinary,
            mutation_class: MutationClass::ReadOnly,
            precondition: None,
            postcondition: Some(Assertion::Heading {
                value: "Report ready".to_string(),
            }),
        }
    }

    fn recording(actions: Vec<RecordedAction>) -> SemanticRecording {
        SemanticRecording {
            schema_version: SCHEMA_VERSION,
            recording_id: Uuid::nil(),
            actions,
        }
    }

    #[test]
    fn roundtrip_preserves_a_multi_page_flow() {
        let navigate = RecordedAction {
            action_id: "go".to_string(),
            recorded_at_millis: 0,
            kind: RecordedKind::Navigate,
            target: None,
            page_origin: origin(),
            url_pattern: "https://reports.example/app".to_string(),
            navigation: Some(NavigationTemplate {
                origin: origin(),
                path: vec![PathSegment {
                    source: ValueSource::Literal {
                        value: "app".to_string(),
                    },
                }],
                query: Vec::new(),
                fragment: None,
            }),
            value_source: None,
            field_classification: FieldClassification::Ordinary,
            mutation_class: MutationClass::ReadOnly,
            precondition: None,
            postcondition: None,
        };
        let document = recording(vec![navigate, click()]);
        let encoded = document.to_redacted_json().expect("encode");
        let decoded = SemanticRecording::from_json(&encoded).expect("decode");
        assert_eq!(decoded, document);
    }

    #[test]
    fn password_fill_persists_only_a_credential_placeholder() {
        let fill = RecordedAction {
            action_id: "fill-1".to_string(),
            recorded_at_millis: 2,
            kind: RecordedKind::Fill,
            target: Some(target()),
            page_origin: origin(),
            url_pattern: "https://reports.example/login".to_string(),
            navigation: None,
            value_source: Some(ValueSource::CredentialField {
                slot: Uuid::nil(),
                field: CredentialFieldKind::Password,
            }),
            field_classification: FieldClassification::Password,
            mutation_class: MutationClass::Mutating,
            precondition: None,
            postcondition: None,
        };
        let encoded = recording(vec![fill]).to_redacted_json().expect("encode");
        assert!(!encoded.contains("literal"), "{encoded}");
        assert!(encoded.contains("credential_field"), "{encoded}");
        assert!(!encoded.to_ascii_lowercase().contains("secret"));
    }

    #[test]
    fn password_fill_with_a_literal_refuses_to_serialize() {
        let fill = RecordedAction {
            action_id: "fill-bad".to_string(),
            recorded_at_millis: 2,
            kind: RecordedKind::Fill,
            target: Some(target()),
            page_origin: origin(),
            url_pattern: "https://reports.example/login".to_string(),
            navigation: None,
            value_source: Some(ValueSource::Literal {
                value: "typed".to_string(),
            }),
            field_classification: FieldClassification::Password,
            mutation_class: MutationClass::Mutating,
            precondition: None,
            postcondition: None,
        };
        assert_eq!(
            recording(vec![fill.clone()]).to_redacted_json(),
            Err(RoutineError::SecretLiteral)
        );
        assert!(serde_json::to_string(&recording(vec![fill])).is_err());
    }

    #[test]
    fn navigate_without_a_template_is_rejected() {
        let navigate = RecordedAction {
            action_id: "go".to_string(),
            recorded_at_millis: 0,
            kind: RecordedKind::Navigate,
            target: None,
            page_origin: origin(),
            url_pattern: "https://reports.example/app?<redacted>".to_string(),
            navigation: None,
            value_source: None,
            field_classification: FieldClassification::Ordinary,
            mutation_class: MutationClass::ReadOnly,
            precondition: None,
            postcondition: None,
        };
        assert_eq!(navigate.validate(), Err(RoutineError::MissingNavigation));
        assert_eq!(
            navigate.url_pattern,
            horizon_browser_protocol::redact_url("https://reports.example/app?session=1")
        );
    }

    #[test]
    fn click_without_a_fingerprint_is_undurable() {
        let mut action = click();
        action.target = None;
        assert_eq!(action.validate(), Err(RoutineError::UndurableTarget));
    }

    #[test]
    fn query_string_url_patterns_are_rejected() {
        let mut action = click();
        action.url_pattern = "https://reports.example/app?session=1".to_string();
        assert_eq!(action.validate(), Err(RoutineError::UnredactedUrl));
    }

    #[test]
    fn unknown_schema_and_unknown_fields_are_rejected() {
        let version_two = SemanticRecording {
            schema_version: 2,
            recording_id: Uuid::nil(),
            actions: vec![click()],
        };
        assert_eq!(version_two.validate(), Err(RoutineError::UnsupportedSchema(2)));
        assert!(
            serde_json::from_str::<SemanticRecording>(
                r#"{"schema_version":1,"recording_id":"00000000-0000-0000-0000-000000000000","actions":[],"extra":true}"#
            )
            .is_err()
        );
        assert_eq!(
            SemanticRecording::from_json(
                r#"{"schema_version":2,"recording_id":"00000000-0000-0000-0000-000000000000","actions":[]}"#
            ),
            Err(RoutineError::UnsupportedSchema(2))
        );
    }
}
