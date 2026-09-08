use super::super::tests::{Source, change, file, fixture, private, resolved};
use super::*;
use crate::repository_overlay::{OverlayContent, checkout::prepare_private_checkout};
use git2::Repository;
use linux::SyncPoint;
use std::{
    cell::Cell,
    fs::{self, File},
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
};

fn prepare(parent: &Path) -> PreparedPrivateCheckout {
    let source = private();
    let repository = Repository::init(source.path()).unwrap();
    let base = fixture(
        &[("base", repository.blob(b"base\0binary\xff").unwrap(), 0o100_644)],
        &repository,
    );
    let (staged, staged_blob) = file("nested/tool", b"staged\0tool", true);
    let (raw, raw_blob) = file("nested/tool", b"working\0tool\r\n", true);
    let link = OverlayContent::Symlink {
        target: "long-dangling-target-".repeat(12),
    };
    let plan = resolved(
        &repository,
        base,
        vec![staged],
        vec![raw, change("link", link)],
        vec![staged_blob, raw_blob],
    );
    prepare_private_checkout(parent, &plan, &mut Source(repository.odb().unwrap()), || false).unwrap()
}

fn sync(_: SyncPoint, file: &File) -> Result<(), PublicationError> {
    file.sync_all().map_err(|_| PublicationError::Storage)
}

fn inject_sync(
    stage: PreparedPrivateCheckout,
    cancelled: &impl Fn() -> bool,
    sync: &mut impl FnMut(SyncPoint, &File) -> Result<(), PublicationError>,
) -> Result<PublishedCheckout, PublicationFailure> {
    linux::publish(stage, "ready", cancelled, sync, &mut linux::rename, &|_| Ok(()))
}

// State/identity tests exercise real files and rename, but make no durability claim.
fn publish(stage: PreparedPrivateCheckout, name: &str) -> Result<PublishedCheckout, PublicationFailure> {
    linux::publish(stage, name, &|| false, &mut sync, &mut linux::rename, &|_| Ok(()))
}

fn unpublished(failure: PublicationFailure, expected: PublicationError) -> PreparedPrivateCheckout {
    assert!(!format!("{failure:?} {failure}").contains("/tmp/"));
    let PublicationFailure::Unpublished { reason, checkout } = failure else {
        panic!("wrong ownership state")
    };
    assert_eq!(reason, expected);
    checkout
}

#[test]
fn qualified_real_publication_preserves_inode_receipt_and_long_symlink() {
    let parent = private();
    let stage = prepare(parent.path());
    if linux::supported_storage(&stage.parent) == Err(PublicationError::Unsupported) {
        eprintln!("SKIP real publication: fixture is not qualified journaled ext4");
        return;
    }
    let old = stage.path().to_owned();
    let inode = fs::metadata(&old).unwrap().ino();
    let base = stage.base_commit();
    let digest = stage.manifest_sha256().clone();
    let checkout = publish_sibling_checkout(stage, "ready", || false).unwrap();
    assert!(!old.exists());
    assert_eq!(checkout.path(), parent.path().join("ready"));
    assert_eq!(fs::metadata(checkout.path()).unwrap().ino(), inode);
    assert_eq!((checkout.base_commit(), checkout.manifest_sha256()), (base, &digest));
    assert_eq!(
        fs::read_link(checkout.path().join("link")).unwrap(),
        Path::new(&"long-dangling-target-".repeat(12))
    );
    assert!(!format!("{checkout:?}").contains("ready"));
    drop(checkout);
    assert!(parent.path().join("ready/.git/HEAD").exists());
}

#[test]
fn existing_names_and_invalid_names_never_replace_or_remove_anything() {
    let parent = private();
    fs::write(parent.path().join("file"), b"sentinel").unwrap();
    fs::create_dir(parent.path().join("directory")).unwrap();
    symlink("missing", parent.path().join("symlink")).unwrap();
    let mut stage = prepare(parent.path());
    let old = stage.path().to_owned();
    for name in ["file", "directory", "symlink"] {
        stage = unpublished(publish(stage, name).unwrap_err(), PublicationError::DestinationExists);
        assert!(old.exists());
    }
    for name in [
        "",
        ".",
        "..",
        "../escape",
        "a/b",
        "a\\b",
        ".git",
        "nul\0name",
        "line\n",
        "bad.",
        old.file_name().unwrap().to_str().unwrap(),
    ] {
        stage = unpublished(publish(stage, name).unwrap_err(), PublicationError::InvalidName);
    }
    drop(stage);
    assert!(old.exists());
    assert_eq!(fs::read(parent.path().join("file")).unwrap(), b"sentinel");
    assert!(parent.path().join("directory").is_dir());
    assert_eq!(
        fs::read_link(parent.path().join("symlink")).unwrap(),
        Path::new("missing")
    );
}

