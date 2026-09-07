use super::*;
use crate::{
    cloud_run::{GitCommitSha, GitSource},
    repository_overlay::{OverlayChange, OverlayContent, RepositoryOverlayPlan, bundle::VerifiedOverlayBlob},
};

fn bundle(payload: &[u8]) -> RepositoryOverlayBundle {
    let staged = VerifiedOverlayBlob::new(b"staged bytes".to_vec()).expect("staged blob");
    let working = VerifiedOverlayBlob::new(payload.to_vec()).expect("working blob");
    let change = |blob: &VerifiedOverlayBlob, executable| {
        OverlayChange::new(
            "selected".into(),
            OverlayContent::File {
                sha256: blob.sha256().clone(),
                bytes: blob.bytes().len().try_into().expect("size"),
                executable,
            },
        )
        .expect("file change")
    };
    let plan = RepositoryOverlayPlan::new(
        GitSource {
            repository: "team/repo".into(),
            commit: GitCommitSha::parse("a".repeat(40)).expect("commit"),
            branch: None,
        },
        [
            change(&staged, false),
            OverlayChange::new("removed".into(), OverlayContent::Remove).expect("removal"),
        ],
        [
            change(&working, true),
            OverlayChange::new(
                "alias".into(),
                OverlayContent::Symlink {
                    target: "selected".into(),
                },
            )
            .expect("literal link"),
        ],
    )
    .expect("plan");
    RepositoryOverlayBundle::new(plan, [staged, working]).expect("bundle")
}

#[cfg(not(target_os = "linux"))]
#[test]
fn unsupported_platform_never_creates_files_or_falls_back() {
    let fixture = tempfile::tempdir().expect("fixture");
    assert!(matches!(
        RepositoryBundleStore::open(fixture.path()),
        Err(BundleStoreError::Unsupported)
    ));
    let store = RepositoryBundleStore {};
    let value = bundle(b"local bytes");
    assert_eq!(store.put(&value), Err(BundleStoreError::Unsupported));
    assert_eq!(store.get(value.manifest_sha256()), Err(BundleStoreError::Unsupported));
    assert_eq!(std::fs::read_dir(fixture.path()).expect("directory").count(), 0);
}

#[cfg(target_os = "linux")]
pub(super) fn private_fixture() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("private fixture")
}

