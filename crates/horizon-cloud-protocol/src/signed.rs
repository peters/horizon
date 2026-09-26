//! Signed management intent. Authenticity is not membership or provider authority.
//!
//! Version 1 signs a domain prefix followed by this type's compact JSON field order.
//! Payload bytes are hashed exactly, without JSON normalization. Wire whitespace
//! outside the payload is immaterial; a protocol version fixes the signing encoding.

use crate::{AllocationId, ControllerId, OperationId, ProjectIdentity};
use ring::{
    digest,
    signature::{self, Ed25519KeyPair, KeyPair},
};
use serde::{Deserialize, Serialize};

const VERSION: u32 = 1;
const DOMAIN: &[u8] = b"horizon-cloud-management-v1\0";
pub const MAX_PAYLOAD_BYTES: usize = 256 * 1024;
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Invalid management request encoding or size")]
    Encoding,
    #[error("Unsupported management protocol")]
    Version,
    #[error("Management action and target scope differ")]
    Scope,
    #[error("Management request does not match the pinned controller")]
    Controller,
    #[error("Management payload fingerprint differs")]
    Fingerprint,
    #[error("Management request signature is invalid")]
    Signature,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Allocation {},
    Project { identity: ProjectIdentity },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Bootstrap,
    InspectAllocation,
    PrepareWorkerTransition,
    ConfirmWorkerTransition,
    ReconcileWorkerTransition,
    CancelWorkerTransition,
    ResumeWorker,
    AttachProject,
    InspectProject,
    ImportProjectSource,
    ReserveProjectSession,
    PrepareProjectSession,
    StartProjectSession,
    StopProjectSession,
    InspectProjectSession,
    ReconcileProject,
    StopProjectSessions,
    RemoveProject,
    PrepareProjectDataDeletion,
    ConfirmProjectDataDeletion,
    ReconcileProjectDataDeletion,
}

impl Action {
    fn project_scoped(self) -> bool {
        match self {
            Self::Bootstrap
            | Self::InspectAllocation
            | Self::PrepareWorkerTransition
            | Self::ConfirmWorkerTransition
            | Self::ReconcileWorkerTransition
            | Self::CancelWorkerTransition
            | Self::ResumeWorker => false,
            Self::AttachProject
            | Self::ImportProjectSource
            | Self::ReserveProjectSession
            | Self::PrepareProjectSession
            | Self::StartProjectSession
            | Self::StopProjectSession
            | Self::InspectProjectSession
            | Self::InspectProject
            | Self::ReconcileProject
            | Self::StopProjectSessions
            | Self::RemoveProject
            | Self::PrepareProjectDataDeletion
            | Self::ConfirmProjectDataDeletion
            | Self::ReconcileProjectDataDeletion => true,
        }
    }
}

/// Every routing, replay and concurrency field participates in the signature.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    version: u32,
    allocation: AllocationId,
    controller: ControllerId,
    target: Target,
    operation: OperationId,
    expected_revision: u64,
    action: Action,
    payload_hash: [u8; 32],
}

impl Intent {
    /// # Errors
    /// Rejects oversized payloads and action/target scope mismatches.
    pub fn new(
        binding: &ControllerBinding,
        operation: OperationId,
        expected_revision: u64,
        target: Target,
        action: Action,
        payload: &[u8],
    ) -> Result<Self, Error> {
        let intent = Self {
            version: VERSION,
            allocation: binding.allocation,
            controller: binding.controller,
            target,
            operation,
            expected_revision,
            action,
            payload_hash: fingerprint(payload)?,
        };
        intent.validate()?;
        Ok(intent)
    }

    #[must_use]
    pub const fn operation(&self) -> OperationId {
        self.operation
    }
    #[must_use]
    pub const fn expected_revision(&self) -> u64 {
        self.expected_revision
    }
    #[must_use]
    pub const fn action(&self) -> Action {
        self.action
    }
    #[must_use]
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// Persist alongside the operation receipt to reject changed retry inputs.
    ///
    /// # Errors
    /// Rejects intent encoding failures.
    pub fn fingerprint(&self) -> Result<[u8; 32], Error> {
        fingerprint(&self.signing_bytes()?)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.version != VERSION {
            return Err(Error::Version);
        }
        if self.action.project_scoped() != matches!(self.target, Target::Project { .. }) {
            return Err(Error::Scope);
        }
        Ok(())
    }

