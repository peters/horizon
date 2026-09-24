use super::{AllocationId, ControllerId, Deployment, ProjectIdentity, Records, SharingMode};
use crate::cloud_runtime::{
    Stage,
    state::{ImageReplacement, ReadyHistory, Session},
};
use horizon_cloud::{CreateState, Profile, Worker, WorkerSpec};
use horizon_cloud_protocol::OperationId;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Allocation {
    pub version: u32,
    pub id: AllocationId,
    pub controller: ControllerId,
    pub member: ProjectIdentity,
    pub sharing: SharingMode,
    pub protocol: Protocol,
    pub operation: CreateState,
    pub spec: Option<WorkerSpec>,
    #[serde(default)]
    pub registry_generation: Option<String>,
    pub worker: Option<Worker>,
    pub stop_requested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_replacement: Option<ImageReplacement>,
}

impl Allocation {
    pub const VERSION: u32 = 1;
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Protocol {
    LegacyDedicated,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Project {
    pub version: u32,
    pub allocation: AllocationId,
    pub identity: ProjectIdentity,
    pub repository: PathBuf,
    pub revision: String,
    pub profile: Profile,
    pub stage: Stage,
    pub sessions: Vec<Session>,
    pub source_ready: bool,
    pub ready_after_seconds: Option<u64>,
    pub ready_history: ReadyHistory,
    pub browserstack_released: bool,
    pub browserstack_targets: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_restart: Option<OperationId>,
}

impl Project {
    pub const VERSION: u32 = 2;
}

impl Records {
    pub(super) fn split(
        legacy: Deployment,
        identity: ProjectIdentity,
        allocation: AllocationId,
        controller: ControllerId,
    ) -> Self {
        let Deployment {
            version: _,
            cloud_id: _,
            repository,
            revision,
            profile,
            stage,
            operation,
            spec,
            registry_generation,
            worker,
            sessions,
            source_ready,
            ready_after_seconds,
            ready_history,
            stop_requested,
            browserstack_released,
            browserstack_targets,
            image_replacement,
            session_restart,
        } = legacy;
        Self {
            allocation: Allocation {
                version: Allocation::VERSION,
                id: allocation,
                controller,
                member: identity.clone(),
                sharing: SharingMode::Dedicated,
                protocol: Protocol::LegacyDedicated,
                operation,
                spec,
                registry_generation,
                worker,
                stop_requested,
                image_replacement,
            },
            project: Project {
                version: Project::VERSION,
                allocation,
                identity,
                repository,
                revision,
                profile,
                stage,
                sessions,
                source_ready,
                ready_after_seconds,
                ready_history,
                browserstack_released,
                browserstack_targets,
                session_restart,
            },
        }
    }

    /// Reconstruct the legacy runtime view without allocation or session changes.
    #[must_use]
    pub fn deployment(&self) -> Deployment {
        Deployment {
            version: 1,
            cloud_id: self.project.identity.cloud_id().into(),
            repository: self.project.repository.clone(),
            revision: self.project.revision.clone(),
            profile: self.project.profile.clone(),
            stage: self.project.stage,
            operation: self.allocation.operation.clone(),
            spec: self.allocation.spec.clone(),
            registry_generation: self.allocation.registry_generation.clone(),
            worker: self.allocation.worker.clone(),
            sessions: self.project.sessions.clone(),
            source_ready: self.project.source_ready,
            ready_after_seconds: self.project.ready_after_seconds,
            ready_history: self.project.ready_history,
            stop_requested: self.allocation.stop_requested,
            browserstack_released: self.project.browserstack_released,
            browserstack_targets: self.project.browserstack_targets.clone(),
            image_replacement: self.allocation.image_replacement.clone(),
            session_restart: self.project.session_restart,
        }
    }
}
