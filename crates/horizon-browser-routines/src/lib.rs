#![forbid(unsafe_code)]

//! Backend-neutral Teach-mode recording protocol for Horizon browser routines.
//!
//! This crate stores semantic recordings. It has no browser process, MCP, UI,
//! filesystem registry, or durable-runner dependency. See
//! `docs/architecture/browser-routines.md`.

mod assertion;
mod fingerprint;
mod origin;
mod recording;
mod value;

pub use assertion::Assertion;
pub use fingerprint::{FrameContext, TargetCandidate, TargetFingerprint, UniquenessEvidence};
pub use origin::Origin;
pub use recording::{
    MutationClass, NavigationTemplate, QueryComponent, RecordedAction, RecordedKind, SemanticRecording,
};
pub use value::{CredentialFieldKind, FieldClassification, ValueSource};

use thiserror::Error;

/// Accepted `schema_version` for recordings, drafts, and routine files.
pub const SCHEMA_VERSION: u32 = 1;

/// Recoverable protocol failure. Error text never includes secret values.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum RoutineError {
    /// `schema_version` is not [`SCHEMA_VERSION`].
    #[error("unsupported schema version {0}; expected 1")]
    UnsupportedSchema(u32),
    /// JSON could not be decoded, including unknown fields.
    #[error("malformed routine JSON: {0}")]
    Json(String),
    /// Input is not an exact HTTPS origin or loopback HTTP fixture origin.
    #[error("origin is not an exact HTTPS or loopback HTTP origin")]
    InvalidOrigin,
    /// A password-classified field carried a literal or variable.
    #[error("typed secrets cannot be serialized into a routine recording")]
    SecretLiteral,
    /// Identifier is empty, too long, or outside the accepted alphabet.
    #[error("identifier is not a bounded printable name")]
    InvalidIdentifier,
    /// Literal exceeds the engine text bound or contains control characters.
    #[error("literal value is too large")]
    LiteralTooLarge,
    /// Literal contains a control character.
    #[error("literal value contains control characters")]
    LiteralControl,
    /// Credential field kind does not match the control classification.
    #[error("credential field does not match the classified control")]
    CredentialFieldMismatch,
    /// Target fingerprint is missing required durable identity.
    #[error("target fingerprint is missing or malformed")]
    InvalidFingerprint,
    /// Click or fill has no reviewed non-coordinate identity.
    #[error("target has no durable semantic candidate")]
    UndurableTarget,
    /// Assertion text is empty, too large, or still has a query/fragment.
    #[error("assertion is missing, oversized, or still has a query string")]
    InvalidAssertion,
    /// Recording has no actions or more than the accepted bound.
    #[error("recording is empty or exceeds the action bound")]
    InvalidRecording,
    /// Stored URL still has userinfo, query, or fragment.
    #[error("url pattern is not redacted")]
    UnredactedUrl,
    /// Fill is missing a value source.
    #[error("fill action requires a value source")]
    MissingValueSource,
    /// A non-fill action carried a value source.
    #[error("value source is only valid on fill actions")]
    UnexpectedValueSource,
    /// A navigate action is missing a replayable navigation template.
    #[error("navigate action requires a navigation template")]
    MissingNavigation,
    /// Navigation path or query component is malformed or secret-bearing.
    #[error("navigation template is not a replayable non-secret destination")]
    InvalidNavigation,
}

#[cfg(test)]
mod tests {
    use super::{SCHEMA_VERSION, SemanticRecording};

    #[test]
    fn schema_version_is_one() {
        assert_eq!(SCHEMA_VERSION, 1);
        let err = SemanticRecording::from_json(
            r#"{"schema_version":1,"recording_id":"00000000-0000-0000-0000-000000000000","actions":[]}"#,
        );
        assert!(err.is_err());
    }
}
