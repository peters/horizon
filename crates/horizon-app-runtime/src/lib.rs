#![forbid(unsafe_code)]

pub mod account;
pub mod journal;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("app_credentials_unavailable: check the configured native credentials and credential store")]
    CredentialsUnavailable,
    #[error("app_credentials_invalid: configure a native BrowserStack provider profile")]
    CredentialsInvalid,
    #[error("app_journal_unavailable: private durable native state is unavailable")]
    JournalUnavailable,
    #[error("app_journal_missing: native state has not been initialized")]
    JournalMissing,
    #[error("app_journal_invalid: native state requires manual reconciliation")]
    JournalInvalid,
    #[error("app_owner_refused: native resources belong to another workspace or credential realm")]
    OwnershipRefused,
    #[error("app_capacity_unavailable: native account capacity is reserved or occupied")]
    CapacityUnavailable,
    #[error("app_operation_invalid: the native lifecycle transition is invalid")]
    OperationInvalid,
    #[error("app_operation_expired: reconcile the expired native operation")]
    OperationExpired,
}

pub type Result<T> = std::result::Result<T, Error>;