    fn signing_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut bytes = DOMAIN.to_vec();
        bytes.extend(serde_json::to_vec(self).map_err(|_| Error::Encoding)?);
        Ok(bytes)
    }
}

/// Public identity pinned by verified bootstrap. It carries no signing material.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerBinding {
    allocation: AllocationId,
    controller: ControllerId,
    public_key: [u8; 32],
}

impl ControllerBinding {
    #[must_use]
    pub const fn new(allocation: AllocationId, controller: ControllerId, public_key: [u8; 32]) -> Self {
        Self {
            allocation,
            controller,
            public_key,
        }
    }
}

/// Decode only through [`Self::parse`] so callers cannot bypass the wire-size bound.
///
/// ```compile_fail
/// use horizon_cloud_protocol::signed::SignedIntent;
/// let request: SignedIntent = serde_json::from_str("{}").unwrap();
/// ```
#[derive(Clone, Debug, Serialize)]
pub struct SignedIntent {
    intent: Intent,
    signature: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EncodedIntent {
    intent: Intent,
    signature: Vec<u8>,
}

impl SignedIntent {
    /// Call only after host ownership and journal-generation checks. This helper
    /// neither loads credentials nor grants authority to a public API caller.
    ///
    /// # Errors
    /// Rejects mismatched controller keys and unsupported intent shapes.
    pub fn sign(intent: Intent, binding: &ControllerBinding, key: &Ed25519KeyPair) -> Result<Self, Error> {
        intent.validate()?;
        if intent.allocation != binding.allocation
            || intent.controller != binding.controller
            || key.public_key().as_ref() != binding.public_key
        {
            return Err(Error::Controller);
        }
        let signature = key.sign(&intent.signing_bytes()?).as_ref().to_vec();
        Ok(Self { intent, signature })
    }

    /// Sign with a credential-owning backend without copying its private key into
    /// this protocol crate. The resulting signature must match the pinned key.
    ///
    /// # Errors
    /// Rejects unsupported intents, controller mismatches and invalid signatures.
    pub fn sign_with(
        intent: Intent,
        binding: &ControllerBinding,
        signer: impl FnOnce(&[u8]) -> Vec<u8>,
    ) -> Result<Self, Error> {
        intent.validate()?;
        if intent.allocation != binding.allocation || intent.controller != binding.controller {
            return Err(Error::Controller);
        }
        let bytes = intent.signing_bytes()?;
        let signature = signer(&bytes);
        signature::UnparsedPublicKey::new(&signature::ED25519, binding.public_key)
            .verify(&bytes, &signature)
            .map_err(|_| Error::Signature)?;
        Ok(Self { intent, signature })
    }

    /// # Errors
    /// Enforces the wire bound before deserialization; values are never included in errors.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err(Error::Encoding);
        }
        let encoded: EncodedIntent = serde_json::from_slice(bytes).map_err(|_| Error::Encoding)?;
        Ok(Self {
            intent: encoded.intent,
            signature: encoded.signature,
        })
    }

    /// Authenticate under the worker allocation lock, then validate the full project
    /// binding, action payload, expected revision and operation receipt before effects.
    /// A valid signature alone never authorizes a target or satisfies those checks.
    ///
    /// # Errors
    /// Refuses any changed routing/concurrency field, controller, payload or signature.
    pub fn verify<'a>(&'a self, binding: &ControllerBinding, payload: &[u8]) -> Result<&'a Intent, Error> {
        self.intent.validate()?;
        if self.intent.allocation != binding.allocation || self.intent.controller != binding.controller {
            return Err(Error::Controller);
        }
        if fingerprint(payload)? != self.intent.payload_hash {
            return Err(Error::Fingerprint);
        }
        signature::UnparsedPublicKey::new(&signature::ED25519, binding.public_key)
            .verify(&self.intent.signing_bytes()?, &self.signature)
            .map_err(|_| Error::Signature)?;
        Ok(&self.intent)
    }
}

fn fingerprint(payload: &[u8]) -> Result<[u8; 32], Error> {
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(Error::Encoding);
    }
    digest::digest(&digest::SHA256, payload)
        .as_ref()
        .try_into()
        .map_err(|_| Error::Encoding)
}

#[cfg(test)]
mod tests;
