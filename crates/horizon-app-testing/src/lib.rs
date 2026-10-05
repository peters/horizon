#![forbid(unsafe_code)]

pub mod catalog;
pub mod contract;
pub mod driver;
pub mod recipe;
pub mod tree;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("device_contract_missing: no remote-device-testing YAML block was found")]
    ContractMissing,
    #[error("device_contract_invalid: the remote-device-testing contract is invalid")]
    ContractInvalid,
    #[error("device_path_rejected: the path must stay inside the selected repository")]
    PathRejected,
    #[error("device_recipe_invalid: the recipe has invalid or unsupported steps")]
    RecipeInvalid,
    #[error("device_catalog_invalid: the provider returned an invalid native-device catalog")]
    CatalogInvalid,
    #[error("device_matrix_unavailable: no offered device matches a matrix entry")]
    MatrixUnavailable,
    #[error("device_file_unavailable: a required project file is unavailable")]
    FileUnavailable,
    #[error("app_source_invalid: the native accessibility source is invalid or exceeds limits")]
    SourceInvalid,
    #[error("app_reference_expired: acquire a fresh native accessibility snapshot")]
    ReferenceExpired,
    #[error("app_target_missing: no native element matches the target")]
    TargetMissing,
    #[error("app_target_ambiguous: more than one native element matches the target")]
    TargetAmbiguous,
    #[error("app_transport_failed: native driver communication failed")]
    TransportFailed,
    #[error("app_allocation_uncertain: native allocation outcome requires reconciliation")]
    AllocationUncertain,
    #[error("app_driver_invalid: native driver returned an invalid response")]
    DriverInvalid,
    #[error("app_session_closed: the native session has been released")]
    SessionClosed,
    #[error("app_reset_requires_reallocation: the owning host must recreate the native session")]
    ResetRequiresReallocation,
    #[error("app_wait_timeout: the native element did not reach the requested state")]
    WaitTimeout,
    #[error("app_assertion_failed: the observed element state did not match")]
    AssertionFailed,
}

pub type Result<T> = std::result::Result<T, Error>;
