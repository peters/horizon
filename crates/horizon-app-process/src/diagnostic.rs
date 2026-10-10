//! Closed host diagnostic vocabulary; caller error text never crosses this boundary.
use serde::{Deserialize, Serialize};
use std::io::ErrorKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum Operation {
    Client,
    Configuration,
    Project,
    Guardian,
    Upload,
    Session,
    Expiry,
    Service,
    Capacity,
    Observation,
    Viewer,
    ViewerCapture,
    Run,
    Evidence,
    Media,
    Output,
    Signal,
    Runtime,
    Mcp,
    Arguments,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum Reason {
    InvalidInput,
    InvalidConfiguration,
    ClientPermissions,
    InvalidSchema,
    InvalidCapture,
    MissingState,
    MissingDeclaration,
    MissingTarget,
    MissingAppIdentifier,
    MissingAllocationReceipt,
    AlreadyRunning,
    WrongGuardianKind,
    LimitExceeded,
    UnsupportedPlatform,
    TaskPanicked,
    TaskCancelled,
    ChannelFull,
    ChannelClosed,
    OutputTimedOut,
    SerializationFailed,
    TransportFailed,
    HandshakeInvalid,
    HandshakeRefused,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum Lock {
    Admission,
    ArchiveAdmission,
    UploadAdmission,
    Uploads,
    Upload,
    Lanes,
    Lane,
    PortClaims,
    Guardian,
    Observations,
    ViewerRun,
    ViewerHistory,
    Viewers,
    ViewerMetadata,
    RunOutput,
    MediaArchive,
    EvidenceRetention,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub enum Cause {
    #[error(transparent)]
    Archive(#[from] HostFailure),
    #[error("{resource:?}: lock poisoned")]
    LockPoisoned { resource: Lock },
    #[error("{operation:?}: {reason:?}")]
    State { operation: Operation, reason: Reason },
    #[error("{operation:?}: I/O {kind:?}", kind = io_kind::normalize(*.kind))]
    Io {
        operation: Operation,
        #[serde(with = "io_kind")]
        kind: ErrorKind,
    },
}

/// Finite archive diagnostics shared with the host; no raw I/O text is retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub enum HostFailure {
    #[error("the retained evidence exceeded its 1 GiB byte limit")]
    EvidenceBytes,
    #[error("the retained evidence exceeded its 1024 file limit")]
    EvidenceFiles,
    #[error("the terminal report exceeded its 8 MiB limit or was already saved")]
    ReportLimit,
    #[error("the capture was empty or exceeded its byte limit")]
    CaptureInvalid,
    #[error("the evidence archive state lock failed")]
    ArchiveState,
    #[error("the evidence archive serialization failed")]
    ArchiveSerialization,
    #[error("the evidence archive write failed ({kind:?})", kind = io_kind::normalize(*.0))]
    ArchiveWrite(#[serde(with = "io_kind")] ErrorKind),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum GuardianReason {
    ParentDisconnected,
    StartupExpired,
    LifetimeExpired,
    ChildFailed,
    ChildExited,
    StartFailed,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub enum Diagnostic {
    Host(Cause),
    Guardian(GuardianReason),
}
impl Diagnostic {
    #[cfg(any(unix, test))]
    pub(crate) fn message(self) -> String {
        match self {
            Self::Host(cause) => format!("app_host_unavailable: {cause}"),
            Self::Guardian(reason) => format!("app_process_failed: Guardian: {reason:?}"),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Retention {
    Recorded,
    AlreadyRecorded,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DiagnosticError {
    #[error("app_diagnostic_invalid: invalid diagnostic budget or private binding")]
    Invalid,
    #[error("app_diagnostic_unavailable: the exact owned log is unavailable")]
    Unavailable,
    #[error("app_diagnostic_timeout: diagnostic retention is unconfirmed")]
    Timeout,
    #[error("app_diagnostic_io: {0:?}")]
    Io(ErrorKind),
}
impl From<std::io::Error> for DiagnosticError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.kind())
    }
}
mod io_kind {
    use super::{Deserialize, ErrorKind};
    use serde::{Deserializer, Serializer};
    pub(super) fn serialize<S: Serializer>(
        kind: impl std::borrow::Borrow<ErrorKind>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{:?}", normalize(*kind.borrow())))
    }
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ErrorKind, D::Error> {
        let value = String::deserialize(deserializer)?;
        from_name(&value).ok_or_else(|| serde::de::Error::custom("unknown I/O kind"))
    }
    pub(super) fn normalize(kind: ErrorKind) -> ErrorKind {
        from_name(&format!("{kind:?}")).unwrap_or(ErrorKind::Other)
    }
    fn from_name(value: &str) -> Option<ErrorKind> {
        macro_rules! kinds { ($($kind:ident),*) => { match value {
            $(stringify!($kind) => Some(ErrorKind::$kind),)*
            _ => None,
        } }; }
        kinds!(
            NotFound,
            PermissionDenied,
            ConnectionRefused,
            ConnectionReset,
            HostUnreachable,
            NetworkUnreachable,
            ConnectionAborted,
            NotConnected,
            AddrInUse,
            AddrNotAvailable,
            NetworkDown,
            BrokenPipe,
            AlreadyExists,
            WouldBlock,
            NotADirectory,
            IsADirectory,
            DirectoryNotEmpty,
            ReadOnlyFilesystem,
            StaleNetworkFileHandle,
            InvalidInput,
            InvalidData,
            TimedOut,
            WriteZero,
            StorageFull,
            NotSeekable,
            QuotaExceeded,
            FileTooLarge,
            ResourceBusy,
            ExecutableFileBusy,
            Deadlock,
            CrossesDevices,
            TooManyLinks,
            InvalidFilename,
            ArgumentListTooLong,
            Interrupted,
            Unsupported,
            UnexpectedEof,
            OutOfMemory,
            Other
        )
    }
}
#[cfg(test)]
mod tests;
