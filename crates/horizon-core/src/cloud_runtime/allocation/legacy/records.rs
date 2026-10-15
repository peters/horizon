use super::{AllocationId, ControllerId, Deployment, ProjectIdentity, Records, SharingMode};
use crate::cloud_runtime::{
    Stage,
    state::{ImageReplacement, ReadyHistory, Session, UncommittedImage},
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<Box<crate::cloud_runtime::publishing::Publication>>,
    pub worker: Option<Worker>,
    pub stop_requested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_replacement: Option<ImageReplacement>,
    #[serde(
        default,
        skip_serializing_if = "crate::cloud_runtime::tailnet::receipt::EnrollmentLedger::is_empty"
    )]
    pub tailnet_enrollment: crate::cloud_runtime::tailnet::receipt::EnrollmentLedger,
}

impl Allocation {
    pub const VERSION: u32 = 1;
    const ENROLLMENT_VERSION: u32 = 2;
    const PUBLISHING_VERSION: u32 = 3;

    pub(super) fn record_version(&self) -> u32 {
        if self.publication.is_some() {
            Self::PUBLISHING_VERSION
        } else if self.tailnet_enrollment.is_empty() {
            Self::VERSION
        } else {
            Self::ENROLLMENT_VERSION
        }
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailnet_device: Option<crate::cloud_runtime::tailnet::DeviceName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncommitted_image: Option<UncommittedImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_copy: Option<crate::cloud_runtime::repository::uncommitted::Confirmed>,
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
            publication,
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
            timeline: _,
            last_self_stop: _,
            tailnet_device,
            tailnet_enrollment,
            // `from_legacy` refuses sibling records, including those with enrollment evidence.
            siblings: _,
            uncommitted_image,
            working_copy,
        } = legacy;
        Self {
            allocation: Allocation {
                version: if publication.is_some() {
                    Allocation::PUBLISHING_VERSION
                } else if tailnet_enrollment.is_empty() {
                    Allocation::VERSION
                } else {
                    Allocation::ENROLLMENT_VERSION
                },
                id: allocation,
                controller,
                member: identity.clone(),
                sharing: SharingMode::Dedicated,
                protocol: Protocol::LegacyDedicated,
                operation,
                spec,
                registry_generation,
                publication,
                worker,
                stop_requested,
                image_replacement,
                tailnet_enrollment,
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
                tailnet_device,
                uncommitted_image,
                working_copy,
            },
        }
    }

    /// Reconstruct the legacy runtime view without allocation or session changes.
    #[must_use]
    pub fn deployment(&self) -> Deployment {
        Deployment {
            version: if self.allocation.publication.is_some() {
                crate::cloud_runtime::state::PUBLISHING_RECORD_VERSION
            } else if self.allocation.tailnet_enrollment.is_empty() {
                1
            } else {
                crate::cloud_runtime::state::ENROLLMENT_RECORD_VERSION
            },
            cloud_id: self.project.identity.cloud_id().into(),
            repository: self.project.repository.clone(),
            revision: self.project.revision.clone(),
            profile: self.project.profile.clone(),
            stage: self.project.stage,
            operation: self.allocation.operation.clone(),
            spec: self.allocation.spec.clone(),
            registry_generation: self.allocation.registry_generation.clone(),
            publication: self.allocation.publication.clone(),
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
            // Presentation only: a split record starts without a deployment timeline or the
            // reason an agent last stopped the worker; the next readiness check reports it again.
            timeline: None,
            last_self_stop: None,
            tailnet_device: self.project.tailnet_device.clone(),
            tailnet_enrollment: self.allocation.tailnet_enrollment.clone(),
            siblings: None,
            uncommitted_image: self.project.uncommitted_image.clone(),
            working_copy: self.project.working_copy.clone(),
        }
    }
}
