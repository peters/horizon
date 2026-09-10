use super::super::{
    EncodedIdentity,
    tests::{Unread, request},
};
use super::*;
use crate::repository_overlay::{RepositoryOverlayPlan, bundle::RepositoryOverlayBundle, storage};
use IntakeError as Error;
use IntakeState as State;
use std::{
    cell::Cell,
    fs::{self, File},
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
};

fn private() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
fn call(parent: &Path, request: &IntakeRequest, input: Option<&mut dyn Read>) -> IntakeResponse {
    execute_with(parent, request, input, &|| false, &mut File::sync_all, &|_| Ok(()))
}
fn data() -> (IntakeRequest, Vec<u8>) {
    let directory = private();
    let repo = git2::Repository::init_bare(directory.path()).unwrap();
    let tree = repo
        .find_tree(repo.treebuilder(None).unwrap().write().unwrap())
        .unwrap();
    let signature = git2::Signature::now("Fixture", "fixture@example.invalid").unwrap();
    let commit = repo
        .commit(None, &signature, &signature, "fixture", &tree, &[])
        .unwrap();
    let mut builder = repo.packbuilder().unwrap();
    builder.insert_commit(commit).unwrap();
    let mut pack = git2::Buf::new();
    builder.write_buf(&mut pack).unwrap();
    let mut request = request();
    request.source.commit = crate::cloud_run::GitCommitSha::parse(commit.to_string()).unwrap();
    let bundle = RepositoryOverlayBundle::new(
        RepositoryOverlayPlan::new(request.source.clone(), vec![], vec![]).unwrap(),
        [],
    )
    .unwrap();
    let overlay = codec::encode(&bundle).unwrap();
    request.pack = EncodedIdentity {
        sha256: ArtifactDigest::sha256(&pack),
        encoded_bytes: pack.len() as u64,
    };
    request.overlay = EncodedIdentity {
        sha256: bundle.manifest_sha256().clone(),
        encoded_bytes: overlay.len() as u64,
    };
    (request, [pack.as_ref(), &overlay].concat())
}

