//! Workspace volumes. A volume belongs to one location and attaches to one
//! server at a time; it outlives its servers until deleted explicitly.
use super::{Action, Hetzner, OPERATION_LABEL, resource_name, servers::Location, valid_name};
use crate::{Cancellation, CloudError, CreateState, Progress};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

/// Hetzner's volume size limits in GB.
pub const SIZE_GB: std::ops::RangeInclusive<u32> = 10..=10_240;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Volume {
    pub id: u64,
    pub name: String,
    pub size: u32,
    pub location: Location,
    /// The server it is attached to.
    pub server: Option<u64>,
    /// The block device on the attached server, such as `/dev/disk/by-id/scsi-0HC_Volume_1`.
    pub linux_device: String,
    pub status: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}
impl Volume {
    /// # Errors
    /// Prevents attaching or deleting a volume another operation owns.
    pub fn verify(&self, operation_id: &str) -> Result<(), CloudError> {
        if self.name != resource_name(operation_id)?
            || self.labels.get(OPERATION_LABEL).map(String::as_str) != Some(operation_id)
        {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct Single {
    volume: Volume,
}
#[derive(Deserialize)]
struct Created {
    volume: Volume,
    action: Action,
}
#[derive(Deserialize)]
struct Acted {
    action: Action,
}

impl Hetzner {
    /// Creates the operation's ext4 volume unattached, or returns the one it
    /// already made. The fence works as for servers: a Requested operation only
    /// reconciles by label.
    /// # Errors
    /// Reports invalid sizes and locations, ambiguous creation and identity conflicts.
    pub fn ensure_volume(
        &self,
        operation_id: &str,
        location: &str,
        size_gb: u32,
        state: &mut CreateState,
        cancel: &Cancellation,
        mut persist: impl FnMut(&CreateState) -> Result<(), CloudError>,
    ) -> Result<Volume, CloudError> {
        let name = resource_name(operation_id)?;
        if !valid_name(location) || !SIZE_GB.contains(&size_gb) {
            return Err(CloudError::Invalid(
                "A Hetzner volume needs a location and a size from 10 to 10,240 GB",
            ));
        }
        cancel.check()?;
        match state {
            CreateState::Bound { worker_id } => {
                let id = worker_id
                    .parse()
                    .map_err(|_| CloudError::Invalid("Invalid volume ID"))?;
                let volume = self.inspect_volume(id, cancel)?.ok_or(CloudError::WorkerLost)?;
                volume.verify(operation_id)?;
                return Ok(volume);
            }
            CreateState::Terminated { .. } => return Err(CloudError::WorkerLost),
            CreateState::Requested => return self.reconcile_volume(operation_id, state, cancel, &mut persist),
            CreateState::Prepared => {}
        }
        match self.find_volumes(operation_id, cancel)?.len() {
            0 => {}
            1 => return self.reconcile_volume(operation_id, state, cancel, &mut persist),
            _ => return Err(CloudError::DuplicateWorkers),
        }
        persist(&CreateState::Requested)?;
        *state = CreateState::Requested;
        let body = json!({
            "name": name, "size": size_gb, "location": location, "format": "ext4",
            "labels": {OPERATION_LABEL: operation_id},
        });
        let created: Created = match self.send("POST", "/volumes", Some(body), cancel) {
            Ok(value) => serde_json::from_value(value).map_err(|_| CloudError::CreationUnresolved)?,
            Err(failure) if failure.name_taken() => {
                return self.reconcile_volume(operation_id, state, cancel, &mut persist);
            }
            Err(failure) if failure.definite() || failure.capacity() => {
                persist(&CreateState::Prepared)?;
                *state = CreateState::Prepared;
                return Err(failure.into());
            }
            Err(failure) => return Err(failure.into()),
        };
        created.volume.verify(operation_id)?;
        bind(state, &created.volume, &mut persist)?;
        self.wait(&created.action, cancel)?;
        self.inspect_volume(created.volume.id, cancel)?
            .ok_or(CloudError::WorkerLost)
    }

    /// # Errors
    /// Returns transport, authentication or response errors. HTTP 404 is a missing volume.
    pub fn inspect_volume(&self, id: u64, cancel: &Cancellation) -> Result<Option<Volume>, CloudError> {
        match self.send("GET", &format!("/volumes/{id}"), None, cancel) {
            Err(failure) if failure.not_found() => Ok(None),
            result => {
                let single: Single = serde_json::from_value(result?).map_err(|_| CloudError::InvalidResponse)?;
                if single.volume.id != id {
                    return Err(CloudError::IdentityMismatch);
                }
                Ok(Some(single.volume))
            }
        }
    }

    /// Volumes carrying the operation's label.
    /// # Errors
    /// Refuses invalid operation IDs and reports provider failures.
    pub fn find_volumes(&self, operation_id: &str, cancel: &Cancellation) -> Result<Vec<Volume>, CloudError> {
        resource_name(operation_id)?;
        Ok(self.list_all(
            "/volumes",
            &format!("label_selector={OPERATION_LABEL}%3D{operation_id}"),
            "volumes",
            cancel,
        )?)
    }

    /// Attaches the operation's volume to the operation's server without mounting it.
    /// # Errors
    /// Refuses other operations' resources, other locations and volumes attached elsewhere.
    pub fn attach(
        &self,
        operation_id: &str,
        volume_id: u64,
        server_id: u64,
        cancel: &Cancellation,
    ) -> Result<(), CloudError> {
        let volume = self.inspect_volume(volume_id, cancel)?.ok_or(CloudError::WorkerLost)?;
        volume.verify(operation_id)?;
        let server = self.inspect_server(server_id, cancel)?.ok_or(CloudError::WorkerLost)?;
        server.verify(operation_id)?;
        if server.location != volume.location {
            return Err(CloudError::Invalid(
                "A server can only attach a volume from its own location",
            ));
        }
        match volume.server {
            Some(attached) if attached == server_id => return Ok(()),
            Some(_) => return Err(CloudError::Invalid("The volume is attached to another server")),
            None => {}
        }
        let body = json!({"server": server_id, "automount": false});
        self.volume_action(volume_id, "attach", Some(body), cancel)
    }

    /// Detaches the operation's volume from whichever server holds it.
    /// # Errors
    /// Refuses other operations' volumes and reports provider failures.
    pub fn detach(&self, operation_id: &str, volume_id: u64, cancel: &Cancellation) -> Result<(), CloudError> {
        let volume = self.inspect_volume(volume_id, cancel)?.ok_or(CloudError::WorkerLost)?;
        volume.verify(operation_id)?;
        if volume.server.is_none() {
            return Ok(());
        }
        self.volume_action(volume_id, "detach", None, cancel)
    }

    /// Deletes only the operation's recorded volume, once no server holds it, and proves it is gone.
    /// # Errors
    /// Refuses unbound, mismatching and attached volumes and reports unfinished deletion.
    pub fn delete_volume(
        &self,
        operation_id: &str,
        state: &mut CreateState,
        cancel: &Cancellation,
        mut persist: impl FnMut(&CreateState) -> Result<(), CloudError>,
        mut progress: impl FnMut(Progress),
    ) -> Result<(), CloudError> {
        let recorded = match state {
            CreateState::Bound { worker_id } | CreateState::Terminated { worker_id } => worker_id.clone(),
            _ => return Err(CloudError::CreationUnresolved),
        };
        let id: u64 = recorded.parse().map_err(|_| CloudError::Invalid("Invalid volume ID"))?;
        progress(Progress::ConfirmingVolume);
        if let Some(volume) = self.inspect_volume(id, cancel)? {
            volume.verify(operation_id)?;
            if volume.server.is_some() {
                return Err(CloudError::Invalid(
                    "The volume is still attached; delete or detach its server first",
                ));
            }
            progress(Progress::DeletingVolume);
            match self.send("DELETE", &format!("/volumes/{id}"), None, cancel) {
                Ok(_) => {}
                Err(failure) if failure.not_found() => {}
                Err(failure) => return Err(failure.into()),
            }
            progress(Progress::ConfirmingVolumeDeletion);
            if self.inspect_volume(id, cancel)?.is_some() {
                return Err(CloudError::Invalid("Volume deletion pending; reconcile again"));
            }
        }
        let next = CreateState::Terminated { worker_id: recorded };
        persist(&next)?;
        *state = next;
        Ok(())
    }

    fn volume_action(
        &self,
        id: u64,
        action: &str,
        body: Option<serde_json::Value>,
        cancel: &Cancellation,
    ) -> Result<(), CloudError> {
        let value = self.send("POST", &format!("/volumes/{id}/actions/{action}"), body, cancel)?;
        let acted: Acted = serde_json::from_value(value).map_err(|_| CloudError::InvalidResponse)?;
        self.wait(&acted.action, cancel)
    }

    fn reconcile_volume(
        &self,
        operation_id: &str,
        state: &mut CreateState,
        cancel: &Cancellation,
        persist: &mut impl FnMut(&CreateState) -> Result<(), CloudError>,
    ) -> Result<Volume, CloudError> {
        let mut found = self.find_volumes(operation_id, cancel)?;
        match found.len() {
            0 => Err(CloudError::CreationUnresolved),
            1 => {
                let volume = found.remove(0);
                volume.verify(operation_id)?;
                bind(state, &volume, persist)?;
                Ok(volume)
            }
            _ => Err(CloudError::DuplicateWorkers),
        }
    }
}

fn bind(
    state: &mut CreateState,
    volume: &Volume,
    persist: &mut impl FnMut(&CreateState) -> Result<(), CloudError>,
) -> Result<(), CloudError> {
    let next = CreateState::Bound {
        worker_id: volume.id.to_string(),
    };
    persist(&next)?;
    *state = next;
    Ok(())
}
