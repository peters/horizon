use super::*;
use crate::{
    cloud_run::{ArtifactDigest, GitCommitSha, GitSource},
    repository_overlay::{OverlayChange, OverlayContent, RepositoryOverlayPlan},
};

mod malformed;
mod roundtrip;

fn source() -> GitSource {
    GitSource {
        repository: "team/repo".into(),
        commit: GitCommitSha::parse("a".repeat(40)).expect("commit"),
        branch: None,
    }
}

fn file(path: &str, bytes: &[u8], executable: bool) -> OverlayChange {
    OverlayChange::new(
        path.into(),
        OverlayContent::File {
            sha256: ArtifactDigest::sha256(bytes),
            bytes: bytes.len().try_into().expect("size"),
            executable,
        },
    )
    .expect("file")
}

fn bundle(index: Vec<OverlayChange>, working: Vec<OverlayChange>, blobs: &[&[u8]]) -> RepositoryOverlayBundle {
    RepositoryOverlayBundle::new(
        RepositoryOverlayPlan::new(source(), index, working).expect("plan"),
        blobs
            .iter()
            .map(|bytes| VerifiedOverlayBlob::new(bytes.to_vec()).expect("blob")),
    )
    .expect("bundle")
}

fn one_file() -> RepositoryOverlayBundle {
    bundle(
        vec![file("selected", b"literal\0\xff\n", true)],
        vec![],
        &[b"literal\0\xff\n"],
    )
}

fn metadata_end(encoded: &[u8]) -> usize {
    12 + usize::try_from(u32::from_le_bytes(encoded[8..12].try_into().expect("header"))).expect("length")
}

fn frame(metadata: &[u8], count: u32, records: &[u8]) -> Vec<u8> {
    let mut encoded = MAGIC.to_vec();
    encoded.extend_from_slice(&u32::try_from(metadata.len()).expect("metadata length").to_le_bytes());
    encoded.extend_from_slice(metadata);
    encoded.extend_from_slice(&count.to_le_bytes());
    encoded.extend_from_slice(records);
    encoded
}

fn changed_metadata(bundle: &RepositoryOverlayBundle, change: impl FnOnce(String) -> String) -> Vec<u8> {
    let encoded = encode(bundle).expect("encoding");
    let end = metadata_end(&encoded);
    let metadata = change(String::from_utf8(encoded[12..end].to_vec()).expect("metadata"));
    frame(
        metadata.as_bytes(),
        bundle.blobs().len().try_into().expect("count"),
        &encoded[end + 4..],
    )
}
