use super::*;
#[test]
fn committed_transfer_excludes_dirty_working_tree_and_preserves_history() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.name", "Fixture"],
        vec!["config", "user.email", "fixture@example.invalid"],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    std::fs::write(repo.join("file.txt"), "committed\n").unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["add", "."])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["commit", "-m", "Create source fixture"])
            .output()
            .unwrap()
            .status
            .success()
    );
    std::fs::write(repo.join("file.txt"), "private dirty value\n").unwrap();
    std::fs::write(repo.join("untracked.txt"), "private untracked value").unwrap();
    let cancel = horizon_cloud::Cancellation::default();
    let emit = |_| {};
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: vec![],
    };
    let sha = resolve(&repo, "HEAD").unwrap();
    let export = snapshot(&repo, &sha, temp.path(), &runner).unwrap();
    assert_eq!(std::fs::read_to_string(export.join("file.txt")).unwrap(), "committed\n");
    assert!(!export.join("untracked.txt").exists());
    validate_tree(&repo, &sha, &runner).unwrap();
    pack(&repo, &sha, &temp.path().join("objects.pack"), &runner).unwrap();
    assert!(temp.path().join("objects.pack").metadata().unwrap().len() > 0);
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git").arg("-C").arg(repo).args(args).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
fn init(repo: &Path) {
    std::fs::create_dir_all(repo).unwrap();
    git(repo, &["init", "-b", "main"]);
    git(repo, &["config", "user.name", "Fixture"]);
    git(repo, &["config", "user.email", "fixture@example.invalid"]);
}
#[test]
fn selected_lfs_and_submodule_objects_are_verified_without_copying_dirty_files() {
    use sha2::{Digest, Sha256};
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    init(&repo);
    let module = repo.join("module");
    init(&module);
    std::fs::write(module.join("value.txt"), "committed module").unwrap();
    git(&module, &["add", "value.txt"]);
    git(&module, &["commit", "-m", "Create source fixture"]);
    let module_sha = git(&module, &["rev-parse", "HEAD"]);
    let content = b"selected binary content";
    let mut oid = String::with_capacity(64);
    for byte in Sha256::digest(content) {
        use std::fmt::Write as _;
        write!(oid, "{byte:02x}").unwrap();
    }
    let media = repo.join(".git/lfs/objects").join(&oid[..2]).join(&oid[2..4]);
    std::fs::create_dir_all(&media).unwrap();
    std::fs::write(media.join(&oid), content).unwrap();
    std::fs::write(
        repo.join("asset.bin"),
        format!(
            "version https://git-lfs.github.com/spec/v1\noid sha256:{oid}\nsize {}\n",
            content.len()
        ),
    )
    .unwrap();
    std::fs::write(
        repo.join("README"),
        "This documentation mentions version https://git-lfs.github.com/spec/v1 without being a pointer.",
    )
    .unwrap();
    std::fs::write(
        repo.join(".gitattributes"),
        "asset.bin filter=lfs diff=lfs merge=lfs -text\n",
    )
    .unwrap();
    std::fs::copy(repo.join("asset.bin"), repo.join("pointer-fixture.txt")).unwrap();
    git(
        &repo,
        &["add", ".gitattributes", "asset.bin", "README", "pointer-fixture.txt"],
    );
    git(
        &repo,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{module_sha},module"),
        ],
    );
    git(&repo, &["commit", "-m", "Pin source dependencies"]);
    std::fs::write(repo.join("asset.bin"), "dirty secret").unwrap();
    std::fs::write(repo.join(".gitattributes"), "asset.bin -filter\n").unwrap();
    std::fs::write(repo.join(".git/info/attributes"), "asset.bin -filter\n").unwrap();
    std::fs::write(module.join("value.txt"), "dirty module secret").unwrap();
    let cancel = horizon_cloud::Cancellation::default();
    let emit = |_| {};
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: vec![],
    };
    validate_tree(&repo, "HEAD", &runner).unwrap();
    let snapshot = snapshot(&repo, "HEAD", temp.path(), &runner).unwrap();
    assert_eq!(std::fs::read(snapshot.join("asset.bin")).unwrap(), content);
    assert!(
        std::fs::read_to_string(snapshot.join("pointer-fixture.txt"))
            .unwrap()
            .starts_with("version https://git-lfs.github.com/spec/v1\n")
    );
    assert_eq!(
        std::fs::read_to_string(snapshot.join("module/value.txt")).unwrap(),
        "committed module"
    );
    let root = temp.path().join("transfer");
    std::fs::create_dir(&root).unwrap();
    auxiliary(&repo, "HEAD", &root, &runner).unwrap();
    let manifest = std::fs::read_to_string(root.join("material/manifest.json")).unwrap();
    assert!(manifest.contains(&module_sha));
    assert!(!manifest.contains(repo.to_str().unwrap()));
    assert!(root.join("material/module-0.pack").metadata().unwrap().len() > 0);
    std::fs::write(media.join(&oid), b"damaged binary content").unwrap();
    assert!(validate_tree(&repo, "HEAD", &runner).is_err());
}

