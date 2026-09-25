use super::{Change, Error, Owner, Result};
use crate::cloud_runtime::bootstrap_recovery::connection::Binding;
use horizon_cloud_protocol::{
    OperationId,
    bootstrap::RecoveryRequest,
    membership::{Manifest, Receipt, Request, State},
    signed::{Intent, Target},
};
use serde::{Deserialize, Serialize};

pub(super) const KEY: &str = "project_reservations";
const LIMIT: usize = 4 * horizon_cloud_protocol::membership::MAX_MANIFEST_BYTES;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Journal {
    version: u32,
    pub binding: Binding,
    pub image_digest: String,
    pub manifest: Manifest,
    pub pending: Option<Pending>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pending {
    pub request: String,
    pub next: Manifest,
    pub receipt: Receipt,
}

impl Journal {
    pub fn new(binding: Binding, image_digest: String) -> Self {
        Self {
            version: 1,
            manifest: Manifest::empty(binding.startup.clone(), binding.worker_id.clone()),
            binding,
            image_digest,
            pending: None,
        }
    }
    pub fn load(owner: &Owner) -> Result<Option<Self>> {
        let payload = owner.load()?;
        let Some(value) = payload.get(KEY) else {
            return Ok(None);
        };
        if serde_json::to_vec(value).map_err(|_| Error::Invalid)?.len() > LIMIT {
            return Err(Error::Invalid);
        }
        let journal: Self = serde_json::from_value(value.clone()).map_err(|_| Error::Invalid)?;
        journal.verify(owner)?;
        Ok(Some(journal))
    }
    pub fn save(&self, owner: &mut Owner) -> Result<()> {
        self.verify(owner)?;
        let mut payload = owner.load()?;
        payload
            .as_object_mut()
            .ok_or(Error::Invalid)?
            .insert(KEY.into(), serde_json::to_value(self).map_err(|_| Error::Invalid)?);
        owner.save(payload)?;
        Ok(())
    }
    fn verify(&self, owner: &Owner) -> Result<()> {
        if self.version != 1
            || self.binding.startup.controller != owner.binding()?
            || self.manifest.startup != self.binding.startup
            || self.manifest.worker_id != self.binding.worker_id
            || !horizon_cloud::valid_image(&self.image_digest)
            || serde_json::to_vec(self).map_err(|_| Error::Invalid)?.len() > LIMIT
        {
            return Err(Error::Invalid);
        }
        self.manifest.validate().map_err(|_| Error::Invalid)?;
        if let Some(pending) = &self.pending {
            if pending.request.len() > horizon_cloud_protocol::membership::MAX_MANIFEST_BYTES {
                return Err(Error::Invalid);
            }
            let request: RecoveryRequest = serde_json::from_str(&pending.request).map_err(|_| Error::Invalid)?;
            let (next, receipt) = self
                .manifest
                .next(&request.message, &request.payload)
                .map_err(|_| Error::Invalid)?;
            if pending.next != next || pending.receipt != receipt {
                return Err(Error::Invalid);
            }
        }
        Ok(())
    }
    pub fn require_change(&self, change: &Change) -> Result<()> {
        if let Change::Reserve(request) = change
            && request.image_digest != self.image_digest
        {
            return Err(Error::Invalid);
        }
        if let Some(pending) = &self.pending {
            if !matches!(change, Change::Resume) && !pending.matches(change)? {
                return Err(Error::Pending);
            }
        } else if matches!(change, Change::Resume) {
            return Err(Error::Missing);
        }
        Ok(())
    }
    pub fn prepare(&mut self, owner: &Owner, change: &Change) -> Result<()> {
        self.require_change(change)?;
        if self.pending.is_some() {
            return Ok(());
        }
        let (identity, payload, state) = match change {
            Change::Reserve(request) => (
                &request.project,
                Request::Reserve {
                    capabilities: request.capabilities.clone(),
                    ports: request.ports.clone(),
                },
                State::Attaching,
            ),
            Change::Cancel(identity) => (identity, Request::Cancel {}, State::Removed),
            Change::Resume => return Err(Error::Missing),
        };
        let payload = serde_json::to_string(&payload).map_err(|_| Error::Invalid)?;
        let message = if let Some(saved) = self
            .manifest
            .operations
            .iter()
            .find(|entry| entry.receipt.identity == *identity && entry.receipt.state == state)
        {
            if saved.payload != payload {
                return Err(Error::Invalid);
            }
            saved.message.clone()
        } else {
            let intent = Intent::new(
                &self.binding.startup.controller,
                OperationId::generate(),
                self.manifest.revision,
                Target::Project {
                    identity: identity.clone(),
                },
                if state == State::Attaching {
                    horizon_cloud_protocol::signed::Action::AttachProject
                } else {
                    horizon_cloud_protocol::signed::Action::RemoveProject
                },
                payload.as_bytes(),
            )
            .map_err(|_| Error::Invalid)?;
            serde_json::to_string(&owner.sign(intent)?).map_err(|_| Error::Invalid)?
        };
        let (next, receipt) = self.manifest.next(&message, &payload).map_err(|_| Error::Invalid)?;
        let request = serde_json::to_string(&RecoveryRequest { message, payload }).map_err(|_| Error::Invalid)?;
        self.pending = Some(Pending { request, next, receipt });
        self.verify(owner)
    }
}

impl Pending {
    fn matches(&self, change: &Change) -> Result<bool> {
        let request: RecoveryRequest = serde_json::from_str(&self.request).map_err(|_| Error::Invalid)?;
        let payload: Request = serde_json::from_str(&request.payload).map_err(|_| Error::Invalid)?;
        Ok(match change {
            Change::Reserve(request) => {
                self.receipt.identity == request.project
                    && payload
                        == (Request::Reserve {
                            capabilities: request.capabilities.clone(),
                            ports: request.ports.clone(),
                        })
            }
            Change::Cancel(identity) => self.receipt.identity == *identity && payload == (Request::Cancel {}),
            Change::Resume => true,
        })
    }
    pub fn command(&self) -> &'static str {
        match self.receipt.state {
            State::Attaching => "horizon-cloud-worker reserve-project",
            State::Removed => "horizon-cloud-worker cancel-project-reservation",
        }
    }
}