#[test]
fn actual_sync_order_and_failures_distinguish_pre_and_post_rename() {
    for fail in [
        None,
        Some(SyncPoint::File),
        Some(SyncPoint::Directory),
        Some(SyncPoint::PublishedRoot),
        Some(SyncPoint::Parent),
    ] {
        let parent = private();
        let stage = prepare(parent.path());
        let old = stage.path().to_owned();
        let target = parent.path().join("ready");
        let mut points = Vec::new();
        let root_inode = fs::metadata(&old).unwrap().ino();
        let nested_inode = fs::metadata(old.join("nested")).unwrap().ino();
        let mut directories = Vec::new();
        let result = inject_sync(stage, &|| false, &mut |point, file| {
            points.push(point);
            if point == SyncPoint::Directory {
                directories.push(file.metadata().unwrap().ino());
            }
            assert_eq!(
                target.exists(),
                matches!(point, SyncPoint::PublishedRoot | SyncPoint::Parent)
            );
            if Some(point) == fail {
                return Err(PublicationError::Storage);
            }
            sync(point, file)
        });
        match fail {
            None => {
                result.unwrap();
            }
            Some(SyncPoint::File | SyncPoint::Directory) => {
                let receipt = unpublished(result.unwrap_err(), PublicationError::Storage);
                assert_eq!(receipt.path(), old);
                drop(receipt);
                assert!(old.exists() && !target.exists());
            }
            Some(SyncPoint::PublishedRoot | SyncPoint::Parent) => {
                let failure = result.unwrap_err();
                assert!(!format!("{failure:?} {failure}").contains("/tmp/"));
                let PublicationFailure::PublishedUnsynchronized { reason, checkout } = failure else {
                    panic!("wrong state")
                };
                assert_eq!(reason, PublicationError::Storage);
                assert_eq!(checkout.path(), target);
                drop(checkout);
                assert!(!old.exists() && target.join(".git/HEAD").exists());
            }
        }
        let rank = |point| match point {
            SyncPoint::File => 0,
            SyncPoint::Directory => 1,
            SyncPoint::PublishedRoot => 2,
            SyncPoint::Parent => 3,
        };
        assert!(points.windows(2).all(|pair| rank(pair[0]) <= rank(pair[1])));
        if fail.is_none() {
            assert_eq!(points.last(), Some(&SyncPoint::Parent));
            assert_eq!(directories.last(), Some(&root_inode));
            assert!(directories.contains(&nested_inode));
        }
    }
}

#[test]
fn cancellation_before_and_after_real_rename_retains_the_correct_name() {
    for after in [false, true] {
        let parent = private();
        let stage = prepare(parent.path());
        let old = stage.path().to_owned();
        let target = parent.path().join("ready");
        let before = Cell::new(false);
        let root_inode = fs::metadata(&old).unwrap().ino();
        let result = inject_sync(
            stage,
            &|| if after { target.exists() } else { before.get() },
            &mut |point, file| {
                sync(point, file)?;
                if point == SyncPoint::Directory && file.metadata().unwrap().ino() == root_inode {
                    before.set(true);
                }
                Ok(())
            },
        );
        if after {
            assert!(matches!(
                result,
                Err(PublicationFailure::PublishedUnsynchronized {
                    reason: PublicationError::Cancelled,
                    ..
                })
            ));
            assert!(target.exists() && !old.exists());
        } else {
            unpublished(result.unwrap_err(), PublicationError::Cancelled);
            assert!(old.exists() && !target.exists());
        }
    }
}

#[test]
fn replaced_parent_or_stage_and_late_binding_changes_are_rejected() {
    for replace_parent in [false, true] {
        let outer = private();
        let parent = outer.path().join("parent");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        let stage = prepare(&parent);
        let old = if replace_parent {
            parent.clone()
        } else {
            stage.path().to_owned()
        };
        let moved = outer.path().join("retained");
        fs::rename(&old, &moved).unwrap();
        fs::create_dir(&old).unwrap();
        fs::set_permissions(&old, fs::Permissions::from_mode(0o700)).unwrap();
        unpublished(publish(stage, "ready").unwrap_err(), PublicationError::UnsafeNode);
        assert!(moved.exists() && !parent.join("ready").exists());
    }
    let parent = private();
    let stage = prepare(parent.path());
    let old = stage.path().to_owned();
    let changed = Cell::new(false);
    let result = inject_sync(stage, &|| false, &mut |point, file| {
        sync(point, file)?;
        if point == SyncPoint::Directory && !changed.replace(true) {
            fs::rename(&old, parent.path().join("retained")).unwrap();
        }
        Ok(())
    });
    unpublished(result.unwrap_err(), PublicationError::UnsafeNode);
    assert!(parent.path().join("retained").exists() && !parent.path().join("ready").exists());
}

#[test]
fn unsupported_nodes_metadata_links_and_oversized_sparse_files_fail_before_rename() {
    for kind in 0..5 {
        let parent = private();
        let stage = prepare(parent.path());
        let extra = stage.path().join("extra");
        match kind {
            0 => symlink("../escape", &extra).unwrap(),
            1 => symlink("config", stage.path().join(".git/extra")).unwrap(),
            2 => fs::hard_link(stage.path().join("base"), &extra).unwrap(),
            3 => rustix::fs::mknodat(
                rustix::fs::CWD,
                &extra,
                rustix::fs::FileType::Fifo,
                rustix::fs::Mode::RUSR,
                0,
            )
            .unwrap(),
            _ => File::create(&extra).unwrap().set_len(walk::MAX_BYTES + 1).unwrap(),
        }
        let reason = if kind == 4 {
            PublicationError::Limit
        } else {
            PublicationError::UnsafeNode
        };
        unpublished(publish(stage, "ready").unwrap_err(), reason);
        assert!(!parent.path().join("ready").exists());
    }
}