#[cfg(target_os = "linux")]
mod supported {
    use super::*;
    use std::{
        fs::{self, File, OpenOptions},
        io::Write,
        os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, symlink},
        path::PathBuf,
        sync::{Arc, Barrier},
        thread,
    };

    fn record_path(root: &Path, value: &RepositoryOverlayBundle) -> PathBuf {
        root.join(format!("{}.hzov", value.manifest_sha256().as_str()))
    }

    fn private_directory(root: &Path) {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root)
            .expect("private directory");
    }

    fn private_file(path: &Path, bytes: &[u8]) {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .expect("private file")
            .write_all(bytes)
            .expect("fixture bytes");
    }

    #[test]
    fn reopen_and_identical_retry_preserve_layers_inode_mode_and_bytes() {
        let fixture = private_fixture();
        let store = RepositoryBundleStore::open(fixture.path()).expect("store");
        let value = bundle(b"working\0\xff\n");
        let digest = store.put(&value).expect("publish");
        assert_eq!(&digest, value.manifest_sha256());
        let path = record_path(fixture.path(), &value);
        let before = fs::metadata(&path).expect("record metadata");
        assert_eq!(before.mode() & 0o7777, 0o600);
        assert_eq!(before.nlink(), 1);
        assert_eq!(
            fs::read(&path).expect("record"),
            codec::encode(&value).expect("canonical bytes").as_ref()
        );
        drop(store);
        let fresh = RepositoryBundleStore::open(fixture.path()).expect("fresh store");
        assert_eq!(fresh.get(&digest).expect("reopen"), value);
        assert_eq!(fresh.put(&value).expect("idempotent retry"), digest);
        let after = fs::metadata(&path).expect("same record");
        assert_eq!(
            (before.ino(), before.mtime(), before.mtime_nsec()),
            (after.ino(), after.mtime(), after.mtime_nsec())
        );
        assert_eq!(fs::read_dir(fixture.path()).expect("records").count(), 1);
    }

    #[test]
    fn distinct_bundles_and_empty_payloads_coexist_without_overwrite() {
        let fixture = private_fixture();
        let store = RepositoryBundleStore::open(fixture.path()).expect("store");
        for bytes in [b"first".as_slice(), b"second", b""] {
            let value = bundle(bytes);
            let digest = store.put(&value).expect("publish");
            assert_eq!(store.get(&digest).expect("read"), value);
        }
        assert_eq!(fs::read_dir(fixture.path()).expect("records").count(), 3);
    }

    #[test]
    fn concurrent_independent_writers_converge_to_one_complete_record() {
        let fixture = private_fixture();
        let value = Arc::new(bundle(b"same bytes"));
        let barrier = Barrier::new(8);
        thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        let store = RepositoryBundleStore::open(fixture.path()).expect("independent store");
                        barrier.wait();
                        store.put(&value).expect("concurrent publish")
                    })
                })
                .collect();
            for handle in handles {
                assert_eq!(&handle.join().expect("writer"), value.manifest_sha256());
            }
        });
        let fresh = RepositoryBundleStore::open(fixture.path()).expect("fresh store");
        assert_eq!(fresh.get(value.manifest_sha256()).expect("complete record"), *value);
        assert_eq!(fs::read_dir(fixture.path()).expect("records").count(), 1);
    }

    #[test]
    fn renamed_root_never_selects_a_replacement_directory() {
        let fixture = private_fixture();
        let selected = fixture.path().join("selected");
        let retained = fixture.path().join("retained");
        private_directory(&selected);
        let original = RepositoryBundleStore::open(&selected).expect("original store");
        fs::rename(&selected, &retained).expect("rename root");
        private_directory(&selected);
        let value = bundle(b"pinned root");
        original.put(&value).expect("write pinned directory");
        assert_eq!(fs::read_dir(&selected).expect("replacement").count(), 0);
        assert_eq!(
            RepositoryBundleStore::open(&selected)
                .expect("replacement store")
                .get(value.manifest_sha256()),
            Err(BundleStoreError::Missing)
        );
        assert_eq!(
            RepositoryBundleStore::open(&retained)
                .expect("retained store")
                .get(value.manifest_sha256())
                .expect("original bytes"),
            value
        );
    }

    #[test]
    fn missing_nonprivate_and_linked_roots_are_not_created_or_repaired() {
        let fixture = private_fixture();
        let missing = fixture.path().join("absent");
        assert!(RepositoryBundleStore::open(&missing).is_err());
        assert!(!missing.exists());
        assert!(RepositoryBundleStore::open(Path::new("relative")).is_err());
        let public = fixture.path().join("public");
        private_directory(&public);
        fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).expect("nonprivate mode");
        assert!(matches!(
            RepositoryBundleStore::open(&public),
            Err(BundleStoreError::UnsafeDirectory)
        ));
        assert_eq!(fs::metadata(&public).expect("unchanged root").mode() & 0o7777, 0o755);
        symlink(fixture.path(), fixture.path().join("linked")).expect("root symlink");
        assert!(RepositoryBundleStore::open(&fixture.path().join("linked")).is_err());
    }

    #[test]
    fn root_permissions_are_rechecked_after_open() {
        let fixture = private_fixture();
        let store = RepositoryBundleStore::open(fixture.path()).expect("store");
        let value = bundle(b"private bytes");
        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o750)).expect("permission change");
        assert_eq!(
            store.get(value.manifest_sha256()),
            Err(BundleStoreError::UnsafeDirectory)
        );
        assert_eq!(store.put(&value), Err(BundleStoreError::UnsafeDirectory));
        assert_eq!(fs::read_dir(fixture.path()).expect("unchanged directory").count(), 0);
    }

    #[test]
    fn corrupt_existing_record_is_never_overwritten_or_returned() {
        let fixture = private_fixture();
        let store = RepositoryBundleStore::open(fixture.path()).expect("store");
        let value = bundle(b"payload secret marker");
        let digest = store.put(&value).expect("publish");
        let path = record_path(fixture.path(), &value);
        let mut corrupted = fs::read(&path).expect("record");
        *corrupted.last_mut().expect("payload") ^= 1;
        fs::write(&path, &corrupted).expect("corrupt owned fixture");
        assert!(matches!(store.get(&digest), Err(BundleStoreError::Codec(_))));
        assert_eq!(store.put(&value), Err(BundleStoreError::Conflict));
        assert_eq!(fs::read(&path).expect("preserved corrupt record"), corrupted);
    }

    #[test]
    fn valid_bundle_under_the_wrong_digest_is_rejected_and_preserved() {
        let fixture = private_fixture();
        let store = RepositoryBundleStore::open(fixture.path()).expect("store");
        let wanted = bundle(b"wanted");
        let different = codec::encode(&bundle(b"different")).expect("other canonical bundle");
        let path = record_path(fixture.path(), &wanted);
        private_file(&path, &different);
        assert_eq!(
            store.get(wanted.manifest_sha256()),
            Err(BundleStoreError::DigestMismatch)
        );
        assert_eq!(store.put(&wanted), Err(BundleStoreError::Conflict));
        assert_eq!(fs::read(&path).expect("unchanged record"), different.as_ref());
    }

    #[test]
    fn unsafe_record_nodes_are_never_followed_read_or_replaced() {
        for kind in ["symlink", "hardlink", "fifo", "directory", "nonprivate"] {
            let fixture = private_fixture();
            let store = RepositoryBundleStore::open(fixture.path()).expect("store");
            let value = bundle(b"new contents");
            let target = fixture.path().join("untouched");
            private_file(&target, b"untouched synthetic contents");
            let path = record_path(fixture.path(), &value);
            match kind {
                "symlink" => symlink(&target, &path).expect("symlink"),
                "hardlink" => fs::hard_link(&target, &path).expect("hardlink"),
                "fifo" => rustix::fs::mkfifoat(rustix::fs::CWD, &path, rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR)
                    .expect("fifo"),
                "directory" => private_directory(&path),
                _ => {
                    private_file(&path, b"not private");
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).expect("nonprivate mode");
                }
            }
            let before = fs::symlink_metadata(&path).expect("node");
            assert!(store.get(value.manifest_sha256()).is_err(), "{kind}");
            assert!(store.put(&value).is_err(), "{kind}");
            let after = fs::symlink_metadata(&path).expect("unchanged node");
            assert_eq!((before.ino(), before.mode()), (after.ino(), after.mode()));
            assert_eq!(
                fs::read(&target).expect("untouched target"),
                b"untouched synthetic contents"
            );
        }
    }

    #[test]
    fn oversized_sparse_record_is_rejected_before_content_allocation() {
        let fixture = private_fixture();
        let store = RepositoryBundleStore::open(fixture.path()).expect("store");
        let value = bundle(b"small");
        let path = record_path(fixture.path(), &value);
        private_file(&path, b"");
        let size = u64::try_from(codec::MAX_ENCODED_BUNDLE_BYTES).expect("limit") + 1;
        File::options()
            .write(true)
            .open(&path)
            .expect("sparse fixture")
            .set_len(size)
            .expect("sparse length");
        assert_eq!(
            store.get(value.manifest_sha256()),
            Err(BundleStoreError::Read(RepositoryReadError::TooLarge))
        );
        assert_eq!(
            store.put(&value),
            Err(BundleStoreError::Read(RepositoryReadError::TooLarge))
        );
        assert_eq!(fs::metadata(&path).expect("preserved sparse fixture").len(), size);
    }

    #[test]
    fn diagnostics_never_include_paths_digests_or_payloads() {
        let fixture = private_fixture();
        let store = RepositoryBundleStore::open(fixture.path()).expect("store");
        let value = bundle(b"sensitive fixture marker");
        let error = store.get(value.manifest_sha256()).expect_err("missing");
        let diagnostic = format!("{store:?} {error:?} {error}");
        for excluded in [
            fixture.path().to_str().expect("path"),
            value.manifest_sha256().as_str(),
            "sensitive fixture marker",
        ] {
            assert!(!diagnostic.contains(excluded));
        }
    }
}