#[test]
fn oversized_lfs_pointer_is_rejected_but_ordinary_pointer_text_is_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    init(&repo);
    let bytes = format!(
        "version https://git-lfs.github.com/spec/v1\next-0-test {}\noid sha256:{}\nsize 1\n",
        "x".repeat(2048),
        "a".repeat(64)
    );
    std::fs::write(repo.join("ordinary.txt"), &bytes).unwrap();
    git(&repo, &["add", "ordinary.txt"]);
    git(&repo, &["commit", "-m", "Create ordinary text fixture"]);
    let cancel = horizon_cloud::Cancellation::default();
    let emit = |_| {};
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: vec![],
    };
    validate_tree(&repo, "HEAD", &runner).unwrap();
    std::fs::write(repo.join(".gitattributes"), "asset.bin filter=lfs\n").unwrap();
    git(&repo, &["add", ".gitattributes"]);
    let oid = git(&repo, &["hash-object", "-w", "ordinary.txt"]);
    git(
        &repo,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("100644,{oid},asset.bin"),
        ],
    );
    git(&repo, &["commit", "-m", "Pin extended source pointer"]);
    assert!(matches!(
        validate_tree(&repo, "HEAD", &runner),
        Err(Error::Invalid("Extended or malformed Git LFS pointers are unsupported"))
    ));
}

#[test]
fn large_ordinary_blob_with_later_lfs_attribute_is_not_treated_as_a_pointer() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    init(&repo);
    std::fs::write(repo.join("asset.bin"), vec![b'x'; 8 * 1024 * 1024]).unwrap();
    git(&repo, &["add", "asset.bin"]);
    git(&repo, &["commit", "-m", "Create large ordinary fixture"]);
    std::fs::write(repo.join(".gitattributes"), "asset.bin filter=lfs\n").unwrap();
    git(&repo, &["add", ".gitattributes"]);
    git(&repo, &["commit", "-m", "Declare later LFS tracking"]);
    // Packed objects exercise the path where libgit2 cannot stream a prefix.
    git(&repo, &["gc", "--prune=now"]);
    let cancel = horizon_cloud::Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: vec![],
    };
    validate_tree(&repo, "HEAD", &runner).unwrap();
}

#[test]
fn sha256_source_reports_unsupported_format_during_preflight() {
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["init", "--object-format=sha256"]);
    git(
        temp.path(),
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "Create source fixture",
        ],
    );
    let cancel = horizon_cloud::Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: vec![],
    };
    assert!(matches!(
        validate_tree(temp.path(), "HEAD", &runner),
        Err(Error::Invalid(
            "Cloud source export currently requires a SHA-1 Git repository"
        ))
    ));
}

#[test]
fn nested_non_utf8_files_and_directories_are_rejected_during_preflight() {
    for directory in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(temp.path()).unwrap();
        let blob = repo.blob(b"committed").unwrap();
        let (entry_id, mode) = if directory {
            let mut tree = repo.treebuilder(None).unwrap();
            tree.insert("file.txt", blob, 0o100_644).unwrap();
            (tree.write().unwrap(), 0o040_000)
        } else {
            (blob, 0o100_644)
        };
        // Git can contain paths the host filesystem cannot materialize.
        let mut nested = repo.treebuilder(None).unwrap();
        nested.insert(b"a\xff".as_slice(), entry_id, mode).unwrap();
        let mut root = repo.treebuilder(None).unwrap();
        root.insert("nested", nested.write().unwrap(), 0o040_000).unwrap();
        let tree = repo.find_tree(root.write().unwrap()).unwrap();
        let author = git2::Signature::now("Fixture", "fixture@example.invalid").unwrap();
        repo.commit(Some("HEAD"), &author, &author, "Create source fixture", &tree, &[])
            .unwrap();
        let cancel = horizon_cloud::Cancellation::default();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: vec![],
        };
        assert!(matches!(
            validate_tree(temp.path(), "HEAD", &runner),
            Err(Error::Invalid("Source paths must be UTF-8"))
        ));
    }
}