#[test]
fn missing_unsafe_and_unsupported_parents_do_not_create_or_read() {
    let directory = private();
    let parent = directory.path();
    let request = request();
    assert_eq!(
        call(&parent.join("missing"), &request, Some(&mut Unread)).reason,
        Some(Error::Storage)
    );
    assert_eq!(call(parent, &request, None).state, State::Error);
    let response = execute_with(
        parent,
        &request,
        Some(&mut Unread),
        &|| false,
        &mut File::sync_all,
        &|_| Err(Error::Unsupported),
    );
    assert_eq!(response.state, State::Unsupported);
    fs::set_permissions(parent, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(call(parent, &request, Some(&mut Unread)).reason, Some(Error::Storage));
    assert_eq!(fs::read_dir(parent).unwrap().count(), 0);
}

#[test]
fn every_root_sync_failure_retains_claim_and_existing_calls_never_replay() {
    for fault in 0..7 {
        let directory = private();
        let parent = directory.path();
        let request = request();
        let mut calls = 0;
        let response = execute_with(
            parent,
            &request,
            Some(&mut Unread),
            &|| false,
            &mut |_| {
                calls += 1;
                if calls == fault + 1 {
                    Err(io::Error::other("injected"))
                } else {
                    Ok(())
                }
            },
            &|_| Ok(()),
        );
        assert_eq!(
            (response.state, response.reason),
            (State::Unconfirmed, Some(Error::Storage))
        );
        assert_eq!(calls, fault + 1);
        let claim = parent.join("repository-intake.claim");
        let before = fs::read(&claim).unwrap();
        assert_eq!(before, request.encode().unwrap());
        assert_eq!(call(parent, &request, Some(&mut Unread)).state, State::ClaimedUnknown);
        assert_eq!(fs::read(&claim).unwrap(), before);
        let mut conflict = request.clone();
        conflict.runtime_generation += 1;
        assert_eq!(call(parent, &conflict, Some(&mut Unread)).reason, Some(Error::Conflict));
        fs::write(&claim, b"{").unwrap();
        assert_eq!(call(parent, &request, Some(&mut Unread)).reason, Some(Error::Storage));
        assert_eq!(fs::read(&claim).unwrap(), b"{");
    }
}

#[test]
fn cancellation_and_replaced_input_parent_stop_before_child_writes() {
    for replace in [false, true] {
        let directory = private();
        let parent = directory.path();
        let outside = private();
        let changed = Cell::new(false);
        let cancelled = || {
            let inputs = parent.join("repository-inputs");
            if replace && inputs.exists() && !changed.replace(true) {
                fs::rename(&inputs, parent.join("held-inputs")).unwrap();
                symlink(outside.path(), inputs).unwrap();
            }
            !replace && parent.join("repository-intake.claim").exists()
        };
        let response = execute_with(
            parent,
            &request(),
            Some(&mut Unread),
            &cancelled,
            &mut File::sync_all,
            &|_| Ok(()),
        );
        assert_eq!(response.state, State::Unconfirmed);
        assert_eq!(
            response.reason,
            Some(if replace { Error::Storage } else { Error::Cancelled })
        );
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        if replace {
            assert!(changed.get());
            assert_eq!(fs::read_dir(parent.join("held-inputs")).unwrap().count(), 0);
        }
    }
}

#[test]
fn exact_frames_retain_unpublished_pack_until_all_overlay_bytes_verify() {
    let (request, bytes) = data();
    for case in 0..4 {
        let directory = private();
        let parent = directory.path();
        let mut bytes = bytes.clone();
        let mut request = request.clone();
        match case {
            0 => {
                bytes.pop();
            }
            1 => bytes.push(0),
            2 => bytes[usize::try_from(request.pack.encoded_bytes).unwrap()] ^= 1,
            _ => request.overlay.sha256 = ArtifactDigest::sha256(b"wrong"),
        }
        let response = call(parent, &request, Some(&mut bytes.as_slice()));
        assert_eq!(
            (response.state, response.reason),
            (State::Unconfirmed, Some(Error::Input))
        );
        let pack = response.pack.unwrap();
        assert_eq!(pack.state, PackState::Unpublished);
        assert!(pack.source.unwrap().exists());
        assert!(response.bundle.is_none());
        assert!(!parent.join("repository-inputs/packs/base").exists());
    }
}

#[test]
fn qualified_combined_intake_observes_without_sync_replay_or_setup() {
    let directory = private();
    let parent = directory.path();
    let (request, bytes) = data();
    if storage::qualify(&File::open(parent).unwrap()) == Err(storage::StorageQualificationError::Unsupported) {
        eprintln!("SKIP combined intake acknowledgement: qualified ext4 unavailable");
        return;
    }
    let response = execute(parent, &request, Some(&mut bytes.as_slice()), &|| false);
    assert_eq!(response.state, State::Acknowledged, "{response:?}");
    let observed = execute_with(
        parent,
        &request,
        Some(&mut Unread),
        &|| false,
        &mut |_| panic!("observation synchronized"),
        &|_| Ok(()),
    );
    assert_eq!(observed.state, State::Observed);
    assert_eq!(observed.bundle, Some(BundleState::Observed));
    assert_eq!(observed.pack.unwrap().state, PackState::Observed);
    assert_eq!(fs::read_dir(parent.join("repository-setup")).unwrap().count(), 0);
    let bundle = parent
        .join("repository-inputs/bundles")
        .join(format!("{}.hzov", request.overlay.sha256.as_str()));
    fs::set_permissions(&bundle, fs::Permissions::from_mode(0o644)).unwrap();
    let failed = call(parent, &request, Some(&mut Unread));
    assert_eq!(
        (failed.state, failed.reason),
        (State::ClaimedUnknown, Some(Error::Storage))
    );
    assert_eq!(failed.pack.unwrap().state, PackState::Observed);
    assert_eq!(fs::metadata(bundle).unwrap().mode() & 0o7777, 0o644);
}

#[test]
fn existing_children_without_a_claim_are_never_adopted() {
    for name in [INPUTS, SETUP] {
        let directory = private();
        let parent = directory.path();
        fs::create_dir(parent.join(name)).unwrap();
        let sentinel = parent.join(name).join("retained");
        fs::write(&sentinel, b"unchanged").unwrap();
        assert_eq!(call(parent, &request(), Some(&mut Unread)).reason, Some(Error::Storage));
        assert!(!parent.join(CLAIM).exists());
        assert_eq!(fs::read(sentinel).unwrap(), b"unchanged");
    }
}

#[test]
fn competing_creators_have_one_claim_writer_and_no_retry_after_partial_confirmation() {
    use std::sync::{
        Barrier,
        atomic::{AtomicUsize, Ordering},
    };
    let directory = private();
    let parent = directory.path();
    let request = request();
    let barrier = Barrier::new(2);
    let writers = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let contenders: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    let checks = Cell::new(0);
                    execute_with(
                        parent,
                        &request,
                        Some(&mut Unread),
                        &|| {
                            checks.set(checks.get() + 1);
                            if checks.get() == 2 {
                                barrier.wait();
                            }
                            false
                        },
                        &mut |_| {
                            writers.fetch_add(1, Ordering::SeqCst);
                            Err(io::Error::other("unconfirmed claim sync"))
                        },
                        &|_| Ok(()),
                    )
                })
            })
            .collect();
        for contender in contenders {
            let response = contender.join().unwrap();
            assert!(matches!(response.state, State::Unconfirmed | State::ClaimedUnknown));
            assert_eq!(response.reason, Some(Error::Storage));
        }
    });
    assert_eq!(writers.load(Ordering::SeqCst), 1);
    assert_eq!(fs::read(parent.join(CLAIM)).unwrap(), request.encode().unwrap());
    assert_eq!(call(parent, &request, Some(&mut Unread)).state, State::ClaimedUnknown);
    assert_eq!(fs::read_dir(parent).unwrap().count(), 1);
}

