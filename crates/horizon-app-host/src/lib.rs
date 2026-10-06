#![forbid(unsafe_code)]

pub mod actor;
pub mod archive;
pub mod artifacts;
pub mod audit;
pub mod bootstrap;
pub mod cli;
pub mod entry;
pub mod local;
pub mod mcp;
mod observations;
mod project;
pub mod runner;
pub mod services;
pub mod sessions;
pub mod view;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Runtime(#[from] horizon_app_runtime::Error),
    #[error(transparent)]
    Provider(#[from] horizon_app_provider::Error),
    #[error(transparent)]
    Process(#[from] horizon_app_process::Error),
    #[error(transparent)]
    Native(#[from] horizon_app_testing::Error),
    #[error("app_host_unavailable: native host execution is unavailable")]
    Unavailable,
    #[error("app_audit_unavailable: inspect the session before replaying a possibly completed action")]
    AuditUnavailable,
    #[error("app_local_cleanup_uncertain: reconcile the exact private guardian receipt")]
    LocalCleanupUncertain(uuid::Uuid),
    #[error("app_session_unknown: the opaque native session is not owned by this actor")]
    SessionUnknown,
    #[error("app_resource_cleanup_uncertain: reconcile the exact owned native resource")]
    CleanupUncertain,
    #[error("app_artifact_unknown: the opaque native artifact is not owned or has expired")]
    ArtifactUnknown,
    #[error("app_run_cancelled: native matrix execution was cancelled")]
    Cancelled,
    #[error("app_run_busy: a native matrix run already owns this project's build outputs")]
    RunBusy,
    #[error("app_run_failed: inspect the retained per-device report and cleanup outcomes")]
    RunFailed,
    #[error("app_evidence_full: export and explicitly retire retained private reports before starting another run")]
    EvidenceFull,
    #[error("app_media_busy: two evidence downloads are already active; retry within the original deadline")]
    MediaBusy,
    #[error("app_admission_deferred: no native allocation was sent; wait within the original deadline")]
    AdmissionDeferred,
}

pub type Result<T> = std::result::Result<T, Error>;
