use super::{
    super::store::{Store, invalid},
    process::Identity,
};
use horizon_cloud_protocol::{
    OperationId, ProjectIdentity,
    membership::{Manifest, Receipt, Request, SessionId},
    session_runtime::Status,
};
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    pub version: u32,
    pub launch: Receipt,
    pub session: SessionId,
    pub nonce: OperationId,
    pub supervisor: Option<Identity>,
    pub agent: Option<Identity>,
    pub status: Status,
}
impl Record {
    pub fn name(id: SessionId) -> String {
        format!("runtime-{id}.json")
    }
    pub fn load(
        store: &Store,
        manifest: &Manifest,
        project: &ProjectIdentity,
        id: SessionId,
    ) -> io::Result<Option<(Self, Vec<u8>)>> {
        let Some(bytes) = store.read(&Self::name(id))? else {
            return Ok(None);
        };
        let record: Self = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if record.version != 1
            || record.session != id
            || &record.launch.identity != project
            || launch(manifest, project, id)? != Some(&record.launch)
            || matches!(record.status, Status::NotStarted)
            || (record.agent.is_some() && record.supervisor.is_none())
            || (matches!(record.status, Status::Running) && record.agent.is_none())
            || (record.status == Status::Stopped
                && !manifest
                    .members
                    .iter()
                    .any(|m| &m.identity == project && m.stops.contains(&id)))
        {
            return Err(invalid());
        }
        Ok(Some((record, bytes)))
    }
    pub fn save(&self, store: &Store, expected: Option<&[u8]>) -> io::Result<()> {
        let name = Self::name(self.session);
        store.write(&name, expected, &serde_json::to_vec(self)?)?;
        store.sync(&name)
    }
    pub fn terminal_barrier(&self, store: &Store, expected: &[u8]) -> io::Result<()> {
        let name = Self::name(self.session);
        store.sync(&name)?;
        if store.read(&name)?.as_deref() != Some(expected) {
            return Err(invalid());
        }
        store.verify()
    }
    pub fn observed(&self) -> Status {
        if self.status == Status::Stopped {
            return Status::Stopped;
        }
        if !self.supervisor.as_ref().is_some_and(Identity::alive) {
            return Status::Uncertain;
        }
        if self.status == Status::Running && !self.agent.as_ref().is_some_and(Identity::alive) {
            return Status::Uncertain;
        }
        self.status.clone()
    }
}
pub(super) fn launch<'a>(
    manifest: &'a Manifest,
    project: &ProjectIdentity,
    id: SessionId,
) -> io::Result<Option<&'a Receipt>> {
    for entry in &manifest.operations {
        if &entry.receipt.identity == project
            && serde_json::from_str::<Request>(&entry.payload).map_err(|_| invalid())?
                == (Request::StartSession { session_id: id })
        {
            return Ok(Some(&entry.receipt));
        }
    }
    Ok(None)
}
