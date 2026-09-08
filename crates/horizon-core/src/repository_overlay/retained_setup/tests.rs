use super::*;
use crate::cloud_run::ArtifactDigest;

pub(super) fn intent() -> SetupIntent {
    let root = std::env::temp_dir();
    SetupIntent::new(
        "workspace_1".into(),
        root.join("source-objects"),
        root.join("bundle-store"),
        ArtifactDigest::sha256(b"manifest"),
        "repository".into(),
    )
    .unwrap()
}

#[test]
fn strict_bounded_record_round_trip_and_redacted_debug() {
    let expected = intent();
    let bytes = codec::encode(&expected).unwrap();
    assert_eq!(codec::decode(&bytes).unwrap(), expected);
    assert!(bytes.len() < codec::MAX_RECORD_BYTES);
    assert_eq!(format!("{expected:?}"), "SetupIntent { .. }");
    for error in [
        SetupClaimError::InvalidIntent,
        SetupClaimError::Unsupported,
        SetupClaimError::UnsafeRoot,
        SetupClaimError::Read,
        SetupClaimError::InvalidRecord,
        SetupClaimError::Conflict,
        SetupClaimError::Storage,
    ] {
        assert!(!format!("{error:?} {error}").contains("source-objects"));
    }
}

#[test]
fn corrupt_newer_duplicate_unknown_and_noncanonical_records_are_rejected() {
    let valid = String::from_utf8(codec::encode(&intent()).unwrap()).unwrap();
    for invalid in [
        String::new(),
        "{}".into(),
        format!("{valid}\n"),
        valid.replace("\"version\":1", "\"version\":2"),
        valid.replace("\"version\":1", "\"version\":1,\"version\":1"),
        valid.replace("\"version\":1", "\"version\":1,\"extra\":0"),
        valid.replace("workspace_1", "../bad"),
        valid.replace("repository", "../bad"),
        valid[..valid.len() - 1].to_owned(),
        "x".repeat(codec::MAX_RECORD_BYTES + 1),
    ] {
        assert_eq!(codec::decode(invalid.as_bytes()), Err(SetupClaimError::InvalidRecord));
    }
    assert_eq!(codec::decode(b"\xff"), Err(SetupClaimError::InvalidRecord));
}

#[test]
fn invalid_intents_share_existing_workspace_and_materialization_policy() {
    let original = intent();
    for id in [String::new(), "../bad".into(), "space id".into(), "a".repeat(129)] {
        assert!(
            SetupIntent::new(
                id,
                original.objects_directory.clone(),
                original.bundle_store.clone(),
                original.bundle_manifest.clone(),
                original.destination.clone()
            )
            .is_err()
        );
    }
    for path in [
        std::path::PathBuf::from("relative"),
        std::env::temp_dir().join("../escape"),
        std::env::temp_dir().join("a".repeat(4097)),
        std::env::temp_dir().join("nul\0path"),
    ] {
        assert!(
            SetupIntent::new(
                "valid".into(),
                path.clone(),
                original.bundle_store.clone(),
                original.bundle_manifest.clone(),
                original.destination.clone()
            )
            .is_err()
        );
        assert!(
            SetupIntent::new(
                "valid".into(),
                original.objects_directory.clone(),
                path,
                original.bundle_manifest.clone(),
                original.destination.clone()
            )
            .is_err()
        );
    }
    for destination in ["", ".git", "../bad", "a/b", "a\\b", "line\n"] {
        assert!(
            SetupIntent::new(
                "valid".into(),
                original.objects_directory.clone(),
                original.bundle_store.clone(),
                original.bundle_manifest.clone(),
                destination.into()
            )
            .is_err()
        );
    }
}

#[test]
fn every_immutable_field_changes_the_claim_bytes() {
    let original = intent();
    let bytes = codec::encode(&original).unwrap();
    for field in 0..5 {
        let mut changed = original.clone();
        match field {
            0 => changed.workspace_local_id.push('2'),
            1 => changed.objects_directory.push("different"),
            2 => changed.bundle_store.push("different"),
            3 => changed.bundle_manifest = ArtifactDigest::sha256(b"different"),
            _ => changed.destination.push('2'),
        }
        let changed_bytes = codec::encode(&changed).unwrap();
        assert_ne!(changed_bytes, bytes);
        assert_eq!(codec::decode(&changed_bytes).unwrap(), changed);
    }
}

#[cfg(not(target_os = "linux"))]
#[test]
fn unsupported_platform_never_creates_or_grants() {
    let root = tempfile::tempdir().unwrap();
    assert!(matches!(
        RetainedSetup::open(root.path()),
        Err(SetupClaimError::Unsupported)
    ));
    let store = RetainedSetup {};
    assert_eq!(store.observe(&intent()), Err(SetupClaimError::Unsupported));
    assert!(matches!(store.admit(intent()), Err(SetupClaimError::Unsupported)));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}
