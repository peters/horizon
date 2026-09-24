use super::{AllocationId, BindingError, ControllerId};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Explicit machine-local worker selection; never accepted from repository YAML.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Placement {
    NewWorker {
        #[serde(default)]
        sharing: SharingMode,
    },
    ExistingWorker(ExistingWorker),
}

impl Default for Placement {
    fn default() -> Self {
        Self::NewWorker {
            sharing: SharingMode::default(),
        }
    }
}

/// Image support alone never authorizes sharing a dedicated allocation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SharingMode {
    #[default]
    Dedicated,
    TrustedShared,
}

/// References saved bindings, never credential values or mutable machine defaults.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "ExistingBinding", into = "ExistingBinding")]
pub struct ExistingWorker(ExistingBinding);

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExistingBinding {
    allocation_id: AllocationId,
    controller_id: ControllerId,
    provider_credential_file: PathBuf,
    ssh_identity_file: PathBuf,
}

impl ExistingWorker {
    /// # Errors
    /// Rejects relative paths so later working-directory changes cannot retarget bindings.
    pub fn new(
        allocation_id: AllocationId,
        controller_id: ControllerId,
        provider_credential_file: PathBuf,
        ssh_identity_file: PathBuf,
    ) -> Result<Self, BindingError> {
        ExistingBinding {
            allocation_id,
            controller_id,
            provider_credential_file,
            ssh_identity_file,
        }
        .try_into()
    }

    #[must_use]
    pub const fn allocation_id(&self) -> AllocationId {
        self.0.allocation_id
    }

    #[must_use]
    pub const fn controller_id(&self) -> ControllerId {
        self.0.controller_id
    }

    #[must_use]
    pub fn provider_credential_file(&self) -> &Path {
        &self.0.provider_credential_file
    }

    #[must_use]
    pub fn ssh_identity_file(&self) -> &Path {
        &self.0.ssh_identity_file
    }
}

impl TryFrom<ExistingBinding> for ExistingWorker {
    type Error = BindingError;

    fn try_from(value: ExistingBinding) -> Result<Self, Self::Error> {
        if !value.provider_credential_file.is_absolute() || !value.ssh_identity_file.is_absolute() {
            return Err(BindingError::CredentialPath);
        }
        Ok(Self(value))
    }
}

impl From<ExistingWorker> for ExistingBinding {
    fn from(value: ExistingWorker) -> Self {
        value.0
    }
}

/// Versioned placement intent. Parsing is pure and cannot inspect or allocate a worker.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "Envelope", into = "Envelope")]
pub struct PlacementBinding(Placement);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    #[serde(default)]
    placement: Placement,
}

impl PlacementBinding {
    pub const VERSION: u32 = 1;

    #[must_use]
    pub const fn new(placement: Placement) -> Self {
        Self(placement)
    }

    #[must_use]
    pub const fn placement(&self) -> &Placement {
        &self.0
    }

    /// # Errors
    /// Rejects malformed, unknown-version or invalid bindings without disclosing input.
    pub fn parse(bytes: &[u8]) -> Result<Self, BindingError> {
        serde_json::from_slice(bytes).map_err(|_| BindingError::Encoding)
    }
}

impl TryFrom<Envelope> for PlacementBinding {
    type Error = BindingError;

    fn try_from(value: Envelope) -> Result<Self, Self::Error> {
        if value.version != Self::VERSION {
            return Err(BindingError::Version);
        }
        Ok(Self(value.placement))
    }
}

impl From<PlacementBinding> for Envelope {
    fn from(value: PlacementBinding) -> Self {
        Self {
            version: PlacementBinding::VERSION,
            placement: value.0,
        }
    }
}
