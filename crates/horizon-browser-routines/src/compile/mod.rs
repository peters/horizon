use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use url::Url;

use crate::RoutineError;
use crate::assertion::Assertion;
use crate::fingerprint::TargetFingerprint;
use crate::recording::{
    MutationClass, NavigationTemplate, PauseReason, RecordedAction, RecordedKind, SemanticRecording,
};
use crate::value::ValueSource;

const PANEL_VAR: &str = "panel_id";

/// How an interrupted step may resume. Mutating steps never replay blindly.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumePolicy {
    RetryIfIdempotent,
    NeverReplayIfUncertain,
}

/// Validated compiler output: routine steps plus MCP calls that exist today.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledRoutine {
    pub steps: Vec<CompiledStep>,
    pub completion_assertions: Vec<Assertion>,
}

/// One reviewed step. Click, fill, targeted scroll, credential fill, and
/// handoff omit `mcp` until the claimed engine fingerprint path lands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledStep {
    pub step_id: String,
    pub action: CompiledAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TargetFingerprint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_source: Option<ValueSource>,
    pub mutation_class: MutationClass,
    pub resume_policy: ResumePolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precondition: Option<Assertion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postcondition: Option<Assertion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<McpCall>,
}

/// Tagged compiled action. Click, fill, and targeted scroll carry identity on
/// [`CompiledStep::target`], not as a CSS selector.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompiledAction {
    Navigate {
        navigation: NavigationTemplate,
    },
    Click {
        count: u32,
    },
    Fill,
    CredentialFill,
    Scroll {
        delta_x: f64,
        delta_y: f64,
    },
    Wait {
        selector: String,
        state: horizon_browser_protocol::SelectorState,
    },
    Reload,
    Back,
    Forward,
    Handoff {
        pause: PauseReason,
    },
}

/// One existing `browser_*` tool call. Arguments never contain secret values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpCall {
    pub tool: String,
    pub arguments: Map<String, Value>,
}

/// Compile a validated recording into crate-local plan steps.
///
/// Consecutive scrolls that share a target are coalesced. Click, fill, targeted
/// scroll, credential fill, and handoff stay off the MCP selector path.
///
/// # Errors
/// Returns the recording's validation error, an assertion validation error, or
/// [`RoutineError::ReservedVariable`] when a variable is named `panel_id`.
pub fn compile(
    recording: &SemanticRecording,
    completion_assertions: Vec<Assertion>,
) -> Result<CompiledRoutine, RoutineError> {
    recording.validate()?;
    for assertion in &completion_assertions {
        assertion.validate()?;
    }
    let mut steps = Vec::new();
    let mut pending_scroll: Option<CompiledStep> = None;
    for action in &recording.actions {
        if let RecordedKind::Scroll { .. } = action.kind {
            let compiled = compile_action(action)?;
            pending_scroll = Some(match pending_scroll.take() {
                Some(previous) if same_scroll_target(&previous, &compiled) => coalesce_scroll(previous, compiled)?,
                Some(previous) => {
                    steps.push(previous);
                    compiled
                }
                None => compiled,
            });
            continue;
        }
        if let Some(scroll) = pending_scroll.take() {
            steps.push(scroll);
        }
        steps.push(compile_action(action)?);
    }
    if let Some(scroll) = pending_scroll {
        steps.push(scroll);
    }
    Ok(CompiledRoutine {
        steps,
        completion_assertions,
    })
}

fn compile_action(action: &RecordedAction) -> Result<CompiledStep, RoutineError> {
    reject_reserved_variables(action)?;
    let resume_policy = resume_policy(action.mutation_class);
    let compiled_action = match &action.kind {
        RecordedKind::Navigate => CompiledAction::Navigate {
            navigation: action.navigation.clone().ok_or(RoutineError::MissingNavigation)?,
        },
        RecordedKind::Click { count } => {
            action.target.as_ref().ok_or(RoutineError::UndurableTarget)?;
            CompiledAction::Click { count: *count }
        }
        RecordedKind::Fill => {
            action.target.as_ref().ok_or(RoutineError::UndurableTarget)?;
            match action.value_source.as_ref().ok_or(RoutineError::MissingValueSource)? {
                ValueSource::CredentialField { .. } => CompiledAction::CredentialFill,
                ValueSource::Literal { .. } | ValueSource::Variable { .. } => CompiledAction::Fill,
            }
        }
        RecordedKind::Scroll { delta_x, delta_y } => CompiledAction::Scroll {
            delta_x: *delta_x,
            delta_y: *delta_y,
        },
        RecordedKind::Wait { selector, state } => CompiledAction::Wait {
            selector: selector.clone(),
            state: *state,
        },
        RecordedKind::Reload => CompiledAction::Reload,
        RecordedKind::Back => CompiledAction::Back,
        RecordedKind::Forward => CompiledAction::Forward,
        RecordedKind::Handoff { pause } => CompiledAction::Handoff { pause: *pause },
    };
    let mcp = mcp_call(&compiled_action, action.target.as_ref())?;
    Ok(CompiledStep {
        step_id: action.action_id.clone(),
        action: compiled_action,
        target: action.target.clone(),
        value_source: action.value_source.clone(),
        mutation_class: action.mutation_class,
        resume_policy,
        precondition: action.precondition.clone(),
        postcondition: action.postcondition.clone(),
        mcp,
    })
}

fn same_scroll_target(previous: &CompiledStep, next: &CompiledStep) -> bool {
    matches!(previous.action, CompiledAction::Scroll { .. })
        && matches!(next.action, CompiledAction::Scroll { .. })
        && previous.target == next.target
}

