//! Bounded plan literals substituted with `{"$var":"name"}`.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::{PlanError, PlanStep, valid_identifier};

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
            return Err(PlanError::InvalidVariableName(format!(
                "{name}: variable values must be JSON literals, not $ref or $var"
            )));
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
        assert!(matches!(validate(&nested), Err(PlanError::InvalidVariableName(_))));
    }
}
