use std::collections::HashSet;

use horizon_browser_protocol::BackendKind;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::RoutineError;
use crate::SCHEMA_VERSION;
use crate::assertion::Assertion;
use crate::compile::{CompiledAction, ResumePolicy};
use crate::fingerprint::TargetFingerprint;
use crate::origin::Origin;
use crate::recording::{MutationClass, validate_wait_selector};
use crate::value::{CredentialFieldKind, FieldClassification, ValueSource, validate_identifier};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const MAX_NAME_BYTES: usize = 128;
const MAX_STEPS: usize = 256;
const MAX_VARIABLES: usize = 32;

/// Named, versioned routine. Filesystem paths use `routine_id`, never `name`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineDefinition {
    pub schema_version: u32,
    pub routine_id: Uuid,
    pub name: String,
    pub backend_requirement: BackendKind,
    pub profile_id: Uuid,
    pub allowed_origins: Vec<Origin>,
    pub credential_policy: CredentialPolicy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<RoutineVariable>,
    pub steps: Vec<RoutineStep>,
    pub completion_assertions: Vec<Assertion>,
    pub plan_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_plan_version: Option<u32>,
    pub created_at: String,
    pub updated_at: String,
}

/// Persistence choice for one routine. Never contains secret bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialPolicy {
    pub mode: CredentialMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_origins: Vec<Origin>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMode {
    #[default]
    None,
    UsernameOnly,
    UsernameAndPassword,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineVariable {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineStep {
    pub step_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_fingerprint: Option<TargetFingerprint>,
    pub action: CompiledAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_source: Option<ValueSource>,
    pub mutation_class: MutationClass,
    pub resume_policy: ResumePolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precondition: Option<Assertion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postcondition: Option<Assertion>,
}

impl RoutineDefinition {
    /// `ready` when this exact `plan_version` has a successful verification run.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.verified_plan_version == Some(self.plan_version)
    }

    /// # Errors
    /// Unknown schema, Safari backend, empty assertions, credential-policy
    /// mismatch, or invalid nested values.
    pub fn validate(&self) -> Result<(), RoutineError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(RoutineError::UnsupportedSchema(self.schema_version));
        }
        if self.backend_requirement == BackendKind::SafariWebDriver || self.profile_id != self.routine_id {
            return Err(RoutineError::InvalidRecording);
        }
        if self.name.is_empty() || self.name.len() > MAX_NAME_BYTES || self.name.chars().any(char::is_control) {
            return Err(RoutineError::InvalidIdentifier);
        }
        if self.steps.is_empty() || self.steps.len() > MAX_STEPS || self.variables.len() > MAX_VARIABLES {
            return Err(RoutineError::InvalidRecording);
        }
        if self.plan_version == 0 {
            return Err(RoutineError::InvalidRecording);
        }
        parse_timestamp(&self.created_at)?;
        parse_timestamp(&self.updated_at)?;
        if self.completion_assertions.is_empty() {
            return Err(RoutineError::InvalidAssertion);
        }
        if let Some(verified) = self.verified_plan_version
            && verified > self.plan_version
        {
            return Err(RoutineError::InvalidRecording);
        }
        self.credential_policy.validate()?;
        let mut declared = HashSet::new();
        for variable in &self.variables {
            validate_identifier(&variable.name)?;
            if variable.name == "panel_id" {
                return Err(RoutineError::ReservedVariable);
            }
            if !declared.insert(variable.name.as_str()) {
                return Err(RoutineError::InvalidRecording);
            }
            if let Some(default) = &variable.default {
                ValueSource::Literal { value: default.clone() }.validate(FieldClassification::Ordinary)?;
            }
        }
        for assertion in &self.completion_assertions {
            assertion.validate()?;
            require_allowed_assertion(assertion, &self.allowed_origins)?;
        }
        let mut seen_steps = HashSet::new();
        for step in &self.steps {
            if !seen_steps.insert(step.step_id.as_str()) {
                return Err(RoutineError::InvalidRecording);
            }
            step.validate(&self.credential_policy, &self.allowed_origins, &declared)?;
        }
        Ok(())
    }
}

