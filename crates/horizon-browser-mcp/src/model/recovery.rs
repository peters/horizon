use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteAllocationsInput {
    pub operation: RecoveryOperation,
    /// Safe reference returned by list; required only for reconcile.
    pub reference: Option<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecoveryOperation {
    List,
    Reconcile,
}

impl RemoteAllocationsInput {
    pub(crate) fn reference(self) -> Result<Option<String>, String> {
        match (self.operation, self.reference) {
            (RecoveryOperation::List, None) => Ok(None),
            (RecoveryOperation::Reconcile, Some(reference)) if !reference.is_empty() => Ok(Some(reference)),
            _ => Err("list takes no reference; reconcile requires a reference returned by list".into()),
        }
    }
}
