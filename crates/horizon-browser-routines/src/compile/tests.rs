use super::{CompiledAction, ResumePolicy, compile};
use crate::assertion::Assertion;
use crate::fingerprint::{FrameContext, RankedCandidate, TargetCandidate, TargetFingerprint};
use crate::origin::Origin;
use crate::recording::{
    MutationClass, NavigationTemplate, PathSegment, PauseReason, QueryComponent, RecordedAction, RecordedKind,
    SemanticRecording,
};
use crate::value::{CredentialFieldKind, FieldClassification, ValueSource};
use crate::{RoutineError, SCHEMA_VERSION};
use horizon_browser_protocol::SelectorState;
use uuid::Uuid;

fn origin() -> Origin {
    Origin::parse("https://reports.example").expect("origin")
}

fn ranked(identity: TargetCandidate, match_count: u32) -> RankedCandidate {
    RankedCandidate {
        identity,
        match_count,
        unique: match_count == 1,
    }
}

fn fingerprint(identity: TargetCandidate, match_count: u32) -> TargetFingerprint {
    TargetFingerprint {
        candidates: vec![ranked(identity, match_count)],
        selected: Some(0),
        frame: FrameContext {
            top_level: true,
            origin: origin(),
            chain: Vec::new(),
        },
        digest: "el-1".to_string(),
    }
}

fn unique_id(value: &str, match_count: u32) -> TargetFingerprint {
    fingerprint(
        TargetCandidate::UniqueId {
            value: value.to_string(),
            reviewed: false,
        },
        match_count,
    )
}

fn role_name(name: &str, match_count: u32) -> TargetFingerprint {
    fingerprint(
        TargetCandidate::RoleName {
            role: "button".to_string(),
            name: name.to_string(),
            reviewed: false,
        },
        match_count,
    )
}

fn action(id: &str, kind: RecordedKind, target: Option<TargetFingerprint>) -> RecordedAction {
    RecordedAction {
        action_id: id.to_string(),
        recorded_at_millis: 1,
        kind,
        target,
        page_origin: origin(),
        url_pattern: "https://reports.example/app".to_string(),
        navigation: None,
        value_source: None,
        field_classification: FieldClassification::Ordinary,
        mutation_class: MutationClass::ReadOnly,
        precondition: None,
        postcondition: None,
    }
}

fn recording(actions: Vec<RecordedAction>) -> SemanticRecording {
    SemanticRecording {
        schema_version: SCHEMA_VERSION,
        recording_id: Uuid::nil(),
        actions,
    }
}

fn heading() -> Vec<Assertion> {
    vec![Assertion::Heading {
        value: "Report ready".to_string(),
    }]
}

fn app_navigation() -> NavigationTemplate {
    NavigationTemplate {
        origin: origin(),
        path: vec![PathSegment {
            source: ValueSource::Literal {
                value: "app".to_string(),
            },
        }],
        query: Vec::new(),
        fragment: None,
    }
}

#[test]
fn multi_page_flow_compiles_without_selector_mcp_clicks() {
    let mut navigate = action("go", RecordedKind::Navigate, None);
    navigate.navigation = Some(app_navigation());
    let click = action(
        "run",
        RecordedKind::Click { count: 1 },
        Some(role_name("Generate report", 1)),
    );
    let wait = action(
        "ready",
        RecordedKind::Wait {
            selector: "#status".to_string(),
            state: SelectorState::Visible,
        },
        None,
    );
    let compiled = compile(&recording(vec![navigate, click, wait]), heading()).expect("compile");
    assert_eq!(compiled.steps.len(), 3);
    assert_eq!(
        compiled.steps[0].mcp.as_ref().map(|call| call.tool.as_str()),
        Some("browser_navigate")
    );
    assert_eq!(
        compiled.steps[0]
            .mcp
            .as_ref()
            .and_then(|call| call.arguments.get("url")),
        Some(&serde_json::json!("https://reports.example/app"))
    );
    assert!(matches!(compiled.steps[1].action, CompiledAction::Click { count: 1 }));
    assert_eq!(compiled.steps[1].mcp, None);
    assert!(
        !serde_json::to_string(&compiled.steps[1])
            .expect("encode")
            .contains("selector")
    );
    assert_eq!(
        compiled.steps[2].mcp.as_ref().map(|call| call.tool.as_str()),
        Some("browser_wait")
    );
    assert_eq!(compiled.steps[1].resume_policy, ResumePolicy::RetryIfIdempotent);
    assert_eq!(compiled.completion_assertions.len(), 1);
}

