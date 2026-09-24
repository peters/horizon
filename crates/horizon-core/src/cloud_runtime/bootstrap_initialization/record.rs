use super::{Error, Request, Result};
use crate::cloud_runtime::{
    bootstrap_recovery::{self, connection::read},
    command::Runner,
    owner::Owner,
};
use horizon_cloud::{CreateState, Credential, WorkerSpec, runpod::volumes};
use horizon_cloud_protocol::{
    OperationId,
    bootstrap::{BootstrapOutcome, BootstrapPayload, BootstrapReceipt, RecoveryRequest, Startup},
    signed::{Action, Intent, SignedIntent, Target},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const KEY: &str = "bootstrap_initialization";
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileBinding {
    path: PathBuf,
    hash: [u8; 32],
}

impl FileBinding {
    pub fn matches(&self, binding: &bootstrap_recovery::connection::Binding) -> bool {
        binding.matches_identity(&self.path, &self.hash)
    }
    pub fn capture(path: &Path) -> Result<(Self, zeroize::Zeroizing<Vec<u8>>)> {
        let (path, bytes) = read(path, true)?;
        Ok((
            Self {
                path,
                hash: Sha256::digest(&bytes).into(),
            },
            bytes,
        ))
    }
    pub fn credential(path: &Path) -> Result<(Self, Credential)> {
        let (binding, bytes) = Self::capture(path)?;
        if bytes.len() > 4096 {
            return Err(Error::Invalid);
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| Error::Invalid)?.trim();
        Ok((binding, Credential::new(text.to_owned())?))
    }
    pub fn identity(path: &Path, runner: &Runner<'_>) -> Result<(Self, String)> {
        let (binding, bytes) = Self::capture(path)?;
        let mut snapshot = tempfile::NamedTempFile::new()?;
        snapshot.write_all(&bytes)?;
        let output = runner.private_exchange(
            Command::new("ssh-keygen")
                .args(["-y", "-P", "", "-f"])
                .arg(snapshot.path()),
            &[],
            Duration::from_secs(5),
        )?;
        let text = std::str::from_utf8(&output).map_err(|_| Error::Invalid)?;
        let fields: Vec<_> = text.split_whitespace().collect();
        if fields.len() < 2 || fields[0] != "ssh-ed25519" {
            return Err(Error::Invalid);
        }
        Ok((binding, format!("{} {}", fields[0], fields[1])))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Phase {
    Creating,
    Prepared,
    Requested,
    Completed,
    AbandonRequested,
    DeleteConfirmed,
    Deleted,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Signed {
    request: String,
    receipt: BootstrapReceipt,
}
impl Signed {
    pub fn receipt(&self) -> &BootstrapReceipt {
        &self.receipt
    }
    pub fn new(
        owner: &Owner,
        startup: &Startup,
        worker_id: &str,
        payload: &BootstrapPayload,
        outcome: BootstrapOutcome,
    ) -> Result<Self> {
        let payload = serde_json::to_string(payload).map_err(|_| Error::Invalid)?;
        let intent = Intent::new(
            &startup.controller,
            OperationId::generate(),
            0,
            Target::Allocation {},
            Action::Bootstrap,
            payload.as_bytes(),
        )
        .map_err(|_| Error::Invalid)?;
        let receipt = BootstrapReceipt {
            version: 1,
            startup: startup.clone(),
            worker_id: worker_id.into(),
            operation: intent.operation(),
            fingerprint: intent.fingerprint().map_err(|_| Error::Invalid)?,
            outcome,
        };
        let message = serde_json::to_string(&owner.sign(intent)?).map_err(|_| Error::Invalid)?;
        Ok(Self {
            request: serde_json::to_string(&RecoveryRequest { message, payload }).map_err(|_| Error::Invalid)?,
            receipt,
        })
    }
    pub fn request(
        &self,
        target: &bootstrap_recovery::Target,
        expected: &BootstrapPayload,
        outcome: BootstrapOutcome,
    ) -> Result<&[u8]> {
        if self.request.len() > 64 * 1024 {
            return Err(Error::Invalid);
        }
        let request: RecoveryRequest = serde_json::from_str(&self.request).map_err(|_| Error::Invalid)?;
        let signed = SignedIntent::parse(request.message.as_bytes()).map_err(|_| Error::Invalid)?;
        let intent = signed
            .verify(&target.startup.controller, request.payload.as_bytes())
            .map_err(|_| Error::Invalid)?;
        if intent.action() != Action::Bootstrap
            || intent.expected_revision() != 0
            || *intent.target() != (Target::Allocation {})
            || serde_json::from_str::<BootstrapPayload>(&request.payload).map_err(|_| Error::Invalid)? != *expected
            || self.receipt
                != (BootstrapReceipt {
                    version: 1,
                    startup: target.startup.clone(),
                    worker_id: target.worker_id.clone(),
                    operation: intent.operation(),
                    fingerprint: intent.fingerprint().map_err(|_| Error::Invalid)?,
                    outcome,
                })
        {
            return Err(Error::Invalid);
        }
        Ok(self.request.as_bytes())
    }
    pub fn confirm(&self, bytes: &[u8]) -> Result<()> {
        if bytes.len() > 64 * 1024
            || serde_json::from_slice::<BootstrapReceipt>(bytes).map_err(|_| Error::Invalid)? != self.receipt
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    pub version: u32,
    pub request: Request,
    pub account: FileBinding,
    pub identity: FileBinding,
    pub spec: WorkerSpec,
    pub volume_spec: volumes::Spec,
    pub volume: volumes::State,
    pub worker: CreateState,
    pub startup: Option<Startup>,
    pub phase: Phase,
    pub requested: bool,
    pub cleanup_receipt: Option<BootstrapReceipt>,
    pub initialize: Option<Signed>,
    pub abandon: Option<Signed>,
}
impl Record {
    pub fn load(owner: &Owner) -> Result<Option<Self>> {
        let value = owner.load()?;
        let object = value.as_object().ok_or(Error::Invalid)?;
        object
            .get(KEY)
            .map(|value| serde_json::from_value(value.clone()).map_err(|_| Error::Invalid))
            .transpose()
    }
    pub fn save(&self, owner: &mut Owner) -> Result<()> {
        let mut value = owner.load()?;
        value
            .as_object_mut()
            .ok_or(Error::Invalid)?
            .insert(KEY.into(), serde_json::to_value(self).map_err(|_| Error::Invalid)?);
        owner.save(value)?;
        Ok(())
    }
    pub fn verify(
        &self,
        owner: &Owner,
        request: &Request,
        account: &FileBinding,
        identity: &FileBinding,
    ) -> Result<()> {
        if self.version != 1 || self.request != *request || self.account != *account || self.identity != *identity {
            return Err(Error::Invalid);
        }
        self.spec.validate()?;
        let mut expected_spec = request.worker.clone();
        expected_spec.startup_metadata.clone_from(&self.spec.startup_metadata);
        if expected_spec != self.spec
            || self.volume_spec.operation_id != self.spec.operation_id
            || self.volume_spec.size != u32::from(self.spec.profile.storage.volume_gb)
            || (!self.spec.data_centers.is_empty()
                && !self.spec.data_centers.contains(&self.volume_spec.data_center_id))
            || (matches!(
                self.phase,
                Phase::Requested | Phase::Completed | Phase::AbandonRequested
            ) && !self.requested)
            || (self.requested && self.initialize.is_none())
            || (self.requested
                && matches!(self.phase, Phase::DeleteConfirmed | Phase::Deleted)
                && self.cleanup_receipt.is_none())
        {
            return Err(Error::Invalid);
        }
        if let Some(receipt) = &self.cleanup_receipt
            && (!self.requested
                || !matches!(self.phase, Phase::DeleteConfirmed | Phase::Deleted)
                || self.abandon.as_ref().is_none_or(|signed| signed.receipt != *receipt))
        {
            return Err(Error::Invalid);
        }
        self.volume.verify(&self.volume_spec)?;
        if let Some(startup) = &self.startup {
            startup.validate().map_err(|_| Error::Invalid)?;
            if startup.controller != owner.binding()?
                || startup.sharing != request.sharing
                || startup.worker_operation != self.spec.operation_id
                || self
                    .spec
                    .startup_metadata
                    .as_ref()
                    .map(horizon_cloud::StartupMetadata::as_str)
                    != Some(serde_json::to_string(startup).map_err(|_| Error::Invalid)?.as_str())
            {
                return Err(Error::Invalid);
            }
        } else if !matches!(self.phase, Phase::Creating | Phase::DeleteConfirmed | Phase::Deleted)
            || self.requested
            || self.spec.startup_metadata.is_some()
            || self.worker != CreateState::Prepared
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
