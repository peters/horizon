//! Small typed views of the ARM resources this client reads. A response body is
//! untrusted: only the fields below are kept, and an identity field that does not
//! name the requested resource is an `IdentityMismatch`, never a partial view.
use super::{Failure, url::valid_resource_group_id};
use crate::CloudError;
use serde_json::Value;
use std::collections::BTreeMap;

/// A resource group as ARM reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Group {
    /// The group's resource id, checked against the requested subscription and group.
    pub id: String,
    pub name: String,
    pub location: String,
    pub provisioning_state: String,
    /// String tags only; a tag with another JSON type is dropped.
    pub tags: BTreeMap<String, String>,
}

pub(crate) fn text(value: &Value, pointer: &str) -> Option<String> {
    value.pointer(pointer).and_then(Value::as_str).map(str::to_string)
}

pub(crate) fn tags(value: &Value) -> BTreeMap<String, String> {
    value
        .get("tags")
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// A group body for `expected` in `subscription_id`. A missing name or id is an
/// invalid response; a name or id of another group is an identity mismatch.
pub(crate) fn group_info(value: &Value, subscription_id: &str, expected: &str) -> Result<Group, Failure> {
    let name = text(value, "/name").ok_or_else(invalid)?;
    let id = text(value, "/id").ok_or_else(invalid)?;
    if !name.eq_ignore_ascii_case(expected) || !valid_resource_group_id(&id, subscription_id, expected) {
        return Err(mismatch());
    }
    Ok(Group {
        id,
        name,
        location: text(value, "/location").unwrap_or_default(),
        provisioning_state: text(value, "/properties/provisioningState").unwrap_or_default(),
        tags: tags(value),
    })
}

fn invalid() -> Failure {
    Failure::Local(CloudError::InvalidResponse)
}

fn mismatch() -> Failure {
    Failure::Local(CloudError::IdentityMismatch)
}