#[test]
fn walk_budgets_charge_before_growth_and_include_both_working_and_seed_bytes() {
    let mut budget = walk::Budget::default();
    budget
        .charge(1, 0, crate::repository_overlay::MAX_CONTENT_BYTES)
        .unwrap();
    budget.charge(1, 0, crate::repository_overlay::seed::MAX_BYTES).unwrap();
    assert_eq!(budget.charge(1, 0, u64::MAX), Err(PublicationError::Limit));
    let mut full = walk::Budget {
        nodes: walk::MAX_NODES,
        ..walk::Budget::default()
    };
    assert_eq!(full.charge(1, 0, 0), Err(PublicationError::Limit));
    assert_eq!(
        walk::Budget::default().charge(usize::MAX, 1, 0),
        Err(PublicationError::Limit)
    );
    let parent = private();
    let stage = prepare(parent.path());
    for n in 0..walk::MAX_NODES {
        File::create(stage.path().join(format!("node-{n}"))).unwrap();
    }
    unpublished(publish(stage, "ready").unwrap_err(), PublicationError::Limit);
    assert!(!parent.path().join("ready").exists());
}

#[test]
fn volatile_tmpfs_is_rejected_without_publishing_or_deleting_the_stage() {
    let Ok(parent) = tempfile::Builder::new()
        .prefix("publication-test-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in("/dev/shm")
    else {
        eprintln!("SKIP volatile fixture: no writable shared-memory directory");
        return;
    };
    if rustix::fs::fstatfs(File::open(parent.path()).unwrap()).unwrap().f_type != libc::TMPFS_MAGIC {
        eprintln!("SKIP volatile fixture: shared-memory directory is not tmpfs");
        return;
    }
    let stage = prepare(parent.path());
    let old = stage.path().to_owned();
    let receipt = unpublished(
        publish_sibling_checkout(stage, "ready", || false).unwrap_err(),
        PublicationError::Unsupported,
    );
    drop(receipt);
    assert!(old.join(".git/HEAD").exists() && !parent.path().join("ready").exists());
    parent.close().unwrap();
}

#[test]
fn mutation_during_final_directory_sync_prevents_publication() {
    let parent = private();
    let stage = prepare(parent.path());
    let old = stage.path().to_owned();
    let root_inode = fs::metadata(&old).unwrap().ino();
    let result = inject_sync(stage, &|| false, &mut |point, file| {
        sync(point, file)?;
        if point == SyncPoint::Directory && file.metadata().unwrap().ino() == root_inode {
            fs::write(old.join("base"), b"changed after file synchronization").unwrap();
        }
        Ok(())
    });
    unpublished(result.unwrap_err(), PublicationError::UnsafeNode);
    assert_eq!(
        fs::read(old.join("base")).unwrap(),
        b"changed after file synchronization"
    );
    assert!(!parent.path().join("ready").exists());
}

#[test]
fn rename_error_after_actual_rename_is_uncertain_and_retains_both_names_for_inspection() {
    let parent = private();
    let stage = prepare(parent.path());
    let old = stage.path().to_owned();
    let failure = linux::publish(
        stage,
        "ready",
        &|| false,
        &mut sync,
        &mut |stage, name| {
            linux::rename(stage, name)?;
            Err(rustix::io::Errno::IO)
        },
        &|_| Ok(()),
    )
    .unwrap_err();
    assert!(!format!("{failure:?} {failure}").contains("/tmp/"));
    let PublicationFailure::RenameUnconfirmed { checkout, destination } = failure else {
        panic!("wrong state")
    };
    assert_eq!(checkout.path(), old);
    assert_eq!(destination, parent.path().join("ready"));
    drop(checkout);
    assert!(!old.exists() && destination.join(".git/HEAD").exists());
}

#[test]
fn journal_contract_requires_exact_noncontradictory_bounded_kernel_options() {
    for mode in ["ordered", "journal"] {
        assert_eq!(linux::journaled_options(&format!("rw\nbarrier\ndata={mode}\n")), Ok(()));
    }
    for tail in [
        "",
        "data=writeback\n",
        "data=ordered",
        "data=ordered\r\n",
        "data=ordered\nro\n",
        "data=ordered\nnobarrier\n",
        "data=ordered\nbarrier\n",
        "data=ordered\ndata=journal\n",
        "\n",
    ] {
        assert_eq!(
            linux::journaled_options(&format!("rw\nbarrier\n{tail}")),
            Err(PublicationError::Unsupported)
        );
    }
    assert!(linux::journaled_options("rw\ndata=ordered\n").is_err());
    assert!(linux::journaled_options(&"rw\nbarrier\ndata=ordered\n".repeat(200)).is_err());
}