#[test]
fn seed_failure_reasons_preserve_storage_and_input_distinction() {
    for (source, expected) in [
        (SeedError::Storage, IntakeError::Storage),
        (SeedError::UnsafeParent, IntakeError::Storage),
        (SeedError::Source, IntakeError::Input),
        (SeedError::Object, IntakeError::Input),
        (SeedError::Limit, IntakeError::Input),
        (SeedError::Unsupported, IntakeError::Unsupported),
        (SeedError::Cancelled, IntakeError::Cancelled),
    ] {
        assert_eq!(seed_error(source), expected);
        assert_eq!(publication_error(PackPublicationError::Verification(source)), expected);
    }
    for (source, expected) in [
        (PackPublicationError::InvalidName, Error::Storage),
        (PackPublicationError::DestinationExists, Error::Storage),
        (PackPublicationError::Unsupported, Error::Unsupported),
        (PackPublicationError::Storage, Error::Storage),
    ] {
        assert_eq!(publication_error(source), expected);
    }
}

#[test]
fn bundle_failure_reasons_distinguish_input_storage_and_capability() {
    use crate::repository_overlay::OverlayPlanError;
    use BundleStoreError as Bundle;
    use RepositoryReadError as ReadError;
    for (source, expected) in [
        (Bundle::Unsupported, Error::Unsupported),
        (Bundle::UnsafeDirectory, Error::Storage),
        (Bundle::Missing, Error::Input),
        (Bundle::Conflict, Error::Input),
        (Bundle::DigestMismatch, Error::Input),
        (Bundle::WriteFailed, Error::Storage),
        (Bundle::Read(ReadError::Unsupported), Error::Unsupported),
        (Bundle::Read(ReadError::InvalidRoot), Error::Storage),
        (Bundle::Read(ReadError::InvalidLimit), Error::Storage),
        (
            Bundle::Read(ReadError::Policy(OverlayPlanError::InvalidPath)),
            Error::Storage,
        ),
        (Bundle::Read(ReadError::Missing), Error::Input),
        (Bundle::Read(ReadError::UnsafePath), Error::Storage),
        (Bundle::Read(ReadError::UnsupportedNode), Error::Storage),
        (Bundle::Read(ReadError::Changed), Error::Storage),
        (Bundle::Read(ReadError::TooLarge), Error::Input),
        (Bundle::Read(ReadError::ReadFailed), Error::Storage),
        (Bundle::Codec(codec::OverlayCodecError::Unsupported), Error::Input),
        (Bundle::Codec(codec::OverlayCodecError::Malformed), Error::Input),
    ] {
        assert_eq!(bundle_error(source), expected);
    }
}