impl CredentialPolicy {
    fn validate(&self) -> Result<(), RoutineError> {
        match self.mode {
            CredentialMode::None => {
                if self.slot.is_some() || !self.allowed_origins.is_empty() {
                    return Err(RoutineError::SlotMismatch);
                }
                Ok(())
            }
            CredentialMode::UsernameOnly | CredentialMode::UsernameAndPassword => {
                if self.slot.is_none() || self.allowed_origins.is_empty() {
                    return Err(RoutineError::SlotMismatch);
                }
                Ok(())
            }
        }
    }

    pub(crate) fn permits(&self, slot: Uuid, field: CredentialFieldKind) -> Result<(), RoutineError> {
        if self.slot != Some(slot) {
            return Err(RoutineError::SlotMismatch);
        }
        match (self.mode, field) {
            (CredentialMode::UsernameOnly, CredentialFieldKind::Username)
            | (CredentialMode::UsernameAndPassword, CredentialFieldKind::Username | CredentialFieldKind::Password) => {
                Ok(())
            }
            _ => Err(RoutineError::SlotMismatch),
        }
    }
}

impl RoutineStep {
    fn validate(
        &self,
        policy: &CredentialPolicy,
        allowed_origins: &[Origin],
        declared: &HashSet<&str>,
    ) -> Result<(), RoutineError> {
        validate_identifier(&self.step_id)?;
        if let Some(target) = &self.target_fingerprint {
            require_allowed_target(target, allowed_origins)?;
        }
        if let Some(precondition) = &self.precondition {
            precondition.validate()?;
        }
        if let Some(postcondition) = &self.postcondition {
            postcondition.validate()?;
        }
        if resume_policy(self.mutation_class) != self.resume_policy {
            return Err(RoutineError::InvalidRecording);
        }
        match &self.action {
            CompiledAction::Navigate { navigation } => {
                reject_target(self.target_fingerprint.as_ref())?;
                reject_value(self.value_source.as_ref())?;
                navigation.validate()?;
                require_declared_variables(navigation, declared)?;
                if !origin_allowed(&navigation.origin, allowed_origins) {
                    return Err(RoutineError::OriginNotAllowed);
                }
                Ok(())
            }
            CompiledAction::Click { count } => {
                if !(1..=3).contains(count) {
                    return Err(RoutineError::InvalidRecording);
                }
                required_target(self.target_fingerprint.as_ref(), allowed_origins)?;
                reject_value(self.value_source.as_ref())
            }
            CompiledAction::Fill => {
                required_target(self.target_fingerprint.as_ref(), allowed_origins)?;
                let source = self.value_source.as_ref().ok_or(RoutineError::MissingValueSource)?;
                match source {
                    ValueSource::Literal { .. } | ValueSource::Variable { .. } => {
                        source.validate(FieldClassification::Ordinary)?;
                        require_declared_source(source, declared)
                    }
                    ValueSource::CredentialField { .. } => Err(RoutineError::CredentialFieldMismatch),
                }
            }
            CompiledAction::CredentialFill => {
                required_target(self.target_fingerprint.as_ref(), allowed_origins)?;
                if let Some(target) = &self.target_fingerprint
                    && !origin_allowed(&target.frame.origin, &policy.allowed_origins)
                {
                    return Err(RoutineError::OriginNotAllowed);
                }
                match self.value_source.as_ref().ok_or(RoutineError::MissingValueSource)? {
                    ValueSource::CredentialField { slot, field } => policy.permits(*slot, *field),
                    ValueSource::Literal { .. } | ValueSource::Variable { .. } => Err(RoutineError::SecretLiteral),
                }
            }
            CompiledAction::Scroll { delta_x, delta_y } => {
                if !delta_x.is_finite() || !delta_y.is_finite() {
                    return Err(RoutineError::InvalidRecording);
                }
                if let Some(target) = &self.target_fingerprint {
                    require_allowed_target(target, allowed_origins)?;
                }
                reject_value(self.value_source.as_ref())
            }
            CompiledAction::Wait { selector, .. } => {
                reject_target(self.target_fingerprint.as_ref())?;
                reject_value(self.value_source.as_ref())?;
                validate_wait_selector(selector)
            }
            CompiledAction::Reload
            | CompiledAction::Back
            | CompiledAction::Forward
            | CompiledAction::Handoff { .. } => {
                reject_target(self.target_fingerprint.as_ref())?;
                reject_value(self.value_source.as_ref())
            }
        }
    }
}

