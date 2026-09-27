use super::{Error, Owner, Request, Result};
use crate::cloud_runtime::{bootstrap_recovery::connection::Binding, project_reservations::journal::Journal};
use horizon_cloud_protocol::{
    ProjectIdentity,
    membership::{MAX_PROJECTS, MAX_SESSIONS, Session},
    session_runtime::SUPPORTED_AGENT,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const KEY: &str = "project_setups";
const MAX_BYTES: usize = 256 * 1024;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Intent {
    pub request: Request,
    pub binding: Binding,
    pub revision: String,
    pub sessions: Vec<Session>,
}
#[derive(PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Registry {
    version: u32,
    pub intents: Vec<Intent>,
}
impl Request {
    pub(super) fn validate(&self) -> Result<()> {
        let digest = self.image_digest.rsplit_once("@sha256:").ok_or(Error::Invalid)?.1;
        if !cfg!(target_os = "linux")
            || self.repository.as_os_str().is_empty()
            || self.repository.as_os_str().len() > 4096
            || self.selection.is_empty()
            || self.selection.len() > 4096
            || !horizon_cloud::valid_image(&self.image_digest)
            || digest.len() != 64
            || !lower_hex(digest)
            || self.agents.is_empty()
            || self.agents.len() > MAX_SESSIONS
            || self.agents.iter().any(|agent| *agent != SUPPORTED_AGENT)
            || self.capabilities.desktop
            || self.capabilities.browser_tools()
            || self.capabilities.agents != self.agents.iter().copied().collect()
        {
            return Err(Error::Invalid);
        }
        self.capabilities.validate().map_err(|_| Error::Invalid)?;
        Ok(())
    }
}
fn lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
impl Registry {
    pub fn load(owner: &Owner) -> Result<Self> {
        let payload = owner.load()?;
        let registry = match payload.get(KEY) {
            None => Self {
                version: 1,
                intents: Vec::new(),
            },
            Some(value) => {
                if serde_json::to_vec(value).map_err(|_| Error::Invalid)?.len() > MAX_BYTES {
                    return Err(Error::Invalid);
                }
                serde_json::from_value(value.clone()).map_err(|_| Error::Invalid)?
            }
        };
        registry.validate(owner)?;
        Ok(registry)
    }
    pub fn save(&self, owner: &mut Owner) -> Result<()> {
        self.validate(owner)?;
        let mut payload = owner.load()?;
        payload
            .as_object_mut()
            .ok_or(Error::Invalid)?
            .insert(KEY.into(), serde_json::to_value(self).map_err(|_| Error::Invalid)?);
        owner.save(payload)?;
        Ok(())
    }
    fn validate(&self, owner: &Owner) -> Result<()> {
        if self.version != 1
            || self.intents.len() > MAX_PROJECTS
            || serde_json::to_vec(self).map_err(|_| Error::Invalid)?.len() > MAX_BYTES
        {
            return Err(Error::Invalid);
        }
        let controller = owner.binding()?;
        let mut projects = BTreeSet::new();
        let mut clouds = BTreeSet::new();
        let mut sessions = BTreeSet::new();
        for intent in &self.intents {
            intent.request.validate()?;
            intent.binding.startup.validate().map_err(|_| Error::Invalid)?;
            if intent.binding.startup.controller != controller
                || !horizon_cloud::valid_id(&intent.binding.worker_id)
                || !intent.request.repository.is_absolute()
                || intent
                    .request
                    .repository
                    .components()
                    .any(|c| matches!(c, std::path::Component::CurDir | std::path::Component::ParentDir))
                || intent.revision.len() != 40
                || !lower_hex(&intent.revision)
                || intent.sessions.len() != intent.request.agents.len()
                || !projects.insert(intent.request.project.project_id())
                || !clouds.insert(intent.request.project.cloud_id())
                || self.intents.first().is_some_and(|first| {
                    first.binding != intent.binding || first.request.image_digest != intent.request.image_digest
                })
            {
                return Err(Error::Invalid);
            }
            for (session, agent) in intent.sessions.iter().zip(&intent.request.agents) {
                if session.id.is_nil()
                    || !sessions.insert(session.id)
                    || &session.agent != agent
                    || session.revision != intent.revision
                {
                    return Err(Error::Invalid);
                }
            }
        }
        Ok(())
    }
    pub fn find(&self, project: &ProjectIdentity) -> Option<&Intent> {
        self.intents.iter().find(|i| &i.request.project == project)
    }
    pub fn require_new(&self, request: &Request, journal: Option<&Journal>) -> Result<()> {
        if self.intents.len() >= MAX_PROJECTS
            || self.intents.iter().any(|i| {
                i.request.project.project_id() == request.project.project_id()
                    || i.request.project.cloud_id() == request.project.cloud_id()
            })
        {
            return Err(Error::Invalid);
        }
        if let Some(journal) = journal {
            if journal.pending.is_some() || journal.generation.is_some() {
                return Err(Error::Blocked);
            }
            if journal.manifest.members.iter().any(|m| {
                m.identity.project_id() == request.project.project_id()
                    || m.identity.cloud_id() == request.project.cloud_id()
            }) {
                return Err(Error::Invalid);
            }
        }
        Ok(())
    }
}
