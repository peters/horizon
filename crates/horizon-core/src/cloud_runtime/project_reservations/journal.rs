use super::{Change, Error, Owner, Result};
use crate::cloud_runtime::bootstrap_recovery::connection::Binding;
use horizon_cloud_protocol::{
    OperationId,
    bootstrap::RecoveryRequest,
    membership::{Manifest, Receipt, Request},
    signed::{Intent, Target},
};
use serde::{Deserialize, Serialize};

pub(super) const KEY: &str = "project_reservations";
const LIMIT: usize = 4 * horizon_cloud_protocol::membership::MAX_MANIFEST_BYTES;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::cloud_runtime) struct Journal {
    version: u32,
    pub binding: Binding,
    pub image_digest: String,
    pub manifest: Manifest,
    pub pending: Option<Pending>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<super::source::Artifacts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<super::source::Generation>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::cloud_runtime) struct Pending {
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
            sources: Vec::new(),
            generation: None,
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
        let mut projects = std::collections::BTreeSet::new();
        for source in &self.sources {
            source.validate(owner)?;
            if !projects.insert(source.project.project_id()) {
                return Err(Error::Invalid);
            }
        }

        if let Some(generation) = &self.generation {
            generation.validate()?;
            if projects.contains(&generation.project.project_id())
                || !self
                    .manifest
                    .members
                    .iter()
                    .any(|member| member.identity == generation.project)
            {
                return Err(Error::Invalid);
            }
        }
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
        let (identity, payload) = match change {
            Change::Reserve(request) => (
                &request.project,
                Request::Reserve {
                    capabilities: request.capabilities.clone(),
                    ports: request.ports.clone(),
                },
            ),
            Change::PrepareNamespace(identity) => (identity, Request::PrepareNamespace {}),
            Change::ImportSource(identity, descriptor) => (
                identity,
                Request::ImportSource {
                    descriptor: descriptor.clone(),
                },
            ),
            Change::ReserveSession(identity, session) => (
                identity,
                Request::ReserveSession {
                    session: session.clone(),
                },
            ),
            Change::PrepareSession(identity, session_id) => (
                identity,
                Request::PrepareSession {
                    session_id: *session_id,
                },
            ),
            Change::Cancel(identity) => (identity, Request::Cancel {}),
            Change::Resume => return Err(Error::Missing),
        };
        let action = payload.action();
        let request = payload;
        let payload = serde_json::to_string(&request).map_err(|_| Error::Invalid)?;
        let message = if let Some(saved) = self.manifest.operations.iter().find(|entry| {
            entry.receipt.identity == *identity
                && serde_json::from_str::<Request>(&entry.payload).is_ok_and(|saved| match (&request, &saved) {
                    (Request::ReserveSession { session }, Request::ReserveSession { session: old }) => {
                        session.id == old.id
                    }
                    (Request::PrepareSession { session_id }, Request::PrepareSession { session_id: old }) => {
                        session_id == old
                    }
                    _ => saved.action() == action,
                })
        }) {
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
                action,
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
            Change::PrepareNamespace(identity) => {
                self.receipt.identity == *identity && payload == (Request::PrepareNamespace {})
            }
            Change::ImportSource(identity, descriptor) => {
                self.receipt.identity == *identity
                    && payload
                        == (Request::ImportSource {
                            descriptor: descriptor.clone(),
                        })
            }
            Change::ReserveSession(identity, session) => {
                self.receipt.identity == *identity
                    && payload
                        == (Request::ReserveSession {
                            session: session.clone(),
                        })
            }
            Change::PrepareSession(identity, session_id) => {
                self.receipt.identity == *identity
                    && payload
                        == (Request::PrepareSession {
                            session_id: *session_id,
                        })
            }
            Change::Cancel(identity) => self.receipt.identity == *identity && payload == (Request::Cancel {}),
            Change::Resume => true,
        })
    }
    pub fn command(&self) -> Result<&'static str> {
        let request: RecoveryRequest = serde_json::from_str(&self.request).map_err(|_| Error::Invalid)?;
        Ok(
            match serde_json::from_str::<Request>(&request.payload).map_err(|_| Error::Invalid)? {
                Request::Reserve { .. } => "horizon-cloud-worker reserve-project",
                Request::PrepareNamespace {} => "horizon-cloud-worker prepare-project-namespace",
                Request::ImportSource { .. } => "horizon-cloud-worker prepare-project-source",
                Request::ReserveSession { .. } => "horizon-cloud-worker reserve-project-session",
                Request::PrepareSession { .. } => "horizon-cloud-worker prepare-project-session",
                Request::Cancel {} => "horizon-cloud-worker cancel-project-reservation",
            },
        )
    }
}
