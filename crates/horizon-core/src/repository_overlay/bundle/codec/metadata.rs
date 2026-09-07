use super::{MAX_CHANGES, OverlayCodecError, fingerprint};
use crate::{
    cloud_run::{ArtifactDigest, GitCommitSha, GitSource},
    repository_overlay::{OverlayChange, OverlayContent, RepositoryOverlayPlan},
};
use serde::{
    Deserialize, Deserializer,
    de::{self, IgnoredAny, SeqAccess, Visitor},
};
use std::fmt;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    domain: String,
    version: u8,
    repository: String,
    commit: GitCommitSha,
    branch: Option<String>,
    #[serde(deserialize_with = "bounded_layer")]
    index: Vec<Change>,
    #[serde(deserialize_with = "bounded_layer")]
    working_tree: Vec<Change>,
}

// A flat checked DTO avoids buffered enum/flatten deserialization of unrecognized fields.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    path: String,
    kind: Kind,
    sha256: Option<ArtifactDigest>,
    bytes: Option<u64>,
    executable: Option<bool>,
    target: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Remove,
    File,
    Symlink,
}

impl Change {
    fn validate(self) -> Result<OverlayChange, OverlayCodecError> {
        let content = match (self.kind, self.sha256, self.bytes, self.executable, self.target) {
            (Kind::Remove, None, None, None, None) => OverlayContent::Remove,
            (Kind::File, Some(sha256), Some(bytes), Some(executable), None) => OverlayContent::File {
                sha256,
                bytes,
                executable,
            },
            (Kind::Symlink, None, None, None, Some(target)) => OverlayContent::Symlink { target },
            _ => return Err(OverlayCodecError::Malformed),
        };
        Ok(OverlayChange::new(self.path, content)?)
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<RepositoryOverlayPlan, OverlayCodecError> {
    let metadata: Metadata = serde_json::from_slice(bytes).map_err(|_| OverlayCodecError::Malformed)?;
    if metadata.domain != "horizon.repository-overlay" || metadata.version != 1 {
        return Err(OverlayCodecError::Unsupported);
    }
    let plan = RepositoryOverlayPlan::new(
        GitSource {
            repository: metadata.repository,
            commit: metadata.commit,
            branch: metadata.branch,
        },
        metadata
            .index
            .into_iter()
            .map(Change::validate)
            .collect::<Result<Vec<_>, _>>()?,
        metadata
            .working_tree
            .into_iter()
            .map(Change::validate)
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    if fingerprint::encode(&plan)? != bytes {
        return Err(OverlayCodecError::NonCanonical);
    }
    Ok(plan)
}

fn bounded_layer<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Change>, D::Error> {
    struct LayerVisitor;
    impl<'de> Visitor<'de> for LayerVisitor {
        type Value = Vec<Change>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a bounded overlay layer")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
            let mut changes = Vec::new();
            while changes.len() < MAX_CHANGES {
                let Some(change) = sequence.next_element()? else {
                    return Ok(changes);
                };
                changes.push(change);
            }
            if sequence.next_element::<IgnoredAny>()?.is_some() {
                return Err(de::Error::custom("overlay layer exceeds its change limit"));
            }
            Ok(changes)
        }
    }
    deserializer.deserialize_seq(LayerVisitor)
}
