//! Bounded plan literals substituted with `{"$var":"name"}`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

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
            if matches!(step.arguments.get(field), Some(Value::String(_))) {
                step.arguments
                    .insert(field.to_string(), Value::String(REDACTED_SECRET.to_string()));
            }
        }
        if let Some(Value::String(origin)) = step.arguments.get_mut("origin")
            && horizon_browser_protocol::parse_http_auth_origin(origin).is_err()
        {
            *origin = REDACTED_SECRET.to_string();
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
        if let Some(name) = variable_name(step.arguments.get("origin"))
            && let Some(Value::String(origin)) = plan.variables.get(name)
            && horizon_browser_protocol::parse_http_auth_origin(origin).is_err()
        {
            names.insert(name.to_string());
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

/// True when remaining work would replay a redacted HTTP auth secret.
#[must_use]
pub(crate) fn resume_blocked_by_http_auth_secrets(plan: &Plan, start_index: usize) -> bool {
    let secret_variables = http_auth_secret_variables(plan)
        .into_iter()
        .filter(|name| plan.variables.get(name).and_then(Value::as_str) == Some(REDACTED_SECRET))
        .collect();
    plan.steps.get(start_index..).is_some_and(|steps| {
        steps.iter().any(|step| {
            remaining_http_auth_cannot_resume(step)
                || step
                    .arguments
                    .values()
                    .any(|value| value_references_variables(value, &secret_variables))
        })
    })
}

fn remaining_http_auth_cannot_resume(step: &PlanStep) -> bool {
    step.tool == "browser_http_auth"
        && step.arguments.get("operation").and_then(Value::as_str) != Some("clear")
        && HTTP_AUTH_SECRET_FIELDS
            .iter()
            .any(|field| match step.arguments.get(*field) {
                None | Some(Value::Null) => true,
                Some(Value::String(value)) => value == REDACTED_SECRET || (*field == "username" && value.is_empty()),
                Some(_) => false,
            })
}

fn value_references_variables(value: &Value, names: &BTreeSet<String>) -> bool {
    match value {
        Value::Object(object) => {
            object
                .get("$var")
                .and_then(Value::as_str)
                .is_some_and(|name| names.contains(name))
                || object.values().any(|value| value_references_variables(value, names))
        }
        Value::Array(values) => values.iter().any(|value| value_references_variables(value, names)),
        _ => false,
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
    use serde_json::{Map, json};

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
    fn ordinary_redacted_literal_does_not_block_resume() {
        let plan = Plan {
            version: 1,
            variables: BTreeMap::from([("label".into(), json!("<redacted>"))]),
            steps: vec![PlanStep {
                id: "fill".into(),
                tool: "browser_act".into(),
                arguments: Map::from_iter([("value".into(), json!({"$var": "label"}))]),
            }],
            project: None,
        };
        assert!(!resume_blocked_by_http_auth_secrets(&redact_plan(&plan), 0));
    }

    #[test]
    fn redacted_username_blocks_resume_with_a_result_reference_password() {
        let step = PlanStep {
            id: "auth".into(),
            tool: "browser_http_auth".into(),
            arguments: Map::from_iter([
                ("operation".into(), json!("set")),
                ("username".into(), json!(REDACTED_SECRET)),
                ("password".into(), json!({"$ref": "previous#/password"})),
            ]),
        };
        assert!(remaining_http_auth_cannot_resume(&step));
    }

    #[test]
    fn invalid_auth_origins_are_redacted_in_literals_and_reused_variables() {
        for origin in ["http://user:secret@example.test", "http:user:secret@example.test"] {
            let mut plan = Plan {
                version: 1,
                variables: BTreeMap::from([("site".into(), json!(origin))]),
                steps: vec![
                    PlanStep {
                        id: "auth".into(),
                        tool: "browser_http_auth".into(),
                        arguments: Map::from_iter([("origin".into(), json!({"$var": "site"}))]),
                    },
                    PlanStep {
                        id: "navigate".into(),
                        tool: "browser_navigate".into(),
                        arguments: Map::from_iter([("url".into(), json!({"$var": "site"}))]),
                    },
                ],
                project: None,
            };
            let saved = redact_plan(&plan);
            assert_eq!(saved.variables["site"], json!(REDACTED_SECRET));
            assert!(resume_blocked_by_http_auth_secrets(&saved, 1));
            plan.variables.clear();
            plan.steps[0].arguments.insert("origin".into(), json!(origin));
            assert_eq!(redact_plan(&plan).steps[0].arguments["origin"], json!(REDACTED_SECRET));
        }
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
                    id: "reuse".into(),
                    tool: "browser_act".into(),
                    arguments: serde_json::Map::from_iter([
                        ("kind".to_string(), json!("fill")),
                        ("value".to_string(), json!({ "$var": "basic_secret" })),
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
        assert_eq!(
            redacted.steps[1].arguments["password"],
            json!({ "$var": "basic_secret" })
        );
        assert_eq!(redacted.steps[2].arguments["value"], json!({ "$var": "basic_secret" }));
        assert_eq!(redacted.steps[3].arguments["value"], json!({ "$var": "fill_user" }));
        assert!(!resume_blocked_by_http_auth_secrets(&plan, 1));
        assert!(resume_blocked_by_http_auth_secrets(&redacted, 1));
        assert!(resume_blocked_by_http_auth_secrets(&redacted, 2));
        assert!(!resume_blocked_by_http_auth_secrets(&redacted, 3));
        let mut substituted = redacted.clone();
        substituted.steps[1]
            .arguments
            .insert("operation".to_string(), json!({ "$var": "op" }));
        assert!(resume_blocked_by_http_auth_secrets(&substituted, 1));
    }
}
