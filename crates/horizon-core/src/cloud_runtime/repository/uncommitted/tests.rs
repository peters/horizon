use super::*;
use std::process::Command;

const SECRET: &str = "the name of a common secret file";

fn git(repository: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid"])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn write(repository: &Path, path: &str, text: &str) {
    let path = repository.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn commit(repository: &Path, message: &str) -> String {
    git(repository, &["add", "."]);
    git(repository, &["commit", "--quiet", "-m", message]);
    git(repository, &["rev-parse", "HEAD"])
}

/// A synthetic checkout with a committed recipe in `.horizon`, application code beside it
/// and a `.gitignore`, and the commit it is on.
fn checkout() -> (tempfile::TempDir, String) {
    let repository = tempfile::tempdir().unwrap();
    let path = repository.path();
    git(path, &["init", "--quiet"]);
    write(path, ".horizon/Dockerfile", "FROM scratch\n");
    write(path, ".horizon/setup.sh", "echo committed\n");
    write(path, ".horizon/.gitignore", "*.log\nscratch/\n");
    write(path, "src/main.rs", "fn main() {}\n");
    let revision = commit(path, "Recipe");
    (repository, revision)
}

fn recipe() -> Build {
    Build {
        context: ".horizon".into(),
        dockerfile: ".horizon/Dockerfile".into(),
        platform: "linux/amd64".into(),
    }
}

fn listed(files: &Uncommitted) -> Vec<(&str, Change)> {
    files
        .files()
        .iter()
        .map(|file| (file.path.as_str(), file.change))
        .collect()
}

/// A snapshot of the committed `.horizon` folder at HEAD, as `repository::snapshot`
/// extracts it.
fn snapshot(repository: &Path) -> tempfile::TempDir {
    let snapshot = tempfile::tempdir().unwrap();
    for name in git(repository, &["ls-tree", "-r", "--name-only", "HEAD", ".horizon"]).lines() {
        let committed = git(repository, &["show", &format!("HEAD:{name}")]);
        write(snapshot.path(), name, &format!("{committed}\n"));
    }
    snapshot
}

#[test]
fn the_context_takes_modified_and_new_files_and_never_ignored_ones() {
    let (repository, revision) = checkout();
    let path = repository.path();
    assert!(collect(path, &revision, &recipe()).unwrap().is_empty());
    assert_eq!(collect(path, &revision, &recipe()).unwrap().hash(), None);
    write(path, ".horizon/Dockerfile", "FROM scratch\nCOPY tool /\n");
    write(path, ".horizon/tool/run.sh", "echo new\n");
    std::fs::remove_file(path.join(".horizon/setup.sh")).unwrap();
    // Ignored through .gitignore and through the repository's own exclude file.
    write(path, ".horizon/build.log", "ignored\n");
    write(path, ".horizon/scratch/private.txt", "ignored\n");
    write(path, ".git/info/exclude", ".horizon/local.txt\n");
    write(path, ".horizon/local.txt", "ignored\n");
    // Outside the context, so not part of the image.
    write(path, "src/main.rs", "fn main() { changed(); }\n");
    write(path, "notes.txt", "untracked outside\n");
    let files = collect(path, &revision, &recipe()).unwrap();
    assert_eq!(
        listed(&files),
        [
            (".horizon/Dockerfile", Change::Modified),
            (".horizon/setup.sh", Change::Deleted),
            (".horizon/tool/run.sh", Change::New),
        ]
    );
    assert!(files.hash().is_some_and(|hash| valid_hash(&hash)));
}

#[test]
fn a_dockerfile_outside_the_context_and_staged_or_unstaged_files_count() {
    let (repository, revision) = checkout();
    let path = repository.path();
    let build = Build {
        context: "./.horizon/tool".into(),
        dockerfile: ".horizon/Dockerfile".into(),
        platform: "linux/amd64".into(),
    };
    write(path, ".horizon/Dockerfile", "FROM scratch\nCOPY . /\n");
    write(path, ".horizon/tool/staged.sh", "echo staged\n");
    git(path, &["add", ".horizon/tool/staged.sh"]);
    // Out of the index but still on disk: the commit's file with new content.
    git(path, &["rm", "--cached", "--quiet", ".horizon/setup.sh"]);
    write(path, ".horizon/setup.sh", "echo uncommitted\n");
    assert_eq!(
        listed(&collect(path, &revision, &build).unwrap()),
        [
            (".horizon/Dockerfile", Change::Modified),
            (".horizon/tool/staged.sh", Change::New),
        ]
    );
    let whole = Build {
        context: ".".into(),
        ..build
    };
    // Out of the index and ignored, the file on disk stays out: the commit's file is
    // deleted from the context.
    write(path, ".horizon/.gitignore", "*.log\nscratch/\nsetup.sh\n");
    let ignored = collect(path, &revision, &whole).unwrap();
    assert!(
        ignored
            .files()
            .iter()
            .any(|file| file.path == ".horizon/setup.sh" && file.change == Change::Deleted)
    );
    write(path, ".horizon/.gitignore", "*.log\nscratch/\n");
    assert_eq!(
        listed(&collect(path, &revision, &whole).unwrap()),
        [
            (".horizon/Dockerfile", Change::Modified),
            (".horizon/setup.sh", Change::Modified),
            (".horizon/tool/staged.sh", Change::New),
        ]
    );
}

#[test]
fn the_hash_binds_each_files_content() {
    let (repository, revision) = checkout();
    let path = repository.path();
    write(path, ".horizon/Dockerfile", "FROM scratch\nRUN true\n");
    let first = collect(path, &revision, &recipe()).unwrap();
    assert_eq!(
        collect(path, &revision, &recipe()).unwrap(),
        first,
        "the same files hash alike"
    );
    write(path, ".horizon/Dockerfile", "FROM scratch\nRUN false\n");
    let second = collect(path, &revision, &recipe()).unwrap();
    assert_eq!(listed(&second), listed(&first));
    assert_ne!(second.hash(), first.hash(), "same names, other content");
}

#[test]
fn the_overlay_builds_the_working_copy_and_refuses_unconfirmed_files() {
    let (repository, revision) = checkout();
    let path = repository.path();
    write(path, ".horizon/Dockerfile", "FROM scratch\nCOPY tool /\n");
    write(path, ".horizon/tool/run.sh", "echo new\n");
    // The working copy's .dockerignore goes into the context, so Docker applies it.
    write(path, ".horizon/.dockerignore", "tool/cache\n");
    std::fs::remove_file(path.join(".horizon/setup.sh")).unwrap();
    let files = collect(path, &revision, &recipe()).unwrap();
    let hash = files.hash().unwrap();

    let unconfirmed = snapshot(path);
    let error = overlay(path, &revision, &recipe(), None, unconfirmed.path()).unwrap_err();
    assert!(
        error.to_string().contains("changed after the rebuild was confirmed"),
        "{error}"
    );
    let other = "0".repeat(64);
    assert!(overlay(path, &revision, &recipe(), Some(&other), unconfirmed.path()).is_err());
    assert!(
        unconfirmed.path().join(".horizon/setup.sh").exists(),
        "nothing was laid over it"
    );

    let built = snapshot(path);
    overlay(path, &revision, &recipe(), Some(&hash), built.path()).unwrap();
    let read = |name: &str| std::fs::read_to_string(built.path().join(".horizon").join(name)).unwrap();
    assert_eq!(read("Dockerfile"), "FROM scratch\nCOPY tool /\n");
    assert_eq!(read("tool/run.sh"), "echo new\n");
    assert_eq!(read(".dockerignore"), "tool/cache\n");
    assert!(!built.path().join(".horizon/setup.sh").exists());

    // A file that changes after the confirmation is refused.
    write(path, ".horizon/tool/run.sh", "echo changed\n");
    let changed = snapshot(path);
    assert!(overlay(path, &revision, &recipe(), Some(&hash), changed.path()).is_err());
}

#[test]
fn a_folder_that_became_a_file_and_back_is_laid_over_cleanly() {
    let (repository, _) = checkout();
    let path = repository.path();
    write(path, ".horizon/tool/run.sh", "echo tracked\n");
    let revision = commit(path, "Tool");
    std::fs::remove_dir_all(path.join(".horizon/tool")).unwrap();
    write(path, ".horizon/tool", "now a file\n");
    std::fs::remove_file(path.join(".horizon/setup.sh")).unwrap();
    write(path, ".horizon/setup.sh/inner.sh", "now a folder\n");
    let files = collect(path, &revision, &recipe()).unwrap();
    let built = snapshot(path);
    overlay(path, &revision, &recipe(), files.hash().as_deref(), built.path()).unwrap();
    let read = |name: &str| std::fs::read_to_string(built.path().join(".horizon").join(name)).unwrap();
    assert_eq!(read("tool"), "now a file\n");
    assert_eq!(read("setup.sh/inner.sh"), "now a folder\n");
}

#[test]
fn common_secret_files_are_refused_unless_git_ignores_them() {
    for name in [
        ".env",
        ".env.local",
        "deploy.pem",
        "tls.key",
        "id_ed25519",
        "id_rsa_ci",
        "id_rsa.bak",
        "prod.env",
        ".npmrc",
        ".envrc",
        "vault.kdbx",
        "prod.tfvars",
        "terraform.tfstate",
        ".pgpass",
        "signing.keystore",
        "AuthKey.p8",
        ".aws/credentials",
        ".docker/config.json",
    ] {
        let (repository, revision) = checkout();
        write(repository.path(), &format!(".horizon/{name}"), "synthetic-secret\n");
        let error = collect(repository.path(), &revision, &recipe()).unwrap_err();
        assert!(error.to_string().contains(SECRET), "{name}: {error}");
        assert!(
            error.to_string().contains(&format!(".horizon/{name} ")),
            "names the file: {error}"
        );
    }
    let (repository, revision) = checkout();
    let path = repository.path();
    for name in [".env.example", "id_ed25519.pub", "keyboard.rs", "id_rsa_ci.pub"] {
        write(path, &format!(".horizon/{name}"), "synthetic\n");
    }
    write(path, ".horizon/.gitignore", "*.log\nscratch/\n.env\n");
    write(path, ".horizon/.env", "synthetic-secret\n");
    let files = collect(path, &revision, &recipe()).unwrap();
    let names: Vec<_> = files.files().iter().map(|file| file.path.as_str()).collect();
    assert_eq!(
        names,
        [
            ".horizon/.env.example",
            ".horizon/.gitignore",
            ".horizon/id_ed25519.pub",
            ".horizon/id_rsa_ci.pub",
            ".horizon/keyboard.rs",
        ]
    );
    // A tracked secret file changed in the working copy is refused too.
    write(path, ".horizon/.npmrc", "registry=https://registry.example\n");
    let revision = commit(path, "Registry");
    write(path, ".horizon/.npmrc", "//registry.example/:_authToken=synthetic\n");
    let error = collect(path, &revision, &recipe()).unwrap_err();
    assert!(error.to_string().contains(SECRET), "{error}");
}

#[test]
fn a_name_that_a_list_could_misshow_is_refused() {
    let (repository, revision) = checkout();
    write(repository.path(), ".horizon/safe\u{202e}hs.txt", "synthetic\n");
    let error = collect(repository.path(), &revision, &recipe()).unwrap_err();
    assert!(error.to_string().contains("text-direction"), "{error}");
    assert!(
        error.to_string().contains("\\u{202e}"),
        "the name is shown escaped: {error}"
    );
    std::fs::remove_file(repository.path().join(".horizon/safe\u{202e}hs.txt")).unwrap();
    write(repository.path(), ".horizon/x\u{200f}.sh", "synthetic\n");
    let error = collect(repository.path(), &revision, &recipe()).unwrap_err();
    assert!(error.to_string().contains("text-direction"), "{error}");
}

#[cfg(target_os = "linux")] // Only Linux file systems take any bytes in a name.
#[test]
fn a_name_that_is_not_utf8_refuses_only_inside_the_context() {
    use std::os::unix::ffi::OsStrExt;
    let (repository, revision) = checkout();
    let name = std::ffi::OsStr::from_bytes(b"notes-\xff.txt");
    std::fs::write(repository.path().join("src").join(name), "synthetic\n").unwrap();
    assert!(collect(repository.path(), &revision, &recipe()).unwrap().is_empty());
    std::fs::write(repository.path().join(".horizon").join(name), "synthetic\n").unwrap();
    let error = collect(repository.path(), &revision, &recipe()).unwrap_err();
    assert!(matches!(error, Error::Uncommitted(Refusal::Name(_))), "{error}");
}

#[test]
fn a_path_part_that_is_not_one_plain_name_is_never_read_or_removed() {
    let snapshot = tempfile::tempdir().unwrap();
    for path in ["../outside.txt", "a/../../outside.txt", "./a", "a//b", ""] {
        assert!(overlay::content(snapshot.path(), path).is_err(), "{path:?}");
    }
    #[cfg(windows)]
    for path in ["..\\outside.txt", "C:\\outside.txt", "a\\..\\..\\x"] {
        assert!(overlay::content(snapshot.path(), path).is_err(), "{path:?}");
    }
}

#[test]
fn a_configured_other_working_folder_is_refused() {
    let (repository, revision) = checkout();
    let elsewhere = tempfile::tempdir().unwrap();
    git(
        repository.path(),
        &["config", "core.worktree", elsewhere.path().to_str().unwrap()],
    );
    let error = collect(repository.path(), &revision, &recipe()).unwrap_err();
    assert!(error.to_string().contains("core.worktree"), "{error}");
}

#[test]
fn a_nested_repository_is_left_out_like_a_submodule() {
    let (repository, revision) = checkout();
    let nested = repository.path().join(".horizon/vendor");
    std::fs::create_dir_all(&nested).unwrap();
    git(&nested, &["init", "--quiet"]);
    write(&nested, "lib.rs", "pub fn nested() {}\n");
    assert!(collect(repository.path(), &revision, &recipe()).unwrap().is_empty());
}

#[cfg(unix)] // Links, hard links and the executable bit are Unix file system features.
mod unix {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn a_link_goes_in_as_a_link_and_its_target_is_never_read() {
        let (repository, revision) = checkout();
        let path = repository.path();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "private/key.txt", "synthetic-outside\n");
        symlink(outside.path().join("private"), path.join(".horizon/linked")).unwrap();
        symlink(outside.path().join("private/key.txt"), path.join(".horizon/file")).unwrap();
        let files = collect(path, &revision, &recipe()).unwrap();
        assert_eq!(
            listed(&files),
            [(".horizon/file", Change::New), (".horizon/linked", Change::New)]
        );
        let built = snapshot(path);
        overlay(path, &revision, &recipe(), files.hash().as_deref(), built.path()).unwrap();
        for name in ["file", "linked"] {
            let copied = built.path().join(".horizon").join(name);
            assert!(
                std::fs::symlink_metadata(&copied).unwrap().file_type().is_symlink(),
                "{name}"
            );
            assert_eq!(
                std::fs::read_link(&copied).unwrap(),
                std::fs::read_link(path.join(".horizon").join(name)).unwrap()
            );
        }
        // Another target is other content.
        std::fs::remove_file(path.join(".horizon/file")).unwrap();
        symlink("/elsewhere", path.join(".horizon/file")).unwrap();
        assert_ne!(collect(path, &revision, &recipe()).unwrap().hash(), files.hash());
    }

    #[test]
    fn a_folder_replaced_by_a_link_is_never_followed() {
        let (repository, _) = checkout();
        let path = repository.path();
        write(path, ".horizon/tool/run.sh", "echo tracked\n");
        let revision = commit(path, "Tool");
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "run.sh", "synthetic-outside\n");
        std::fs::remove_dir_all(path.join(".horizon/tool")).unwrap();
        symlink(outside.path(), path.join(".horizon/tool")).unwrap();
        let files = collect(path, &revision, &recipe()).unwrap();
        assert_eq!(
            listed(&files),
            [
                (".horizon/tool", Change::New),
                (".horizon/tool/run.sh", Change::Deleted)
            ]
        );
        assert!(matches!(
            overlay::content(path, ".horizon/tool/run.sh"),
            Err(Error::Uncommitted(Refusal::LinkedFolder(_)))
        ));
        let built = snapshot(path);
        overlay(path, &revision, &recipe(), files.hash().as_deref(), built.path()).unwrap();
        let copied = built.path().join(".horizon/tool");
        assert!(std::fs::symlink_metadata(copied).unwrap().file_type().is_symlink());
    }

    #[test]
    fn a_hard_link_is_refused_and_the_executable_bit_is_kept() {
        let (repository, revision) = checkout();
        let path = repository.path();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "secret.txt", "synthetic-outside\n");
        std::fs::hard_link(outside.path().join("secret.txt"), path.join(".horizon/copy.txt")).unwrap();
        let error = collect(path, &revision, &recipe()).unwrap_err();
        assert!(error.to_string().contains("hard link"), "{error}");
        std::fs::remove_file(path.join(".horizon/copy.txt")).unwrap();

        write(path, ".horizon/tool/run.sh", "echo new\n");
        let script = path.join(".horizon/tool/run.sh");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let executable = collect(path, &revision, &recipe()).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_ne!(collect(path, &revision, &recipe()).unwrap().hash(), executable.hash());
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let built = snapshot(path);
        overlay(path, &revision, &recipe(), executable.hash().as_deref(), built.path()).unwrap();
        let copied = std::fs::metadata(built.path().join(".horizon/tool/run.sh")).unwrap();
        assert_eq!(copied.permissions().mode() & 0o777, 0o755);
        assert_eq!(copied.modified().unwrap(), std::time::SystemTime::UNIX_EPOCH);
    }
}
