#![forbid(unsafe_code)]

pub mod actor;
pub mod archive;
pub mod artifacts;
pub mod audit;
pub mod bootstrap;
pub mod cli;
pub mod entry;
pub mod lifecycle;
pub mod local;
pub mod mcp;
mod observations;
mod project;
pub mod runner;
pub mod services;
pub mod sessions;
pub mod view;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
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
    #[error("app_host_unavailable: {0}")]
    LifecycleUnavailable(#[from] lifecycle::Fault),
    #[error("app_host_unavailable: {0}")]
    HostUnavailable(#[from] HostFailure),
    #[error("app_audit_unavailable: inspect the session before replaying a possibly completed action")]
    AuditUnavailable,
    #[error("app_local_cleanup_uncertain: reconcile the exact private guardian receipt")]
    LocalCleanupUncertain(uuid::Uuid),
    #[error("app_session_unknown: the opaque native session is not owned by this actor")]
    SessionUnknown,
    #[error("app_screenshot_requires_capture: use app_screenshot to retain and return capture evidence")]
    ScreenshotRequiresCapture,
    #[error("app_resource_cleanup_uncertain: reconcile the exact owned native resource")]
    CleanupUncertain,
    #[error("{cause}; app_resource_cleanup_uncertain: reconcile the exact owned native resource")]
    CleanupUnconfirmed {
        #[source]
        cause: Box<Error>,
    },
    #[error("app_artifact_unknown: the opaque native artifact is not owned or has expired")]
    ArtifactUnknown,
    #[error("app_run_cancelled: native matrix execution was cancelled")]
    Cancelled,
    #[error("app_run_busy: a native matrix run already owns this project's build outputs")]
    RunBusy,
    #[error("app_run_failed: inspect the retained per-device report and cleanup outcomes")]
    RunFailed,
    #[error("app_evidence_full: reduce requested evidence or export and explicitly retire retained private reports")]
    EvidenceFull,
    #[error("app_media_busy: two evidence downloads are already active; retry within the original deadline")]
    MediaBusy,
    #[error("app_admission_deferred: no native allocation was sent; wait within the original deadline")]
    AdmissionDeferred,
}

impl Error {
    pub(crate) fn with_unconfirmed_cleanup(self) -> Self {
        match self {
            Self::CleanupUncertain | Self::LocalCleanupUncertain(_) | Self::CleanupUnconfirmed { .. } => self,
            cause => Self::CleanupUnconfirmed { cause: Box::new(cause) },
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Public host diagnostics contain typed causes, never paths or application data.
pub use horizon_app_process::diagnostic::HostFailure;
