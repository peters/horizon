use super::{CredentialMode, CredentialPolicy, RoutineDefinition, RoutineStep};
use crate::Origin;
use crate::assertion::Assertion;
use crate::compile::{CompiledAction, ResumePolicy};
use crate::recording::{MutationClass, NavigationTemplate, PathSegment};
use crate::value::ValueSource;
use crate::{RoutineError, SCHEMA_VERSION};
use horizon_browser_protocol::BackendKind;
use uuid::Uuid;

pub(crate) fn sample_definition() -> RoutineDefinition {
    let id = Uuid::nil();
    RoutineDefinition {
        schema_version: SCHEMA_VERSION,
        routine_id: id,
        name: "monthly-report".to_string(),
        backend_requirement: BackendKind::ChromiumCdp,
        profile_id: id,
        allowed_origins: vec![Origin::parse("https://reports.example").expect("origin")],
        credential_policy: CredentialPolicy {
            mode: CredentialMode::None,
            slot: None,
            allowed_origins: Vec::new(),
        },
        variables: Vec::new(),
        steps: vec![RoutineStep {
            step_id: "go".to_string(),
            target_fingerprint: None,
            action: CompiledAction::Navigate {
                navigation: NavigationTemplate {
                    origin: Origin::parse("https://reports.example").expect("origin"),
                    path: vec![PathSegment {
                        source: ValueSource::Literal {
                            value: "app".to_string(),
                        },
                    }],
                    query: Vec::new(),
                    fragment: None,
                },
            },
            value_source: None,
            mutation_class: MutationClass::ReadOnly,
            resume_policy: ResumePolicy::RetryIfIdempotent,
            precondition: None,
            postcondition: None,
        }],
        completion_assertions: vec![Assertion::Heading {
            value: "Report ready".to_string(),
        }],
        plan_version: 1,
        verified_plan_version: None,
        created_at: "2026-09-13T00:00:00Z".to_string(),
        updated_at: "2026-09-13T00:00:00Z".to_string(),
    }
}

#[test]
fn safari_is_not_a_routine_backend() {
    let mut routine = sample_definition();
    routine.backend_requirement = BackendKind::SafariWebDriver;
    assert_eq!(routine.validate(), Err(RoutineError::InvalidRecording));
}

#[test]
fn none_policy_cannot_carry_a_slot() {
    let mut routine = sample_definition();
    routine.credential_policy.slot = Some(Uuid::nil());
    assert_eq!(routine.validate(), Err(RoutineError::SlotMismatch));
}

#[test]
fn empty_completion_assertions_are_malformed() {
    let mut routine = sample_definition();
    routine.completion_assertions.clear();
    assert_eq!(routine.validate(), Err(RoutineError::InvalidAssertion));
}

#[test]
fn ready_requires_matching_verified_plan_version() {
    let mut routine = sample_definition();
    assert!(!routine.is_ready());
    routine.verified_plan_version = Some(1);
    assert!(routine.is_ready());
    routine.plan_version = 2;
    assert!(!routine.is_ready());
}
