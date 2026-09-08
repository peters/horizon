use super::{Outcome, VERSION};
use horizon_core::{
    cloud_run::ArtifactDigest,
    repository_overlay::retained_setup::{SetupCompletion, SetupCompletionState},
};
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Rejected,
    Error,
    Absent,
    ClaimedUnknown,
    Completed,
    RecordingUnconfirmed,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Recording {
    NotAcknowledged,
    Acknowledged,
    Observed,
}

#[derive(Serialize)]
pub(super) struct Response<'a> {
    version: u32,
    status: Status,
    recording: Recording,
    reason: Option<&'a str>,
    execution: Option<Execution<'a>>,
}

#[derive(Serialize)]
struct Execution<'a> {
    state: SetupCompletionState,
    reason: Option<&'a str>,
    source_metadata: Option<&'a Path>,
    checkout: Option<&'a Path>,
    possible_destination: Option<&'a Path>,
    base_commit: Option<&'a str>,
    bundle_manifest: Option<&'a ArtifactDigest>,
}

impl<'a> From<&'a SetupCompletion> for Execution<'a> {
    fn from(receipt: &'a SetupCompletion) -> Self {
        Self {
            state: receipt.state(),
            reason: receipt.reason(),
            source_metadata: receipt.source_metadata(),
            checkout: receipt.checkout(),
            possible_destination: receipt.possible_destination(),
            base_commit: receipt.base_commit(),
            bundle_manifest: receipt.bundle_manifest(),
        }
    }
}

impl<'a> Response<'a> {
    pub(super) fn from_outcome(outcome: &'a Outcome) -> Self {
        let mut response = Self {
            version: VERSION,
            status: Status::Rejected,
            recording: Recording::NotAcknowledged,
            reason: None,
            execution: None,
        };
        match outcome {
            Outcome::Rejected => response.reason = Some("invalid or unsupported setup request"),
            Outcome::Error(reason) => {
                response.status = Status::Error;
                response.reason = Some(reason);
            }
            Outcome::Absent => response.status = Status::Absent,
            Outcome::ClaimedUnknown => response.status = Status::ClaimedUnknown,
            Outcome::Completed { receipt, observed } => {
                response.status = Status::Completed;
                response.recording = if *observed {
                    Recording::Observed
                } else {
                    Recording::Acknowledged
                };
                response.execution = Some(receipt.into());
            }
            Outcome::RecordingUnconfirmed { reason, execution } => {
                response.status = Status::RecordingUnconfirmed;
                response.reason = Some(reason);
                response.execution = execution.as_ref().map(Into::into);
            }
        }
        response
    }

    pub(super) fn exit_code(&self) -> u8 {
        match self.status {
            Status::Absent => 0,
            Status::Rejected => 2,
            Status::ClaimedUnknown => 4,
            Status::Completed => match self.execution.as_ref().map(|receipt| receipt.state) {
                Some(SetupCompletionState::Published) => 0,
                Some(SetupCompletionState::Rejected) => 2,
                _ => 1,
            },
            Status::Error | Status::RecordingUnconfirmed => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::request::RESPONSE_LIMIT;
    use horizon_core::repository_overlay::materialize::MAX_REQUEST_PATH_BYTES;
    use std::path::PathBuf;

    #[test]
    fn maximum_escaped_uncertain_receipt_fits_complete_response() {
        // Synthetic wire-format coverage, not a filesystem or rename claim.
        let root = PathBuf::from(format!("/{}", "\u{1}".repeat(MAX_REQUEST_PATH_BYTES - 1)));
        let metadata = root.join("m".repeat(255));
        let checkout = root.join("c".repeat(255));
        let destination = root.join("d".repeat(255));
        let digest = ArtifactDigest::sha256(b"synthetic");
        let base = "a".repeat(40);
        let response = Response {
            version: VERSION,
            status: Status::RecordingUnconfirmed,
            recording: Recording::NotAcknowledged,
            reason: Some("recording unconfirmed"),
            execution: Some(Execution {
                state: SetupCompletionState::RenameUnconfirmed,
                reason: Some("rename unconfirmed"),
                source_metadata: Some(&metadata),
                checkout: Some(&checkout),
                possible_destination: Some(&destination),
                base_commit: Some(&base),
                bundle_manifest: Some(&digest),
            }),
        };
        let bytes = serde_json::to_vec(&response).unwrap();
        assert!(bytes.len() > 64 * 1024 && bytes.len() < RESPONSE_LIMIT);
        assert_eq!(response.exit_code(), 1);
    }
}
