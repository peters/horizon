use super::super::tests::request;
use super::*;
use crate::repository_overlay::retained_setup::SetupCompletionState as State;

fn selection() -> SetupCheckoutSelection {
    SetupCheckoutSelection::new(request(), "published".into()).unwrap()
}

#[test]
fn canonical_binding_is_inert_strict_and_covers_runtime_and_destination() {
    let selected = selection();
    let bytes = serde_json::to_vec(&selected).unwrap();
    assert_eq!(SetupCheckoutSelection::decode(&bytes).unwrap(), selected);
    let value = serde_json::to_value(&selected).unwrap();
    assert_eq!(
        SetupCheckoutSelection::decode(&serde_json::to_vec(&value).unwrap())
            .unwrap()
            .binding(),
        selected.binding()
    );
    for bytes in [
        vec![b' '; SETUP_SELECTION_LIMIT + 1],
        b"{}".to_vec(),
        [b"{\"version\":1,".as_slice(), &bytes[1..]].concat(),
    ] {
        assert_eq!(
            SetupCheckoutSelection::decode(&bytes).err(),
            Some(SetupCheckoutError::Invalid)
        );
    }
    for (field, value) in [
        ("version", serde_json::json!(2)),
        ("extra", serde_json::json!(true)),
        ("destination", serde_json::json!("../escape")),
    ] {
        let mut changed = serde_json::to_value(&selected).unwrap();
        changed[field] = value;
        assert!(SetupCheckoutSelection::decode(&serde_json::to_vec(&changed).unwrap()).is_err());
    }
    let mut changed = selected.clone();
    changed.intake.runtime_generation += 1;
    assert_ne!(changed.binding(), selected.binding());
    changed = selected.clone();
    changed.destination.push('2');
    assert_ne!(changed.binding(), selected.binding());
    assert_eq!(selected.runtime(), selected.intake.job_id);
    assert_eq!(selected.inspect(|| true).err(), Some(SetupCheckoutError::Cancelled));
    assert!(!format!("{selected:?}").contains("fixture"));
}

#[test]
fn only_exact_published_completion_matches_original_repository_identity() {
    let selected = selection();
    for state in [
        State::Rejected,
        State::Unpublished,
        State::RenameUnconfirmed,
        State::PublishedUnsynchronized,
        State::Published,
    ] {
        assert_eq!(
            published(
                &selected,
                state,
                Some(selected.intake.source.commit.as_str()),
                Some(&selected.intake.overlay.sha256)
            ),
            state == State::Published
        );
    }
    for (base, manifest) in [
        (None, Some(&selected.intake.overlay.sha256)),
        (
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            Some(&selected.intake.overlay.sha256),
        ),
        (Some(selected.intake.source.commit.as_str()), None),
    ] {
        assert!(!published(&selected, State::Published, base, manifest));
    }
    assert!(!published(
        &selected,
        State::Published,
        Some(selected.intake.source.commit.as_str()),
        Some(&ArtifactDigest::sha256(b"wrong"))
    ));
}

#[cfg(not(target_os = "linux"))]
#[test]
fn inspection_refuses_unsupported_platform_without_losing_binding() {
    let selected = selection();
    assert_eq!(selected.inspect(|| false).err(), Some(SetupCheckoutError::Unsupported));
    assert!(selected.binding().is_ok());
}

