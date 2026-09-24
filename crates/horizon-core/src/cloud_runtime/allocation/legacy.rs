//! Lossless dedicated-deployment conversion. No filesystem, provider or worker I/O.
mod records;
#[cfg(test)]
mod tests;
mod unique_json;

use super::{AllocationId, ControllerId, ProjectId, ProjectIdentity, SharingMode};
use crate::cloud_runtime::state::Deployment;
use records::{Allocation, Project};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid or unsupported legacy deployment encoding")]
    Encoding,
    #[error("Unsupported migration record version")]
    Version,
    #[error("Migration records do not describe the same dedicated project")]
    Ownership,
}

/// A validated pair for the future journal transaction to publish together.
/// Constructing or decoding this pair grants no provider-operation authority.
#[derive(Clone, Debug)]
pub struct Records {
    allocation: Allocation,
    project: Project,
}

impl Records {
    /// Convert the complete version-1 payload using IDs supplied by durable intent.
    ///
    /// # Errors
    /// Rejects unknown fields, unsupported versions and mismatched cloud membership.
    pub fn from_legacy(
        bytes: &[u8],
        identity: ProjectIdentity,
        allocation: AllocationId,
        controller: ControllerId,
    ) -> Result<Self, Error> {
        let mut legacy: Deployment = decode_preserving(bytes)?;
        if legacy.version != 1 {
            return Err(Error::Version);
        }
        if identity.cloud_id() != legacy.cloud_id {
            return Err(Error::Ownership);
        }
        legacy.normalize_readiness_history();
        let pair = Self::split(legacy, identity, allocation, controller);
        pair.validate()?;
        Ok(pair)
    }

    /// Decode both independently persisted records; neither can stand alone.
    ///
    /// # Errors
    /// Rejects unsupported versions, unknown fields and conflicting ownership.
    pub fn decode(allocation: &[u8], project: &[u8]) -> Result<Self, Error> {
        let pair = Self {
            allocation: decode_preserving(allocation)?,
            project: decode_preserving(project)?,
        };
        pair.validate()?;
        Ok(pair)
    }

    /// # Errors
    /// Returns an encoding error without disclosing persisted values.
    pub fn allocation_bytes(&self) -> Result<Vec<u8>, Error> {
        serde_json::to_vec_pretty(&self.allocation).map_err(|_| Error::Encoding)
    }

    /// # Errors
    /// Returns an encoding error without disclosing persisted values.
    pub fn project_bytes(&self) -> Result<Vec<u8>, Error> {
        serde_json::to_vec_pretty(&self.project).map_err(|_| Error::Encoding)
    }

    #[must_use]
    pub const fn allocation_id(&self) -> AllocationId {
        self.allocation.id
    }

    #[must_use]
    pub const fn controller_id(&self) -> ControllerId {
        self.allocation.controller
    }

    #[must_use]
    pub const fn project_id(&self) -> ProjectId {
        self.project.identity.project_id()
    }

    #[must_use]
    pub fn identity(&self) -> &ProjectIdentity {
        &self.project.identity
    }

    fn validate(&self) -> Result<(), Error> {
        if self.allocation.version != Allocation::VERSION || self.project.version != Project::VERSION {
            return Err(Error::Version);
        }
        if self.allocation.id != self.project.allocation
            || self.allocation.member != self.project.identity
            || self.allocation.sharing != SharingMode::Dedicated
        {
            return Err(Error::Ownership);
        }
        Ok(())
    }
}

fn decode_preserving<T: DeserializeOwned + Serialize>(bytes: &[u8]) -> Result<T, Error> {
    let mut original = unique_json::parse(bytes).map_err(|_| Error::Encoding)?;
    let parsed: T = serde_json::from_value(original.clone()).map_err(|_| Error::Encoding)?;
    let normalized = serde_json::to_value(&parsed).map_err(|_| Error::Encoding)?;
    normalize_supported_input(&mut original, &normalized);
    if !preserves_present_values(&original, &normalized) {
        return Err(Error::Encoding);
    }
    Ok(parsed)
}

fn normalize_supported_input(original: &mut Value, normalized: &Value) {
    // Profiles recursively deny unknown fields. Their defaults, optional omissions
    // and capability sets are therefore safely canonicalized by the existing schema.
    // The worker's custom address/rate parsers and project target set likewise own
    // normalization; do not duplicate their parsing or default-selection rules here.
    for pointer in [
        "/profile",
        "/spec/profile",
        "/worker/publicIp",
        "/worker/costPerHr",
        "/browserstack_targets",
    ] {
        if let Some(input) = original.pointer_mut(pointer)
            && let Some(output) = normalized.pointer(pointer)
        {
            *input = output.clone();
        }
    }
    if let Some(worker) = original.get_mut("worker").and_then(Value::as_object_mut)
        && let Some(image) = worker.remove("image")
    {
        // Typed decoding above rejects input containing both spellings.
        worker.insert("imageName".into(), image);
    }
}

// After supported normalization, no present value may disappear or change.
fn preserves_present_values(original: &Value, normalized: &Value) -> bool {
    match (original, normalized) {
        (Value::Object(before), Value::Object(after)) => before.iter().all(|(key, value)| {
            after
                .get(key)
                .is_some_and(|current| preserves_present_values(value, current))
        }),
        (Value::Array(before), Value::Array(after)) => {
            before.len() == after.len()
                && before
                    .iter()
                    .zip(after)
                    .all(|(left, right)| preserves_present_values(left, right))
        }
        _ => original == normalized,
    }
}