fn coalesce_scroll(mut previous: CompiledStep, next: CompiledStep) -> Result<CompiledStep, RoutineError> {
    let CompiledAction::Scroll { delta_x, delta_y } = &mut previous.action else {
        return Err(RoutineError::InvalidRecording);
    };
    let CompiledAction::Scroll {
        delta_x: next_x,
        delta_y: next_y,
    } = next.action
    else {
        return Err(RoutineError::InvalidRecording);
    };
    *delta_x += next_x;
    *delta_y += next_y;
    previous.postcondition = next.postcondition;
    previous.mcp = mcp_call(&previous.action, previous.target.as_ref())?;
    Ok(previous)
}

fn resume_policy(class: MutationClass) -> ResumePolicy {
    match class {
        MutationClass::ReadOnly | MutationClass::Idempotent => ResumePolicy::RetryIfIdempotent,
        MutationClass::Mutating | MutationClass::Consequential => ResumePolicy::NeverReplayIfUncertain,
    }
}

fn reject_reserved_variables(action: &RecordedAction) -> Result<(), RoutineError> {
    if let Some(source) = &action.value_source {
        reject_reserved(source)?;
    }
    if let Some(navigation) = &action.navigation {
        for segment in &navigation.path {
            reject_reserved(&segment.source)?;
        }
        for component in navigation.query.iter().chain(navigation.fragment.iter()) {
            reject_reserved(&component.source)?;
        }
    }
    Ok(())
}

fn reject_reserved(source: &ValueSource) -> Result<(), RoutineError> {
    match source {
        ValueSource::Variable { name } if name == PANEL_VAR => Err(RoutineError::ReservedVariable),
        ValueSource::Literal { .. } | ValueSource::Variable { .. } | ValueSource::CredentialField { .. } => Ok(()),
    }
}

fn panel_ref() -> Value {
    json!({ "$var": PANEL_VAR })
}

fn mcp_call(action: &CompiledAction, target: Option<&TargetFingerprint>) -> Result<Option<McpCall>, RoutineError> {
    match action {
        CompiledAction::Navigate { navigation } => mcp_navigate(navigation),
        CompiledAction::Click { .. }
        | CompiledAction::Fill
        | CompiledAction::CredentialFill
        | CompiledAction::Handoff { .. } => Ok(None),
        CompiledAction::Scroll { delta_x, delta_y } => {
            if target.is_some() {
                return Ok(None);
            }
            let mut arguments = panel_arguments();
            arguments.insert("action".to_string(), json!("scroll"));
            arguments.insert("delta_x".to_string(), json!(delta_x));
            arguments.insert("delta_y".to_string(), json!(delta_y));
            Ok(Some(McpCall {
                tool: "browser_act".to_string(),
                arguments,
            }))
        }
        CompiledAction::Wait { selector, state } => {
            let mut arguments = panel_arguments();
            arguments.insert("selector".to_string(), Value::String(selector.clone()));
            arguments.insert(
                "state".to_string(),
                serde_json::to_value(state).map_err(|_| RoutineError::Json("malformed routine JSON".to_string()))?,
            );
            Ok(Some(McpCall {
                tool: "browser_wait".to_string(),
                arguments,
            }))
        }
        CompiledAction::Reload => Ok(Some(mcp_history("reload"))),
        CompiledAction::Back => Ok(Some(mcp_history("back"))),
        CompiledAction::Forward => Ok(Some(mcp_history("forward"))),
    }
}

fn mcp_navigate(navigation: &NavigationTemplate) -> Result<Option<McpCall>, RoutineError> {
    let Some(url) = literal_navigation_url(navigation)? else {
        return Ok(None);
    };
    let mut arguments = panel_arguments();
    arguments.insert("url".to_string(), Value::String(url));
    Ok(Some(McpCall {
        tool: "browser_navigate".to_string(),
        arguments,
    }))
}

fn mcp_history(action: &str) -> McpCall {
    let mut arguments = panel_arguments();
    arguments.insert("action".to_string(), json!(action));
    McpCall {
        tool: "browser_act".to_string(),
        arguments,
    }
}

fn panel_arguments() -> Map<String, Value> {
    let mut arguments = Map::new();
    arguments.insert("panel_id".to_string(), panel_ref());
    arguments
}

fn literal_navigation_url(template: &NavigationTemplate) -> Result<Option<String>, RoutineError> {
    if template
        .path
        .iter()
        .any(|segment| !matches!(segment.source, ValueSource::Literal { .. }))
        || template
            .query
            .iter()
            .chain(template.fragment.iter())
            .any(|component| !matches!(component.source, ValueSource::Literal { .. }))
    {
        return Ok(None);
    }
    let mut url = Url::parse(template.origin.as_str()).map_err(|_| RoutineError::InvalidNavigation)?;
    {
        let mut segments = url.path_segments_mut().map_err(|()| RoutineError::InvalidNavigation)?;
        segments.clear();
        for segment in &template.path {
            if let ValueSource::Literal { value } = &segment.source {
                segments.push(value);
            }
        }
    }
    if !template.query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for component in &template.query {
            if let ValueSource::Literal { value } = &component.source {
                pairs.append_pair(&component.name, value);
            }
        }
    }
    if let Some(fragment) = &template.fragment
        && let ValueSource::Literal { value } = &fragment.source
    {
        url.set_fragment(Some(value));
    }
    Ok(Some(url.into()))
}

#[cfg(test)]
mod tests;