#[test]
fn duplicate_text_keeps_uniqueness_evidence_and_skips_mcp() {
    let click = action(
        "dup",
        RecordedKind::Click { count: 1 },
        Some(fingerprint(
            TargetCandidate::TestId {
                attribute: "data-testid".to_string(),
                value: "row-save".to_string(),
                reviewed: false,
            },
            2,
        )),
    );
    let compiled = compile(&recording(vec![click]), heading()).expect("compile");
    assert!(!compiled.steps[0].target.as_ref().expect("target").candidates[0].unique);
    assert_eq!(compiled.steps[0].mcp, None);
}

#[test]
fn password_fill_stays_a_placeholder_without_mcp_fill() {
    let mut fill = action("password", RecordedKind::Fill, Some(unique_id("password", 1)));
    fill.field_classification = FieldClassification::Password;
    fill.mutation_class = MutationClass::Mutating;
    fill.value_source = Some(ValueSource::CredentialField {
        slot: Uuid::nil(),
        field: CredentialFieldKind::Password,
    });
    let compiled = compile(&recording(vec![fill]), heading()).expect("compile");
    assert!(matches!(compiled.steps[0].action, CompiledAction::CredentialFill));
    assert_eq!(compiled.steps[0].mcp, None);
    assert_eq!(compiled.steps[0].resume_policy, ResumePolicy::NeverReplayIfUncertain);
    let encoded = serde_json::to_string(&compiled).expect("encode");
    assert!(!encoded.contains("\"tool\":\"browser_act\""));
    assert!(encoded.contains("credential_field"));
    assert!(!encoded.to_ascii_lowercase().contains("secret"));
}

#[test]
fn scroll_bursts_are_coalesced() {
    let first = action(
        "s1",
        RecordedKind::Scroll {
            delta_x: 0.0,
            delta_y: 80.0,
        },
        None,
    );
    let second = action(
        "s2",
        RecordedKind::Scroll {
            delta_x: 0.0,
            delta_y: 40.0,
        },
        None,
    );
    let compiled = compile(&recording(vec![first, second]), heading()).expect("compile");
    assert_eq!(compiled.steps.len(), 1);
    assert!(matches!(
        compiled.steps[0].action,
        CompiledAction::Scroll { delta_y: 120.0, .. }
    ));
    assert_eq!(
        compiled.steps[0]
            .mcp
            .as_ref()
            .and_then(|call| call.arguments.get("delta_y")),
        Some(&serde_json::json!(120.0))
    );
}

#[test]
fn targeted_scroll_does_not_emit_mcp() {
    let scroll = action(
        "into",
        RecordedKind::Scroll {
            delta_x: 0.0,
            delta_y: 40.0,
        },
        Some(unique_id("table", 1)),
    );
    let compiled = compile(&recording(vec![scroll]), heading()).expect("compile");
    assert_eq!(compiled.steps[0].mcp, None);
}

#[test]
fn variable_navigation_keeps_the_template_and_skips_mcp() {
    let mut navigate = action("go", RecordedKind::Navigate, None);
    navigate.navigation = Some(NavigationTemplate {
        origin: origin(),
        path: vec![PathSegment {
            source: ValueSource::Variable {
                name: "report_month".to_string(),
            },
        }],
        query: vec![QueryComponent {
            name: "tab".to_string(),
            source: ValueSource::Literal {
                value: "summary".to_string(),
            },
        }],
        fragment: None,
    });
    let compiled = compile(&recording(vec![navigate]), heading()).expect("compile");
    assert!(matches!(compiled.steps[0].action, CompiledAction::Navigate { .. }));
    assert_eq!(compiled.steps[0].mcp, None);
}

#[test]
fn variable_named_panel_id_is_reserved() {
    let mut fill = action("month", RecordedKind::Fill, Some(unique_id("month", 1)));
    fill.value_source = Some(ValueSource::Variable {
        name: "panel_id".to_string(),
    });
    assert_eq!(
        compile(&recording(vec![fill]), heading()),
        Err(RoutineError::ReservedVariable)
    );
}

#[test]
fn login_handoff_is_not_browser_handoff_mcp() {
    let handoff = action(
        "login",
        RecordedKind::Handoff {
            pause: PauseReason::NeedsLogin,
        },
        None,
    );
    let compiled = compile(&recording(vec![handoff]), heading()).expect("compile");
    assert!(matches!(
        compiled.steps[0].action,
        CompiledAction::Handoff {
            pause: PauseReason::NeedsLogin
        }
    ));
    assert_eq!(compiled.steps[0].mcp, None);
}

#[test]
fn css_fallback_alone_is_not_compiled_unless_reviewed() {
    let click = action(
        "weak",
        RecordedKind::Click { count: 1 },
        Some(fingerprint(
            TargetCandidate::CssFallback {
                value: "div > span:nth-child(3)".to_string(),
                reviewed: false,
            },
            1,
        )),
    );
    assert_eq!(
        compile(&recording(vec![click]), heading()),
        Err(RoutineError::UndurableTarget)
    );
}
