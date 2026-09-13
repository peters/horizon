use super::*;
use serde_json::json;
use std::path::Path;

fn request(parent: &Path) -> CheckpointRequest {
    serde_json::from_value(json!({"version":1,"preparation":{"version":1,"workspace_local_id":"synthetic",
        "runtime_id":"11111111-1111-4111-8111-111111111111","source":{"repository":"synthetic/project","commit":"1".repeat(40),"branch":null},"work_branch":"work"},
        "selected":["kept"],"complete_base_closure_consent":true,"retained_volume_attested":true,
        "parent":parent,"attempt_name":"first","max_retained_bytes":CAPACITY})).unwrap()
}

#[test]
fn strict_explicit_request_refuses_missing_consent_and_invalid_bounds_before_io() {
    let root = if cfg!(windows) {
        "C:/private/generations"
    } else {
        "/private/generations"
    };
    let valid = request(Path::new(root));
    assert!(valid.validate().is_ok());
    for (field, value) in [
        ("version", json!(3)),
        ("complete_base_closure_consent", json!(false)),
        ("retained_volume_attested", json!(false)),
        ("selected", json!([])),
        ("selected", json!(["kept", "kept"])),
        ("selected", json!(["../outside"])),
        ("attempt_name", json!("../outside")),
        ("max_retained_bytes", json!(CAPACITY + 1)),
        ("max_retained_bytes", json!(0)),
        ("parent", json!("relative")),
    ] {
        let mut wire = serde_json::to_value(&valid).unwrap();
        wire[field] = value;
        let request = serde_json::from_value(wire).unwrap();
        let failure = checkpoint_once(&request, || panic!("invalid request reached execution")).unwrap_err();
        assert_eq!(failure.reason, CheckpointError::Invalid);
        assert!(failure.retained.is_none());
    }
    let mut wire = serde_json::to_value(valid).unwrap();
    wire["unknown"] = json!(true);
    assert!(serde_json::from_value::<CheckpointRequest>(wire).is_err());
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::repository_overlay::{
        bundle::store::RepositoryBundleStore,
        capture::capture_selected_revision,
        seed::receive::{ExpectedGitPack, PackReceiveLimits, observe_git_base_pack},
    };
    use git2::{Repository, Signature};
    use std::{
        cell::Cell,
        collections::BTreeMap,
        fs,
        os::unix::fs::{PermissionsExt, symlink},
        path::Path,
    };

    #[test]
    fn typed_change_cancellation_and_capacity_are_not_unsupported_formats() {
        use super::super::linux::{capture_error, namespace_error};
        use crate::repository_overlay::{
            capture::GitCaptureError as Capture, namespace::NamespaceError as Namespace,
            reader::RepositoryReadError as Read, seed::SeedError as Seed,
        };
        for error in [Capture::Changed, Capture::Read(Read::Changed)] {
            assert_eq!(capture_error(error), CheckpointError::Changed);
        }
        assert_eq!(capture_error(Capture::Read(Read::TooLarge)), CheckpointError::Capacity);
        for (error, expected) in [
            (Namespace::Limit, CheckpointError::Capacity),
            (Namespace::Source(Seed::Limit), CheckpointError::Capacity),
            (Namespace::Source(Seed::Cancelled), CheckpointError::Cancelled),
            (Namespace::Source(Seed::Storage), CheckpointError::Storage),
            (Namespace::UnsupportedNode, CheckpointError::Unsupported),
        ] {
            assert_eq!(namespace_error(error), expected);
        }
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, CheckpointRequest) {
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let checkout = root.path().join("checkout");
        let parent = root.path().join("retained");
        for path in [&checkout, &parent] {
            fs::create_dir(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let repository = Repository::init(&checkout).unwrap();
        repository.set_head("refs/heads/work").unwrap();
        fs::write(checkout.join("kept"), b"base").unwrap();
        fs::write(checkout.join("unselected"), b"complete base content").unwrap();
        let mut index = repository.index().unwrap();
        index.add_path(Path::new("kept")).unwrap();
        index.add_path(Path::new("unselected")).unwrap();
        index.write().unwrap();
        let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
        let author = Signature::now("Fixture", "fixture@example.invalid").unwrap();
        let commit = repository
            .commit(Some("HEAD"), &author, &author, "Synthetic", &tree, &[])
            .unwrap();
        fs::write(checkout.join("kept"), b"staged").unwrap();
        index.add_path(Path::new("kept")).unwrap();
        index.write().unwrap();
        fs::write(checkout.join("kept"), b"working").unwrap();
        let mut request = request(&parent);
        request.preparation.source.commit = GitCommitSha::parse(commit.to_string()).unwrap();
        (root, checkout, request)
    }

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut result = BTreeMap::new();
        let mut paths = vec![root.to_owned()];
        while let Some(path) = paths.pop() {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    paths.push(entry.path());
                } else {
                    result.insert(
                        entry.path().strip_prefix(root).unwrap().to_path_buf(),
                        fs::read(entry.path()).unwrap(),
                    );
                }
            }
        }
        result
    }

    #[test]
    fn actual_generation_preserves_full_base_selected_layers_and_prior_attempts() {
        let (_root, checkout, mut request) = fixture();
        let inherited = request.parent.join("inherited");
        fs::create_dir(&inherited).unwrap();
        let file = inherited.join("file");
        fs::write(&file, b"retained").unwrap();
        fs::set_permissions(&inherited, fs::Permissions::from_mode(0o775)).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o664)).unwrap();
        let source = snapshot(&checkout);
        let first = super::super::linux::run(&request, &checkout, &|| Ok(()), &|| false).unwrap();
        assert_eq!(inherited.metadata().unwrap().permissions().mode() & 0o7777, 0o775);
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o7777, 0o664);
        assert_eq!(fs::read(&file).unwrap(), b"retained");
        let bytes = fs::read(first.path.join("generation.json")).unwrap();
        assert_eq!(ArtifactDigest::sha256(&bytes), first.manifest_sha256);
        assert!(serde_json::from_slice::<GenerationManifest>(&bytes).unwrap() == first.manifest);
        let manifest = &first.manifest;
        let pack = first
            .path
            .join("packs")
            .join(manifest.pack.sha256.as_str())
            .join("pack");
        let expected = ExpectedGitPack {
            base_commit: manifest.pack.base_commit.as_str().parse().unwrap(),
            sha256: &manifest.pack.sha256,
            encoded_bytes: manifest.pack.encoded_bytes,
        };
        observe_git_base_pack(&pack, expected, PackReceiveLimits::default(), || false).unwrap();
        let repository = Repository::open_bare(pack.join("decoded")).unwrap();
        let tree = repository.find_commit(expected.base_commit).unwrap().tree().unwrap();
        assert_eq!(
            repository
                .find_blob(tree.get_name("unselected").unwrap().id())
                .unwrap()
                .content(),
            b"complete base content"
        );
        let bundle = RepositoryBundleStore::open_named(&first.path.join("bundles"))
            .unwrap()
            .get(&manifest.overlay_manifest)
            .unwrap();
        assert_eq!(
            bundle,
            capture_selected_revision(&checkout, request.preparation.source.clone(), "work", &["kept"]).unwrap()
        );
        let retained = snapshot(&first.path);
        assert!(super::super::linux::run(&request, &checkout, &|| Ok(()), &|| false).is_err());
        request.attempt_name = "second".into();
        super::super::linux::run(&request, &checkout, &|| Ok(()), &|| false).unwrap();
        assert_eq!(snapshot(&first.path), retained);
        assert_eq!(snapshot(&checkout), source);
        let pack_file = pack.join("decoded/objects/pack");
        let file = fs::read_dir(pack_file)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == "pack"))
            .unwrap();
        fs::write(file, b"corrupt").unwrap();
        assert!(observe_git_base_pack(&pack, expected, PackReceiveLimits::default(), || false).is_err());
        assert_eq!(fs::read(first.path.join("generation.json")).unwrap(), bytes);
    }

    #[test]
    fn admission_rejects_low_capacity_overlap_links_and_cancellation_without_a_claim() {
        for case in 0..6 {
            let (root, checkout, mut request) = fixture();
            match case {
                0 => request.max_retained_bytes = ADMISSION_BYTES,
                1 => request.parent = checkout.join(".git"),
                2 => request.parent = root.path().to_path_buf(),
                3 => {
                    let alias = root.path().join("alias");
                    symlink(&request.parent, &alias).unwrap();
                    request.parent = alias;
                }
                5 => fs::set_permissions(&request.parent, fs::Permissions::from_mode(0o770)).unwrap(),
                _ => {}
            }
            let failure = super::super::linux::run(&request, &checkout, &|| Ok(()), &|| case == 4).unwrap_err();
            assert!(failure.retained.is_none());
            assert!(!request.parent.join("first").exists());
        }
    }

    #[test]
    fn unsupported_lfs_gitlinks_and_postclaim_identity_failure_keep_every_partial_stage() {
        for case in 0..3 {
            let (_root, checkout, request) = fixture();
            if case == 0 {
                fs::write(
                    checkout.join("kept"),
                    b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 1\n",
                )
                .unwrap();
            }
            if case == 1 {
                let repo = Repository::open(&checkout).unwrap();
                let mut index = repo.index().unwrap();
                let mut entry = index.get_path(Path::new("kept"), 0).unwrap();
                entry.mode = 0o160_000;
                index.add(&entry).unwrap();
                index.write().unwrap();
            }
            let checks = Cell::new(0);
            let failure = super::super::linux::run(
                &request,
                &checkout,
                &|| {
                    checks.set(checks.get() + 1);
                    if case == 2 && checks.get() > 1 {
                        Err(CheckpointError::Identity)
                    } else {
                        Ok(())
                    }
                },
                &|| false,
            )
            .unwrap_err();
            let attempt = request.parent.join("first");
            assert_eq!(failure.retained.as_deref(), Some(attempt.as_path()));
            assert!(!attempt.join("generation.json").exists());
            let before = snapshot(&attempt);
            assert!(super::super::linux::run(&request, &checkout, &|| Ok(()), &|| false).is_err());
            assert_eq!(snapshot(&attempt), before);
        }
    }
}
