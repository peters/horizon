use super::*;

#[cfg(target_os = "linux")]
mod linux_tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
        sync::{Arc, Barrier},
    };

    fn fixture() -> (tempfile::TempDir, RemoteSshIdentityStore, CloudWorkflowId, CloudJobId) {
        let directory = tempfile::tempdir().expect("private fixture");
        let store = RemoteSshIdentityStore::new(&HorizonHome::from_root(directory.path().join("home")));
        (directory, store, CloudWorkflowId::new(), CloudJobId::new())
    }

    #[test]
    fn retained_key_survives_store_drop_and_matches_the_reserved_public_identity() {
        let (directory, store, workflow, job) = fixture();
        let identity = store.prepare_new(workflow, job).expect("new identity");
        let public = identity.public_key().to_string();
        let path = identity.private_key_path().to_path_buf();
        assert!(public.starts_with("ssh-ed25519 "));
        assert_eq!(public.split(' ').count(), 2);
        assert_eq!(fs::metadata(&path).expect("file").permissions().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(path.parent().expect("parent"))
                .expect("directory")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert!(!format!("{identity:?}").contains(&public));
        drop(identity);
        drop(store);
        let store = RemoteSshIdentityStore::new(&HorizonHome::from_root(directory.path().join("home")));
        let recovered = store.recover(workflow, job, &public).expect("recovery");
        assert_eq!(recovered.public_key(), public);
        assert_eq!(recovered.private_key_path(), path);
        assert_eq!(
            store
                .prepare_new(workflow, job)
                .expect("interrupted setup candidate")
                .public_key(),
            public
        );
        assert_eq!(
            fs::read_dir(path.parent().expect("parent")).expect("directory").count(),
            1
        );
    }

    #[test]
    fn missing_or_mismatched_identity_never_generates_a_replacement() {
        let (_directory, store, workflow, job) = fixture();
        let identity = store.prepare_new(workflow, job).expect("identity");
        let other = store
            .prepare_new(CloudWorkflowId::new(), CloudJobId::new())
            .expect("other");
        assert_eq!(
            store.recover(workflow, job, other.public_key()).expect_err("mismatch"),
            RemoteSshIdentityError::Mismatch
        );
        assert_eq!(
            store.recover(workflow, job, "secret-test-value").expect_err("invalid"),
            RemoteSshIdentityError::InvalidIdentity
        );
        fs::remove_file(identity.private_key_path()).expect("simulate lost key");
        assert_eq!(
            store.recover(workflow, job, identity.public_key()).expect_err("lost"),
            RemoteSshIdentityError::Missing
        );
        assert!(!identity.private_key_path().exists());
    }

    #[test]
    fn concurrent_preparation_publishes_one_complete_key_and_reuses_the_winner() {
        let (_directory, store, workflow, job) = fixture();
        let store = Arc::new(store);
        let barrier = Arc::new(Barrier::new(3));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    store.prepare_new(workflow, job)
                })
            })
            .collect();
        barrier.wait();
        let identities: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("thread").expect("identity"))
            .collect();
        assert_eq!(identities[0].public_key(), identities[1].public_key());
        assert_eq!(identities[0].private_key_path(), identities[1].private_key_path());
        assert_eq!(
            store
                .recover(workflow, job, identities[0].public_key())
                .expect("recovery")
                .public_key(),
            identities[0].public_key()
        );
    }

    #[test]
    fn insecure_paths_are_rejected_without_repairing_or_overwriting_them() {
        let (_directory, store, workflow, job) = fixture();
        let identity = store.prepare_new(workflow, job).expect("identity");
        let key_path = identity.private_key_path();
        fs::set_permissions(key_path, fs::Permissions::from_mode(0o644)).expect("fault");
        assert_eq!(
            store.recover(workflow, job, identity.public_key()).expect_err("unsafe"),
            RemoteSshIdentityError::InsecurePath
        );
        assert_eq!(
            store.prepare_new(workflow, job).expect_err("unsafe candidate"),
            RemoteSshIdentityError::InsecurePath
        );
        assert_eq!(
            fs::metadata(key_path).expect("file").permissions().mode() & 0o777,
            0o644
        );
        fs::set_permissions(key_path, fs::Permissions::from_mode(0o600)).expect("restore");
        fs::set_permissions(key_path.parent().expect("parent"), fs::Permissions::from_mode(0o755))
            .expect("fault directory");
        assert_eq!(
            store
                .recover(workflow, job, identity.public_key())
                .expect_err("unsafe directory"),
            RemoteSshIdentityError::InsecurePath
        );
    }

    #[test]
    fn symlinks_empty_and_oversized_keys_fail_closed() {
        let (directory, store, workflow, job) = fixture();
        let identity = store.prepare_new(workflow, job).expect("identity");
        let key_path = identity.private_key_path();
        let saved = directory.path().join("saved-key");
        fs::rename(key_path, &saved).expect("save");
        symlink(&saved, key_path).expect("symlink");
        assert_eq!(
            store
                .recover(workflow, job, identity.public_key())
                .expect_err("symlink"),
            RemoteSshIdentityError::InsecurePath
        );
        fs::remove_file(key_path).expect("remove fixture link");
        fs::rename(saved, key_path).expect("restore");
        for content in [Vec::new(), vec![b'x'; 4097]] {
            fs::write(key_path, content).expect("corrupt fixture");
            assert_eq!(
                store
                    .recover(workflow, job, identity.public_key())
                    .expect_err("corrupt"),
                RemoteSshIdentityError::InvalidIdentity
            );
        }
    }

    #[test]
    fn missing_store_recovery_is_read_only_and_directory_symlinks_are_rejected() {
        let (directory, store, workflow, job) = fixture();
        let key = store.prepare_new(workflow, job).expect("fixture key");
        let missing = directory.path().join("missing");
        let missing_store = RemoteSshIdentityStore::new(&HorizonHome::from_root(missing.clone()));
        assert_eq!(
            missing_store
                .recover(workflow, job, key.public_key())
                .expect_err("missing"),
            RemoteSshIdentityError::Missing
        );
        assert!(!missing.exists());
        let linked = directory.path().join("linked");
        symlink(directory.path().join("home"), &linked).expect("linked home");
        let linked_store = RemoteSshIdentityStore::new(&HorizonHome::from_root(linked));
        assert_eq!(
            linked_store.prepare_new(workflow, job).expect_err("linked home"),
            RemoteSshIdentityError::InsecurePath
        );
    }

    #[test]
    fn invalid_ids_fail_before_writes_and_public_home_permissions_do_not_weaken_key_privacy() {
        let (directory, store, workflow, job) = fixture();
        let nil = "00000000-0000-0000-0000-000000000000";
        assert_eq!(
            store
                .prepare_new(nil.parse().expect("nil workflow"), job)
                .expect_err("nil"),
            RemoteSshIdentityError::InvalidIdentity
        );
        assert_eq!(
            store
                .prepare_new(workflow, nil.parse().expect("nil job"))
                .expect_err("nil"),
            RemoteSshIdentityError::InvalidIdentity
        );
        let home = directory.path().join("home");
        assert!(!home.exists());
        fs::create_dir(&home).expect("home");
        fs::set_permissions(&home, fs::Permissions::from_mode(0o755)).expect("public directory");
        let identity = store.prepare_new(workflow, job).expect("private child of public home");
        assert_eq!(
            fs::metadata(identity.private_key_path())
                .expect("key")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::set_permissions(&home, fs::Permissions::from_mode(0o777)).expect("writable directory");
        assert_eq!(
            store
                .recover(workflow, job, identity.public_key())
                .expect_err("writable home"),
            RemoteSshIdentityError::InsecurePath
        );
    }
}

#[cfg(not(target_os = "linux"))]
#[test]
fn unsupported_platform_never_writes_an_unprotected_key() {
    let directory = tempfile::tempdir().expect("fixture");
    let home = directory.path().join("untouched");
    let store = RemoteSshIdentityStore::new(&HorizonHome::from_root(home.clone()));
    let workflow = CloudWorkflowId::new();
    let job = CloudJobId::new();
    assert_eq!(
        store.prepare_new(workflow, job).expect_err("unsupported"),
        RemoteSshIdentityError::UnsupportedPlatform
    );
    assert_eq!(
        store.recover(workflow, job, "key").expect_err("unsupported"),
        RemoteSshIdentityError::UnsupportedPlatform
    );
    assert!(!home.exists());
}
