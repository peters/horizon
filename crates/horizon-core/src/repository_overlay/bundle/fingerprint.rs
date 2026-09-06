use super::super::OverlayChange;
use super::{ArtifactDigest, OverlayBundleError, OverlayContent, RepositoryOverlayPlan};
use serde::Serialize;

/// Fixed versioned field order; plan construction already sorts both layers by path.
#[derive(Serialize)]
struct Manifest<'a> {
    domain: &'static str,
    version: u8,
    repository: &'a str,
    commit: &'a str,
    branch: Option<&'a str>,
    index: Vec<Change<'a>>,
    working_tree: Vec<Change<'a>>,
}

#[derive(Serialize)]
struct Change<'a> {
    path: &'a str,
    #[serde(flatten)]
    content: Content<'a>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Content<'a> {
    Remove,
    File {
        sha256: &'a str,
        bytes: u64,
        executable: bool,
    },
    Symlink {
        target: &'a str,
    },
}

pub(super) fn digest(plan: &RepositoryOverlayPlan) -> Result<ArtifactDigest, OverlayBundleError> {
    Ok(ArtifactDigest::sha256(&encode(plan)?))
}

fn layer(changes: &[OverlayChange]) -> Vec<Change<'_>> {
    changes
        .iter()
        .map(|change| Change {
            path: change.path(),
            content: match change.content() {
                OverlayContent::Remove => Content::Remove,
                OverlayContent::File {
                    sha256,
                    bytes,
                    executable,
                } => Content::File {
                    sha256: sha256.as_str(),
                    bytes: *bytes,
                    executable: *executable,
                },
                OverlayContent::Symlink { target } => Content::Symlink { target },
            },
        })
        .collect()
}

pub(super) fn encode(plan: &RepositoryOverlayPlan) -> Result<Vec<u8>, OverlayBundleError> {
    serde_json::to_vec(&Manifest {
        domain: "horizon.repository-overlay",
        version: 1,
        repository: &plan.source().repository,
        commit: plan.source().commit.as_str(),
        branch: plan.source().branch.as_deref(),
        index: layer(plan.index()),
        working_tree: layer(plan.working_tree()),
    })
    .map_err(|_| OverlayBundleError::Encoding)
}
