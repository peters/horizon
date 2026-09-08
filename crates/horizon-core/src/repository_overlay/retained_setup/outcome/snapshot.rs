use crate::cloud_run::ArtifactDigest;
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupCompletionState {
    Rejected,
    Unpublished,
    Published,
    PublishedUnsynchronized,
    RenameUnconfirmed,
}

/// Historical returned execution state, not a current filesystem or task assertion.
#[derive(Clone, Eq, PartialEq)]
pub struct SetupCompletion {
    pub(super) data: CompletionData,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CompletionData {
    pub(super) state: SetupCompletionState,
    pub(super) reason: Option<String>,
    pub(super) source_metadata: Option<PathBuf>,
    pub(super) checkout: Option<PathBuf>,
    pub(super) possible_destination: Option<PathBuf>,
    pub(super) base_commit: Option<String>,
    pub(super) bundle_manifest: Option<ArtifactDigest>,
}

impl fmt::Debug for SetupCompletion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetupCompletion")
            .field("state", &self.data.state)
            .finish_non_exhaustive()
    }
}

impl SetupCompletion {
    #[must_use]
    pub fn state(&self) -> SetupCompletionState {
        self.data.state
    }
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.data.reason.as_deref()
    }
    #[must_use]
    pub fn source_metadata(&self) -> Option<&Path> {
        self.data.source_metadata.as_deref()
    }
    #[must_use]
    pub fn checkout(&self) -> Option<&Path> {
        self.data.checkout.as_deref()
    }
    #[must_use]
    pub fn possible_destination(&self) -> Option<&Path> {
        self.data.possible_destination.as_deref()
    }
    #[must_use]
    pub fn base_commit(&self) -> Option<&str> {
        self.data.base_commit.as_deref()
    }
    #[must_use]
    pub fn bundle_manifest(&self) -> Option<&ArtifactDigest> {
        self.data.bundle_manifest.as_ref()
    }

    /// Project a typed execution result without observing or changing the filesystem.
    /// This does not establish recording, synchronization, liveness or replay authority.
    #[must_use]
    pub fn from_execution(result: &super::SetupMaterializationResult) -> Self {
        use super::super::SetupExecutionError;
        use crate::repository_overlay::{
            checkout::publication::PublicationFailure, materialize::MaterializationProblem,
        };
        let mut snapshot = CompletionData {
            state: SetupCompletionState::Rejected,
            reason: None,
            source_metadata: None,
            checkout: None,
            possible_destination: None,
            base_commit: None,
            bundle_manifest: None,
        };
        match result {
            Ok(repository) => {
                snapshot.state = SetupCompletionState::Published;
                snapshot.source_metadata = Some(repository.source_metadata().to_owned());
                let checkout = repository.checkout();
                snapshot.repository(checkout.path(), checkout.base_commit(), checkout.manifest_sha256());
            }
            Err(error) => {
                snapshot.reason = Some(error.to_string());
                if let SetupExecutionError::Materialization(failure) = error {
                    snapshot.source_metadata = failure.source_metadata().map(Path::to_owned);
                    snapshot.state = SetupCompletionState::Unpublished;
                    match &failure.problem {
                        MaterializationProblem::InvalidRequest => snapshot.state = SetupCompletionState::Rejected,
                        MaterializationProblem::Preparation(failure) => {
                            snapshot.checkout = failure.residue().map(Path::to_owned);
                        }
                        MaterializationProblem::Publication(failure) => match failure.as_ref() {
                            PublicationFailure::Unpublished { checkout: stage, .. } => {
                                snapshot.repository(stage.path(), stage.base_commit(), stage.manifest_sha256());
                            }
                            PublicationFailure::PublishedUnsynchronized { checkout, .. } => {
                                snapshot.state = SetupCompletionState::PublishedUnsynchronized;
                                snapshot.repository(
                                    checkout.path(),
                                    checkout.base_commit(),
                                    checkout.manifest_sha256(),
                                );
                            }
                            PublicationFailure::RenameUnconfirmed {
                                checkout: stage,
                                destination,
                            } => {
                                snapshot.state = SetupCompletionState::RenameUnconfirmed;
                                snapshot.repository(stage.path(), stage.base_commit(), stage.manifest_sha256());
                                snapshot.possible_destination = Some(destination.clone());
                            }
                        },
                        _ => {}
                    }
                }
            }
        }
        Self { data: snapshot }
    }
}

impl CompletionData {
    fn repository(&mut self, path: &Path, base: git2::Oid, manifest: &ArtifactDigest) {
        self.checkout = Some(path.to_owned());
        self.base_commit = Some(base.to_string());
        self.bundle_manifest = Some(manifest.clone());
    }
}