#[cfg(target_os = "linux")]
mod supported {
    use super::*;
    use crate::repository_overlay::{
        intake::{BundleState, IntakeError, IntakeResponse, IntakeRoots, IntakeState, PackProgress, PackState},
        retained_setup::{RetainedSetup, SetupClaimError, SetupIntent, completion_fixture},
    };
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, PermissionsExt, symlink},
    };

    #[test]
    fn retained_record_reasons_preserve_unsupported_cancellation_and_uncertainty() {
        use crate::repository_overlay::retained_setup::{SetupBoundaryError as Boundary, SetupRecordError as Record};
        for (error, expected) in [
            (Record::Boundary(Boundary::Unsupported), SetupCheckoutError::Unsupported),
            (Record::Boundary(Boundary::Cancelled), SetupCheckoutError::Cancelled),
            (
                Record::Boundary(Boundary::InvalidClaim),
                SetupCheckoutError::Unconfirmed,
            ),
            (Record::InvalidRecord, SetupCheckoutError::Unconfirmed),
            (Record::Boundary(Boundary::UnsafeRoot), SetupCheckoutError::Storage),
            (Record::Read, SetupCheckoutError::Storage),
            (Record::Storage, SetupCheckoutError::Storage),
        ] {
            assert_eq!(record_error(error), expected);
        }
    }

    #[test]
    fn checkout_pin_does_not_freeze_development_files_and_rejects_unsafe_roots() {
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = directory.path().join("published");
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let held = pin_location(directory.path(), &root).unwrap();
        for name in ["HEAD", "index", "working"] {
            fs::write(root.join(name), b"evolving development bytes").unwrap();
        }
        let current = pin_location(directory.path(), &root).unwrap();
        assert_eq!((held.0.device, held.0.inode), (current.0.device, current.0.inode));
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(pin_location(directory.path(), &root).is_err());
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let moved = directory.path().join("retained");
        fs::rename(&root, &moved).unwrap();
        assert!(pin_location(directory.path(), &root).is_err());
        symlink(&moved, &root).unwrap();
        assert!(pin_location(directory.path(), &root).is_err());
        assert!(pin_location(&root, &moved).is_err());
        assert_eq!(fs::read(moved.join("working")).unwrap(), b"evolving development bytes");
    }

    #[test]
    fn existing_setup_records_are_read_only_and_unknown_or_conflicting_results_refuse() {
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let parent = directory.path();
        if matches!(RetainedSetup::open(parent), Err(SetupClaimError::Unsupported)) {
            eprintln!("SKIP setup checkout record fixture: qualified ext4 unavailable");
            return;
        }
        let selected = selection();
        let roots = IntakeRoots {
            packs: parent.join("packs"),
            bundles: parent.join("bundles"),
            setup: parent.to_owned(),
        };
        let observed = || {
            let mut response = IntakeResponse::failure(IntakeError::Input);
            response.state = IntakeState::Observed;
            response.reason = None;
            response.roots = Some(roots.clone());
            response.bundle = Some(BundleState::Observed);
            response.pack = Some(PackProgress {
                state: PackState::Observed,
                source: None,
                destination: None,
                objects: None,
            });
            response
        };
        let intent = SetupIntent::new(
            selected.intake.workspace_local_id.clone(),
            roots.packs.join("base/decoded/objects"),
            roots.bundles.clone(),
            selected.intake.overlay.sha256.clone(),
            selected.destination.clone(),
        )
        .unwrap();
        let (claim, result) = completion_fixture(parent, &intent, State::Published);
        assert!(inspect_observed(&selected, observed(), &|| false).is_err());
        let claim_path = parent.join("setup-claim.json");
        fs::write(&claim_path, &claim).unwrap();
        fs::set_permissions(&claim_path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            inspect_observed(&selected, observed(), &|| false).err(),
            Some(SetupCheckoutError::Unconfirmed)
        );
        let checkout = parent.join("setup-data/published");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&checkout)
            .unwrap();
        let result_path = parent.join("setup-result.json");
        for state in [
            State::Rejected,
            State::Unpublished,
            State::RenameUnconfirmed,
            State::PublishedUnsynchronized,
            State::Published,
        ] {
            fs::write(&result_path, completion_fixture(parent, &intent, state).1).unwrap();
            fs::set_permissions(&result_path, fs::Permissions::from_mode(0o600)).unwrap();
            let before = fs::read(&result_path).unwrap();
            assert_eq!(
                inspect_observed(&selected, observed(), &|| false).is_ok(),
                state == State::Published
            );
            assert_eq!(fs::read(&result_path).unwrap(), before);
        }
        assert_eq!(fs::read(&result_path).unwrap(), result);
        let mut changed = selected.clone();
        changed.destination.push('2');
        assert!(inspect_observed(&changed, observed(), &|| false).is_err());
        let swapped = std::cell::Cell::new(false);
        let retained = parent.join("retained-checkout");
        let outcome = inspect_observed(&selected, observed(), &|| {
            if !swapped.replace(true) {
                fs::rename(&checkout, &retained).unwrap();
                fs::DirBuilder::new().mode(0o700).create(&checkout).unwrap();
            }
            false
        });
        assert_eq!(outcome.err(), Some(SetupCheckoutError::Storage));
        fs::remove_dir(&checkout).unwrap();
        fs::rename(&retained, &checkout).unwrap();
        fs::rename(&checkout, parent.join("retained-checkout")).unwrap();
        assert!(inspect_observed(&selected, observed(), &|| false).is_err());
        assert_eq!(fs::read(&claim_path).unwrap(), claim);
        assert_eq!(fs::read(&result_path).unwrap(), result);
    }
}
