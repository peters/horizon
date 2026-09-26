//! Dedicated workspace storage with a durable fence around every allocation.
use super::{RunPod, flavors::Flavor, json};
use crate::{Cancellation, CloudError, NetworkVolume, Progress, Worker, WorkerSpec, valid_id};
use serde::{Deserialize, Serialize};
use std::time::Duration;

type Result<T> = std::result::Result<T, CloudError>;

pub(crate) const REQUEST_SIZE_GB: std::ops::RangeInclusive<u32> = 10..=4000;
pub(crate) const INVALID_REQUEST_SIZE: &str = "CPU workspace volume must be between 10 and 4000 GB";

pub(crate) fn validate_request_size(size: u32) -> Result<()> {
    if !REQUEST_SIZE_GB.contains(&size) {
        return Err(CloudError::Invalid(INVALID_REQUEST_SIZE));
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Spec {
    pub operation_id: String,
    pub size: u32,
    pub data_center_id: String,
}
impl Spec {
    #[must_use]
    pub fn name(&self) -> String {
        format!("horizon-volume-{}", self.operation_id)
    }
    fn validate(&self) -> Result<()> {
        if !valid_id(&self.operation_id) || !valid_id(&self.data_center_id) || !(1..=4000).contains(&self.size) {
            return Err(CloudError::Invalid("Invalid workspace volume specification"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub id: String,
    pub name: String,
    pub size: u32,
    #[serde(alias = "dataCenter")]
    pub data_center_id: String,
    /// Absent only in legacy durable records; v2 responses must include a tier.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub tier: Option<Tier>,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Tier {
    Standard,
    HighPerformance,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Workspace,
    Cleanup,
}
impl Volume {
    fn response(value: serde_json::Value) -> Result<Self> {
        let volume: Self = serde_json::from_value(value).map_err(|_| CloudError::InvalidResponse)?;
        if volume.tier.is_none() {
            return Err(CloudError::InvalidResponse);
        }
        Ok(volume)
    }
    fn require_standard(&self) -> Result<()> {
        if self.tier != Some(Tier::Standard) {
            return Err(CloudError::Invalid(
                "New workspace storage requires the STANDARD volume tier",
            ));
        }
        Ok(())
    }
    fn verify_recorded_tier(&self, recorded: &Self) -> Result<()> {
        if recorded.tier.is_some() && self.tier != recorded.tier {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(())
    }
    /// # Errors
    /// Refuses a different identity, name, capacity or storage location.
    pub fn verify(&self, spec: &Spec) -> Result<()> {
        spec.validate()?;
        if !valid_id(&self.id)
            || self.name != spec.name()
            || self.size != spec.size
            || self.data_center_id != spec.data_center_id
        {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(())
    }
    pub(crate) fn verify_worker_spec(&self, spec: &WorkerSpec) -> Result<()> {
        if self.tier.is_some() {
            self.require_standard()?;
        }
        let expected = Spec {
            operation_id: spec.operation_id.clone(),
            size: u32::from(spec.profile.storage.volume_gb),
            data_center_id: self.data_center_id.clone(),
        };
        self.verify(&expected)?;
        if spec.profile.gpu || (!spec.data_centers.is_empty() && !spec.data_centers.contains(&self.data_center_id)) {
            return Err(CloudError::Invalid(
                "Workspace volume does not match the worker placement",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    #[default]
    Prepared,
    Requested,
    Bound {
        volume: Volume,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        creation: Option<CreationReceipt>,
    },
    Deleting {
        volume: Volume,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        creation: Option<CreationReceipt>,
    },
    Deleted,
}

/// Evidence of a verified direct creation response, persisted with the binding.
/// This is not proof of current storage freshness or permission to initialize it.
/// Consumers must retain their own anchored intent, mount checks and one-shot
/// bootstrap fence. Inspection and lost-response reconciliation never mint this.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreationReceipt {
    version: u32,
    spec: Spec,
    volume: Volume,
}

impl State {
    /// # Errors
    /// Rejects a binding or creation receipt that differs from the original specification.
    pub fn verify(&self, spec: &Spec) -> Result<()> {
        spec.validate()?;
        if let Self::Bound { volume, creation } | Self::Deleting { volume, creation } = self {
            volume.verify(spec)?;
            if let Some(receipt) = creation
                && (receipt.version != 1 || receipt.spec != *spec || receipt.volume != *volume)
            {
                return Err(CloudError::IdentityMismatch);
            }
        }
        Ok(())
    }

    /// Returns recorded direct-create evidence only for an intact bound volume.
    /// Absence on legacy or reconciled records must never be inferred away.
    /// # Errors
    /// Rejects changed specifications, identities and unsupported receipt versions.
    pub fn creation_receipt(&self, spec: &Spec) -> Result<Option<&CreationReceipt>> {
        self.verify(spec)?;
        Ok(match self {
            Self::Bound { creation, .. } => creation.as_ref(),
            _ => None,
        })
    }
}
impl RunPod {
    /// Selects a data center with the worker's exact CPU size in stock and standard
    /// network storage, honoring placement preferences.
    /// # Errors
    /// Refuses missing or unknown capacity rather than allocating storage in an arbitrary location.
    pub fn workspace_volume_spec(&self, worker: &WorkerSpec, cancel: &Cancellation) -> Result<Spec> {
        worker.validate()?;
        if worker.profile.gpu {
            return Err(CloudError::Invalid(
                "Automatic network storage is only used for CPU workers",
            ));
        }
        validate_request_size(u32::from(worker.profile.storage.volume_gb))?;
        let url = format!(
            "{}/datacenters?include=CPU_AVAILABILITY&networkVolumeTypes=STANDARD",
            self.catalog_endpoint
        );
        let catalog: Catalog = serde_json::from_value(self.request_url("GET", &url, None, cancel, None)?)
            .map_err(|_| CloudError::InvalidResponse)?;
        let candidates = candidates(catalog, &worker.data_centers, &worker.cpu_flavors);
        let centers: Vec<String> = candidates.iter().map(|(_, id)| id.clone()).collect();
        let flavors: Vec<&Flavor> = worker.cpu_flavors.iter().filter_map(|id| Flavor::get(id)).collect();
        let stock = self.cpu_stock(&centers, &flavors, worker.profile.cpu, cancel)?;
        let data_center_id = candidates
            .into_iter()
            .filter_map(|(preference, id)| Some((preference, *stock.get(&id)?, id)))
            .min()
            .map(|(_, _, id)| id)
            .ok_or(CloudError::Invalid(
                "No allowed data center has this CPU size in stock with standard workspace storage",
            ))?;
        let spec = Spec {
            operation_id: worker.operation_id.clone(),
            size: u32::from(worker.profile.storage.volume_gb),
            data_center_id,
        };
        spec.validate()?;
        Ok(spec)
    }

    /// Caller holds the cloud operation lock and persists every transition before returning.
    /// # Errors
    /// An uncertain POST can only reconcile; empty listings never authorize another allocation.
    pub fn ensure_volume(
        &self,
        spec: &Spec,
        state: &mut State,
        cancel: &Cancellation,
        persist: impl FnMut(&State) -> Result<()>,
    ) -> Result<Volume> {
        self.ensure_volume_for(spec, state, cancel, persist, Purpose::Workspace)
    }

    fn ensure_volume_for(
        &self,
        spec: &Spec,
        state: &mut State,
        cancel: &Cancellation,
        mut persist: impl FnMut(&State) -> Result<()>,
        purpose: Purpose,
    ) -> Result<Volume> {
        state.verify(spec)?;
        cancel.check()?;
        match state {
            State::Bound { volume, .. } => {
                volume.verify(spec)?;
                let mut current = self.inspect_volume(&volume.id, cancel)?.ok_or(CloudError::Invalid(
                    "Workspace volume is missing; replacement is not automatic",
                ))?;
                current.verify(spec)?;
                current.verify_recorded_tier(volume)?;
                if purpose == Purpose::Workspace && volume.tier.is_some() {
                    current.require_standard()?;
                }
                // Observing a legacy binding cannot rewrite its durable receipt.
                current.tier = volume.tier;
                return Ok(current);
            }
            State::Deleting { .. } | State::Deleted => {
                return Err(CloudError::Invalid(
                    "Workspace volume deletion was requested; finish cleanup before creating a new cloud",
                ));
            }
            // New limits must not prevent recovery or deletion of saved allocations.
            State::Prepared => {
                validate_request_size(spec.size)?;
                self.require_no_serverless_endpoints(cancel)?;
            }
            State::Requested => {}
        }
        let matches: Vec<_> = self
            .list_volumes(cancel)?
            .into_iter()
            .filter(|volume| volume.name == spec.name())
            .collect();
        if matches.len() > 1 || (*state == State::Prepared && !matches.is_empty()) {
            return Err(CloudError::Invalid(
                "Workspace volume identity conflicts with existing storage; no volume was adopted",
            ));
        }
        if let Some(volume) = matches.into_iter().next() {
            volume.verify(spec)?;
            transition(
                state,
                State::Bound {
                    volume: volume.clone(),
                    creation: None,
                },
                &mut persist,
            )?;
            if purpose == Purpose::Workspace {
                volume.require_standard()?;
            }
            return Ok(volume);
        }
        if *state == State::Requested {
            return Err(CloudError::Invalid(
                "Workspace volume allocation is unresolved; an empty listing cannot authorize another request",
            ));
        }
        transition(state, State::Requested, &mut persist)?;
        let response = self.request(
            "POST",
            "/network-volumes",
            Some(json!({"name":spec.name(),"size":spec.size,"dataCenter":spec.data_center_id,"type":"STANDARD"})),
            cancel,
        );
        let value = match response {
            Err(error @ (CloudError::Unauthorized | CloudError::Rejected(_) | CloudError::Cancelled)) => {
                transition(state, State::Prepared, &mut persist)?;
                return Err(error);
            }
            result => result?,
        };
        let volume = Volume::response(value).map_err(|_| CloudError::CreationUnresolved)?;
        volume.verify(spec)?;
        let creation = (volume.tier == Some(Tier::Standard)).then(|| CreationReceipt {
            version: 1,
            spec: spec.clone(),
            volume: volume.clone(),
        });
        transition(
            state,
            State::Bound {
                volume: volume.clone(),
                creation,
            },
            &mut persist,
        )?;
        volume.require_standard()?;
        Ok(volume)
    }

    /// Deletes only recorded, identity-verified storage after every attached worker is absent.
    /// # Errors
    /// Preserves deletion intent across errors and refuses uncertain or still-attached storage.
    pub fn terminate_volume(
        &self,
        spec: &Spec,
        state: &mut State,
        cancel: &Cancellation,
        persist: impl FnMut(&State) -> Result<()>,
    ) -> Result<()> {
        self.terminate_volume_with_progress(spec, state, cancel, persist, |_| {})
    }

    /// As `terminate_volume`, naming each provider request in `progress` before it is sent.
    /// # Errors
    /// As `terminate_volume`.
    pub fn terminate_volume_with_progress(
        &self,
        spec: &Spec,
        state: &mut State,
        cancel: &Cancellation,
        mut persist: impl FnMut(&State) -> Result<()>,
        mut progress: impl FnMut(Progress),
    ) -> Result<()> {
        state.verify(spec)?;
        cancel.check()?;
        if *state == State::Requested {
            // Reconciling a requested volume already reads the provider.
            progress(Progress::ConfirmingVolume);
            self.ensure_volume_for(spec, state, cancel, &mut persist, Purpose::Cleanup)?;
        }
        let (volume, creation) = match state {
            State::Prepared => return transition(state, State::Deleted, &mut persist),
            State::Deleted => return Ok(()),
            State::Bound { volume, creation } | State::Deleting { volume, creation } => {
                (volume.clone(), creation.clone())
            }
            State::Requested => return Err(CloudError::CreationUnresolved),
        };
        volume.verify(spec)?;
        progress(Progress::ConfirmingVolume);
        if let Some(current) = self.inspect_volume(&volume.id, cancel)? {
            current.verify(spec)?;
            current.verify_recorded_tier(&volume)?;
            if self.volume_attached(&volume.id, cancel, &mut progress)? {
                return Err(CloudError::Invalid(
                    "Workspace volume is still attached to a worker; storage was not deleted",
                ));
            }
            transition(
                state,
                State::Deleting {
                    volume: volume.clone(),
                    creation,
                },
                &mut persist,
            )?;
            progress(Progress::DeletingVolume);
            match self.request("DELETE", &format!("/network-volumes/{}", volume.id), None, cancel) {
                Ok(_) | Err(CloudError::Http(404, _)) => {}
                Err(error) => return Err(error),
            }
            progress(Progress::ConfirmingVolumeDeletion);
            if self.inspect_volume(&volume.id, cancel)?.is_some() {
                return Err(CloudError::Invalid(
                    "Workspace volume deletion is pending; reconcile cleanup again",
                ));
            }
        }
        transition(state, State::Deleted, &mut persist)
    }

    /// Confirms the current CPU mount identity before source transfer.
    /// # Errors
    /// Refuses missing, extra, misplaced or differently owned mounts before source transfer.
    pub fn confirm_workspace_mount(
        &self,
        worker: &mut Worker,
        volume: &Volume,
        cancel: &Cancellation,
        timeout: Duration,
    ) -> Result<()> {
        if self.mounted_image(worker, volume, cancel, timeout)? != worker.image_name {
            return Err(CloudError::IdentityMismatch);
        }
        worker.network_volume = Some(NetworkVolume {
            id: Some(volume.id.clone()),
            size: Some(volume.size),
            data_center_id: Some(volume.data_center_id.clone()),
        });
        Ok(())
    }

    /// The image the current API reports for `worker` with exactly `volume` at `/workspace`.
    pub(super) fn mounted_image(
        &self,
        worker: &Worker,
        volume: &Volume,
        cancel: &Cancellation,
        timeout: Duration,
    ) -> Result<String> {
        if !valid_id(&worker.id) {
            return Err(CloudError::IdentityMismatch);
        }
        let url = format!("{}/pods/{}", self.api_endpoint, worker.id);
        let current: MountedWorker =
            serde_json::from_value(self.request_url("GET", &url, None, cancel, Some(timeout))?)
                .map_err(|_| CloudError::InvalidResponse)?;
        if current.id != worker.id
            || current.name != worker.name
            || current.data_center_id != volume.data_center_id
            || current.mounts.network.len() != 1
            || current.mounts.persistent.is_some()
            || current.mounts.network[0].volume_id != volume.id
            || current.mounts.network[0].path != "/workspace"
        {
            return Err(CloudError::IdentityMismatch);
        }
        Ok(current.image)
    }

    fn volume_attached(&self, id: &str, cancel: &Cancellation, progress: &mut impl FnMut(Progress)) -> Result<bool> {
        self.volume_attached_except(id, None, cancel, progress)
    }

    pub(super) fn volume_attached_except(
        &self,
        id: &str,
        allowed: Option<&str>,
        cancel: &Cancellation,
        progress: &mut impl FnMut(Progress),
    ) -> Result<bool> {
        progress(Progress::CheckingAttachments);
        self.require_no_serverless_endpoints(cancel)?;
        let workers = self.list(cancel)?;
        let count = workers.len();
        for (index, worker) in workers.into_iter().enumerate() {
            if allowed == Some(worker.id.as_str()) {
                continue;
            }
            if worker.network_volume.as_ref().is_some_and(|attached| {
                attached
                    .id
                    .as_ref()
                    .is_none_or(|attached_id| !valid_id(attached_id) || attached_id == id)
            }) {
                return Ok(true);
            }
            if !valid_id(&worker.id) {
                return Err(CloudError::InvalidResponse);
            }
            progress(Progress::InspectingMounts {
                worker: index + 1,
                workers: count,
            });
            let url = format!("{}/pods/{}", self.api_endpoint, worker.id);
            let value = match self.request_url("GET", &url, None, cancel, None) {
                Err(CloudError::Http(404, _)) => {
                    if self.inspect(&worker.id, cancel)?.is_some() {
                        return Err(CloudError::Invalid(
                            "Worker mount inspection is unavailable; retry storage cleanup",
                        ));
                    }
                    continue;
                }
                result => result?,
            };
            let current: MountedWorker = serde_json::from_value(value).map_err(|_| CloudError::InvalidResponse)?;
            if current.id != worker.id {
                return Err(CloudError::IdentityMismatch);
            }
            for mount in current.mounts.network {
                if !valid_id(&mount.volume_id) {
                    return Err(CloudError::InvalidResponse);
                }
                if mount.volume_id == id {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn require_no_serverless_endpoints(&self, cancel: &Cancellation) -> Result<()> {
        // v2 exposes only active serverless workers without their mounts. Even
        // an unrelated endpoint can retain older releases with other attachments.
        if !self.pages("/serverless", "endpoints", cancel)?.is_empty() {
            return Err(CloudError::Invalid(
                "Cannot prove workspace storage is detached while serverless endpoints exist; the provider API does not expose all worker mounts",
            ));
        }
        Ok(())
    }

    fn list_volumes(&self, cancel: &Cancellation) -> Result<Vec<Volume>> {
        self.request("GET", "/network-volumes", None, cancel)?
            .get("networkVolumes")
            .and_then(serde_json::Value::as_array)
            .ok_or(CloudError::InvalidResponse)?
            .iter()
            .cloned()
            .map(Volume::response)
            .collect()
    }
    fn inspect_volume(&self, id: &str, cancel: &Cancellation) -> Result<Option<Volume>> {
        if !valid_id(id) {
            return Err(CloudError::Invalid("Invalid workspace volume ID"));
        }
        match self.request("GET", &format!("/network-volumes/{id}"), None, cancel) {
            Err(CloudError::Http(404, _)) => Ok(None),
            result => {
                let volume = Volume::response(result?)?;
                if volume.id != id {
                    return Err(CloudError::IdentityMismatch);
                }
                Ok(Some(volume))
            }
        }
    }
}
fn transition(state: &mut State, next: State, persist: &mut impl FnMut(&State) -> Result<()>) -> Result<()> {
    persist(&next)?;
    *state = next;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Catalog {
    pub(super) data_centers: Vec<Center>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Center {
    pub(super) id: String,
    #[serde(default)]
    network_volume_types: Vec<String>,
    #[serde(default)]
    cpu_availability: Vec<Capacity>,
    #[serde(default)]
    pub(super) gpu_availability: Vec<Capacity>,
}
#[derive(Deserialize)]
pub(super) struct Capacity {
    pub(super) id: String,
    pub(super) availability: String,
}
/// Configured data centers with standard storage whose flavor family reports
/// capacity, with their preference rank. Family capacity only narrows the stock query.
pub(super) fn candidates(catalog: Catalog, data_centers: &[String], cpu_flavors: &[String]) -> Vec<(usize, String)> {
    catalog
        .data_centers
        .into_iter()
        .filter_map(|center| {
            if !valid_id(&center.id) || !center.network_volume_types.iter().any(|tier| tier == "STANDARD") {
                return None;
            }
            let preference = if data_centers.is_empty() {
                0
            } else {
                data_centers.iter().position(|id| id == &center.id)?
            };
            center
                .cpu_availability
                .iter()
                .any(|cpu| {
                    cpu_flavors.contains(&cpu.id) && matches!(cpu.availability.as_str(), "HIGH" | "MEDIUM" | "LOW")
                })
                .then_some((preference, center.id))
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MountedWorker {
    id: String,
    name: String,
    image: String,
    data_center_id: String,
    mounts: Mounts,
}
#[derive(Deserialize)]
struct Mounts {
    #[serde(default)]
    network: Vec<Mount>,
    persistent: Option<serde_json::Value>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mount {
    volume_id: String,
    path: String,
}