fn resume_policy(class: MutationClass) -> ResumePolicy {
    match class {
        MutationClass::ReadOnly | MutationClass::Idempotent => ResumePolicy::RetryIfIdempotent,
        MutationClass::Mutating | MutationClass::Consequential => ResumePolicy::NeverReplayIfUncertain,
    }
}

fn required_target(target: Option<&TargetFingerprint>, allowed_origins: &[Origin]) -> Result<(), RoutineError> {
    require_allowed_target(target.ok_or(RoutineError::UndurableTarget)?, allowed_origins)
}

fn require_allowed_target(target: &TargetFingerprint, allowed_origins: &[Origin]) -> Result<(), RoutineError> {
    target.validate()?;
    if !origin_allowed(&target.frame.origin, allowed_origins)
        || target
            .frame
            .chain
            .iter()
            .any(|frame| !origin_allowed(&frame.origin, allowed_origins))
    {
        return Err(RoutineError::OriginNotAllowed);
    }
    Ok(())
}

fn origin_allowed(origin: &Origin, allowed_origins: &[Origin]) -> bool {
    allowed_origins.iter().any(|allowed| allowed == origin)
}

fn require_allowed_assertion(assertion: &Assertion, allowed_origins: &[Origin]) -> Result<(), RoutineError> {
    match assertion {
        Assertion::ElementPresent { target } | Assertion::ElementAbsent { target } => {
            require_allowed_target(target, allowed_origins)
        }
        Assertion::UrlPattern { value } if !value.starts_with('/') => {
            let origin = Origin::parse(value).map_err(|_| RoutineError::InvalidAssertion)?;
            if origin_allowed(&origin, allowed_origins) {
                Ok(())
            } else {
                Err(RoutineError::OriginNotAllowed)
            }
        }
        Assertion::UrlPattern { .. } | Assertion::Heading { .. } | Assertion::TextShape { .. } => Ok(()),
    }
}

fn require_declared_variables(
    navigation: &crate::recording::NavigationTemplate,
    declared: &HashSet<&str>,
) -> Result<(), RoutineError> {
    for segment in &navigation.path {
        require_declared_source(&segment.source, declared)?;
    }
    for component in navigation.query.iter().chain(navigation.fragment.iter()) {
        require_declared_source(&component.source, declared)?;
    }
    Ok(())
}

fn require_declared_source(source: &ValueSource, declared: &HashSet<&str>) -> Result<(), RoutineError> {
    match source {
        ValueSource::Variable { name } if !declared.contains(name.as_str()) => Err(RoutineError::InvalidRecording),
        ValueSource::Literal { .. } | ValueSource::Variable { .. } | ValueSource::CredentialField { .. } => Ok(()),
    }
}

fn parse_timestamp(value: &str) -> Result<(), RoutineError> {
    if value.is_empty() || value.len() > 64 || value.chars().any(char::is_control) {
        return Err(RoutineError::InvalidRecording);
    }
    OffsetDateTime::parse(value, &Rfc3339)
        .map(|_| ())
        .map_err(|_| RoutineError::InvalidRecording)
}

fn reject_target(target: Option<&TargetFingerprint>) -> Result<(), RoutineError> {
    if target.is_some() {
        Err(RoutineError::InvalidRecording)
    } else {
        Ok(())
    }
}

fn reject_value(value_source: Option<&ValueSource>) -> Result<(), RoutineError> {
    if value_source.is_some() {
        Err(RoutineError::UnexpectedValueSource)
    } else {
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests;
