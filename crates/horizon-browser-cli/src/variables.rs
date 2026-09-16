//! Bounded plan literals substituted with `{"$var":"name"}`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::{Plan, PlanError, PlanStep, valid_identifier};

const REDACTED_SECRET: &str = "<redacted>";
const HTTP_AUTH_SECRET_FIELDS: [&str; 2] = ["password", "username"];

pub(crate) const MAX_VARIABLES: usize = 32;
const MAX_VARIABLE_BYTES: usize = 4 * 1024;
const MAX_VARIABLES_TOTAL_BYTES: usize = MAX_VARIABLES * MAX_VARIABLE_BYTES;

pub(crate) fn validate(variables: &BTreeMap<String, Value>) -> Result<(), PlanError> {
    if variables.len() > MAX_VARIABLES {
        return Err(PlanError::TooManyVariables {
            actual: variables.len(),
            maximum: MAX_VARIABLES,
        });
    }
    let mut total = 0_usize;
    for (name, value) in variables {
        if !valid_identifier(name) {
            return Err(PlanError::InvalidVariableName(name.clone()));
        }
        if value_contains_substitution(value) {
            return Err(PlanError::InvalidVariableValue { name: name.clone() });
        }
        let encoded = serde_json::to_vec(value).map_err(|error| PlanError::InvalidVariableName(error.to_string()))?;
        if encoded.len() > MAX_VARIABLE_BYTES {
            return Err(PlanError::VariableTooLarge {
                name: name.clone(),
                actual: encoded.len(),
                maximum: MAX_VARIABLE_BYTES,
            });
        }
        total = total.saturating_add(encoded.len());
        if total > MAX_VARIABLES_TOTAL_BYTES {
            return Err(PlanError::VariablesTooLarge {
                actual: total,
                maximum: MAX_VARIABLES_TOTAL_BYTES,
            });
        }
    }
    Ok(())
}

pub(crate) fn lookup(variables: &BTreeMap<String, Value>, step: &PlanStep, name: &str) -> Result<Value, PlanError> {
    variables.get(name).cloned().ok_or_else(|| PlanError::UnknownVariable {
        step: step.id.clone(),
        name: name.to_string(),
    })
}

/// Copy of a plan with HTTP auth username/password values replaced so durable
/// job state does not keep those secrets. Other steps keep their variables.
pub(crate) fn redact_plan(plan: &Plan) -> Plan {
    let mut plan = plan.clone();
    let secret_variables = http_auth_secret_variables(&plan);
    for name in &secret_variables {
        if let Some(value) = plan.variables.get_mut(name) {
            *value = Value::String(REDACTED_SECRET.to_string());
        }
    }
    for step in &mut plan.steps {
        if step.tool != "browser_http_auth" {
            continue;
        }
        for field in HTTP_AUTH_SECRET_FIELDS {
            if step.arguments.get(field).is_some_and(|value| !value.is_null()) {
                step.arguments
                    .insert(field.to_string(), Value::String(REDACTED_SECRET.to_string()));
            }
        }
    }
    plan
}

fn http_auth_secret_variables(plan: &Plan) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for step in &plan.steps {
        if step.tool != "browser_http_auth" {
            continue;
        }
        for field in HTTP_AUTH_SECRET_FIELDS {
            if let Some(name) = variable_name(step.arguments.get(field)) {
                names.insert(name.to_string());
            }
        }
    }
    names
}

fn variable_name(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_object)
        .and_then(|object| object.get("$var"))
        .and_then(Value::as_str)
}

/// True when remaining work includes `browser_http_auth` set without a usable
/// password, so resume must not replay a redacted secret.
#[must_use]
pub(crate) fn resume_blocked_by_http_auth_secrets(plan: &Plan, start_index: usize) -> bool {
    plan.steps.get(start_index..).is_some_and(|steps| {
        steps.iter().any(|step| {
            step.tool == "browser_http_auth"
                && step.arguments.get("operation").and_then(Value::as_str) == Some("set")
                && password_missing_or_redacted(&step.arguments)
        })
    })
}

fn password_missing_or_redacted(arguments: &Map<String, Value>) -> bool {
    match arguments.get("password") {
        None | Some(Value::Null) => true,
        Some(Value::String(value)) => value.is_empty() || value == REDACTED_SECRET,
        Some(_) => false,
    }
}

fn value_contains_substitution(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(value_contains_substitution),
        Value::Object(object) => {
            object.contains_key("$ref")
                || object.contains_key("$var")
                || object.values().any(value_contains_substitution)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_too_many_or_nested_substitutions() {
        let mut variables = BTreeMap::new();
        for index in 0..=MAX_VARIABLES {
            variables.insert(format!("v{index}"), json!(index));
        }
        assert!(matches!(
            validate(&variables),
            Err(PlanError::TooManyVariables { actual, .. }) if actual == MAX_VARIABLES + 1
        ));
        let nested = BTreeMap::from([("url".to_string(), json!({"$var":"other"}))]);
        assert!(matches!(validate(&nested), Err(PlanError::InvalidVariableValue { name }) if name == "url"));
    }

    #[test]
    fn redacts_http_auth_secrets_and_blocks_resume_of_remaining_set() {
        use crate::PlanStep;

        let plan = Plan {
            version: 1,
            variables: BTreeMap::from([
                ("username".to_string(), json!("smoke-user")),
                ("basic_secret".to_string(), json!("smoke-pass-zephyr")),
                ("fill_user".to_string(), json!("visible-user")),
                ("url".to_string(), json!("http://127.0.0.1:8080/basic-auth")),
            ]),
            steps: vec![
                PlanStep {
                    id: "panels".into(),
                    tool: "browser_list".into(),
                    arguments: serde_json::Map::new(),
                },
                PlanStep {
                    id: "credentials".into(),
                    tool: "browser_http_auth".into(),
                    arguments: serde_json::Map::from_iter([
                        ("operation".to_string(), json!("set")),
                        ("username".to_string(), json!({ "$var": "username" })),
                        ("password".to_string(), json!({ "$var": "basic_secret" })),
                    ]),
                },
                PlanStep {
                    id: "fill".into(),
                    tool: "browser_act".into(),
                    arguments: serde_json::Map::from_iter([
                        ("kind".to_string(), json!("fill")),
                        ("value".to_string(), json!({ "$var": "fill_user" })),
                    ]),
                },
            ],
            project: None,
        };
        let redacted = redact_plan(&plan);
        assert_eq!(redacted.variables["basic_secret"], json!("<redacted>"));
        assert_eq!(redacted.variables["username"], json!("<redacted>"));
        assert_eq!(redacted.variables["fill_user"], json!("visible-user"));
        assert_eq!(redacted.variables["url"], json!("http://127.0.0.1:8080/basic-auth"));
        assert_eq!(redacted.steps[1].arguments["password"], json!("<redacted>"));
        assert_eq!(redacted.steps[2].arguments["value"], json!({ "$var": "fill_user" }));
        assert!(!resume_blocked_by_http_auth_secrets(&plan, 1));
        assert!(resume_blocked_by_http_auth_secrets(&redacted, 1));
        assert!(!resume_blocked_by_http_auth_secrets(&redacted, 2));
    }
}
