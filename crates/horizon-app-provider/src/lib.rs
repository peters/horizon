#![forbid(unsafe_code)]

pub mod api;
pub mod artifact;
pub mod cache;
pub mod tunnel;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("app_artifact_rejected: a declared regular artifact is required within the work root")]
    ArtifactRejected,
    #[error("app_artifact_changed: the artifact changed while it was captured")]
    ArtifactChanged,
    #[error("app_provider_rejected: provider authorization or native response is invalid")]
    ProviderRejected,
    #[error("app_provider_failed: the native provider request failed")]
    ProviderFailed,
    #[error("app_owner_refused: the native resource belongs to another owner")]
    OwnershipRefused,
    #[error("app_reference_expired: the uploaded app handle has expired or was released")]
    AppExpired,
    #[error("app_cache_full: release owned native artifacts before uploading more")]
    CacheFull,
    #[error("app_tunnel_binary_rejected: the configured tunnel binary failed checksum validation")]
    TunnelBinaryRejected,
    #[error("app_tunnel_port_refused: declare a reachable loopback service port")]
    TunnelPortRefused,
    #[error("app_tunnel_start_failed: the restricted native tunnel did not become ready")]
    TunnelStartFailed,
    #[error("app_upload_uncertain: upload outcome must be reconciled before retry")]
    UploadUncertain,
}

pub type Result<T> = std::result::Result<T, Error>;
