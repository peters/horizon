//! Lock discovery, clones and their bound, against untrusted lock files and addresses.
use super::locks::*;
use super::mirrors::*;
use super::watch::*;
use super::*;
use horizon_core::cloud_runtime::Cancellation;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn run(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn commit(path: &Path) {
    run(path, &["add", "."]);
    run(
        path,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "-m",
            "x",
        ],
    );
}

const SHA: &str = "9b9bc5aa9b9bc5aa9b9bc5aa9b9bc5aa9b9bc5aa";

#[test]
fn the_committed_lock_files_name_their_github_repositories_once() {
    let repository = tempfile::tempdir().unwrap();
    let path = repository.path();
    run(path, &["init", "-q"]);
    std::fs::create_dir_all(path.join("wasm/deep")).unwrap();
    let lock = format!(
        "[[package]]\nname = \"nodkey-proto\"\nsource = \"git+https://github.com/peters/nodkey-cli?rev=9b9b#{SHA}\"\n\
         [[package]]\nname = \"serde\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\
         [[package]]\nname = \"other\"\nsource = \"git+https://example.com/acme/other#{SHA}\"\n"
    );
    std::fs::write(path.join("Cargo.lock"), &lock).unwrap();
    std::fs::write(
        path.join("wasm/Cargo.lock"),
        format!("source = \"git+ssh://git@github.com/Peters/Nodkey-CLI.git#{SHA}\"\n"),
    )
    .unwrap();
    std::fs::write(
        path.join("wasm/deep/Cargo.lock"),
        format!("source = \"git+ssh://git@github.com/acme/deeper.git#{SHA}\"\n"),
    )
    .unwrap();
    std::fs::create_dir_all(path.join("apps/desktop/rust")).unwrap();
    std::fs::write(
        path.join("apps/desktop/rust/Cargo.lock"),
        format!("source = \"git+https://github.com/peters/deep-lib#{SHA}\"\n"),
    )
    .unwrap();
    commit(path);
    // What the agent writes after the commit is not read.
    std::fs::write(
        path.join("Cargo.lock"),
        format!("source = \"git+https://github.com/evil/added#{SHA}\"\n"),
    )
    .unwrap();
    assert_eq!(
        locked_repositories(path, "HEAD"),
        ["acme/deeper", "peters/deep-lib", "peters/nodkey-cli"],
        "every committed lock file, github.com only, once in any case"
    );
}

#[test]
fn a_withdrawn_approval_forgets_the_volume_until_a_proof_proves_it_did() {
    let access = |repositories: &[&str], forget_volume| Access {
        repositories: repositories.iter().map(|name| (*name).to_owned()).collect(),
        forget_volume,
        allowed: std::sync::Arc::default(),
    };
    let mut in_volume = Vec::new();
    access(&["peters/tools"], false).settle(&mut in_volume, (true, false));
    assert_eq!(in_volume, ["peters/tools"], "a started CI phase may have copied it");
    // The box is cleared: the next proof must forget the volume, and stops in Build.
    access(&[], true).settle(&mut in_volume, (false, false));
    assert_eq!(in_volume, ["peters/tools"], "nothing proved the volume went");
    // The one after it gets its CI phase through: the volume is new and holds nothing.
    access(&[], true).settle(&mut in_volume, (true, true));
    assert!(in_volume.is_empty());
}

#[test]
fn a_source_needs_github_plain_names_and_a_full_commit() {
    assert_eq!(
        git_source(&format!("source = \"git+https://github.com/../$(x)#{SHA}\"")),
        None
    );
    assert_eq!(git_source("source = \"git+https://github.com/o/tools#abc\""), None);
    assert_eq!(git_source(&format!("source = \"git+file:///etc/repo#{SHA}\"")), None);
    let source = git_source(&format!("source = \"git+ssh://git@github.com/acme/tools.git#{SHA}\"")).unwrap();
    assert_eq!(source.key(), "acme/tools");
    assert_eq!(source.address, "ssh://git@github.com/acme/tools.git");
    assert!(git_source(&format!("source = \"git+https://github.com/acme/.github#{SHA}\"")).is_some());
    // GitHub's limits: an owner of at most 39 characters, a name of at most 100.
    let source =
        |owner: &str, name: &str| git_source(&format!("source = \"git+https://github.com/{owner}/{name}#{SHA}\""));
    assert!(source(&"o".repeat(39), &"n".repeat(100)).is_some());
    assert_eq!(source(&"o".repeat(40), "tools"), None);
    assert_eq!(source("acme", &"n".repeat(101)), None);
    // The scheme and the host match in any case; the address stays as Cargo wrote it.
    let source = git_source(&format!("source = \"git+HTTPS://GitHub.com/acme/tools#{SHA}\"")).unwrap();
    assert_eq!(source.key(), "acme/tools");
    assert_eq!(source.address, "HTTPS://GitHub.com/acme/tools");
    assert!(git_source(&format!("source = \"git+SSH://git@GITHUB.COM/acme/tools#{SHA}\"")).is_some());
    assert_eq!(
        git_source(&format!("source = \"git+https://user@github.com/acme/tools#{SHA}\"")),
        None
    );
    assert_eq!(
        git_source(&format!("source = \"git+https://github.com.evil/acme/tools#{SHA}\"")),
        None
    );
    assert_eq!(
        git_source(&format!("source = \"git+https://github.com/acme/..#{SHA}\"")),
        None
    );
}

#[test]
fn only_approved_repositories_are_cloned_once_and_a_failure_leaves_the_others() {
    let repository = tempfile::tempdir().unwrap();
    let path = repository.path();
    run(path, &["init", "-q"]);
    let other = SHA.replace('9', "7");
    std::fs::write(
        path.join("Cargo.lock"),
        format!(
            "source = \"git+https://github.com/peters/tools#{SHA}\"\n\
             source = \"git+ssh://git@github.com/Peters/tools.git#{other}\"\n\
             source = \"git+https://github.com/peters/gone#{SHA}\"\n\
             source = \"git+https://github.com/victim/private#{SHA}\"\n"
        ),
    )
    .unwrap();
    commit(path);
    let parent = tempfile::tempdir().unwrap();
    let approved = ["peters/tools".to_owned(), "peters/gone".to_owned()];
    let mut cloned = Vec::new();
    let mirrors = Mirrors::clone(
        (path, "HEAD", parent.path(), &Cancellation::default()),
        (&approved, &AtomicBool::new(true)),
        |command| {
            let text = format!("{command:?}");
            cloned.push(text.clone());
            if text.contains("gone") {
                Err("not found".to_owned())
            } else {
                Ok(())
            }
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(cloned.len(), 2, "peters/tools once, then peters/gone");
    assert!(cloned.iter().all(|text| text.contains("protocol.allow=never")));
    assert!(!cloned.iter().any(|text| text.contains("victim")));
    assert!(
        mirrors
            .notes
            .iter()
            .any(|note| note.starts_with("victim/private not cloned here"))
    );
    assert!(
        mirrors
            .notes
            .iter()
            .any(|note| note.starts_with("peters/gone not cloned"))
    );
    assert!(mirrors.notes.iter().any(|note| note.starts_with("peters/tools lacks")));
    let unchecked = Mirrors::clone(
        (path, "HEAD", parent.path(), &Cancellation::default()),
        (&[], &AtomicBool::new(true)),
        |_| Ok(()),
    )
    .unwrap()
    .unwrap();
    assert!(unchecked.clones.is_empty());
    assert!(
        unchecked.notes.iter().any(|note| note.contains("is not checked")),
        "the log says why"
    );
}

#[test]
fn the_container_reads_clones_read_only_and_a_longer_address_keeps_its_own() {
    let mirrors = Mirrors {
        _folder: None,
        clones: vec![(
            "peters/nodkey-cli.git".to_owned(),
            PathBuf::from("/private/peters/nodkey-cli.git"),
            vec!["https://github.com/peters/nodkey-cli".to_owned()],
        )],
        others: vec![
            "https://github.com/acme/unrelated".to_owned(),
            "https://github.com/peters/nodkey-cli-extra".to_owned(),
        ],
        notes: Vec::new(),
        trust: None,
    };
    let args = mirrors.docker_args().join(" ");
    assert!(args.contains("-v /private/peters/nodkey-cli.git:/opt/host-cargo-git/peters/nodkey-cli.git:ro"));
    assert!(args.contains("GIT_CONFIG_KEY_0=url.file:///opt/host-cargo-git/peters/nodkey-cli.git.insteadOf"));
    assert!(args.contains("GIT_CONFIG_VALUE_0=https://github.com/peters/nodkey-cli"));
    assert!(args.contains("GIT_CONFIG_KEY_2=url.https://github.com/peters/nodkey-cli-extra.insteadOf"));
    assert!(
        !args.contains("acme/unrelated"),
        "an address no clone captures needs nothing"
    );
    assert!(args.contains("GIT_CONFIG_COUNT=3"));
    assert!(args.contains("CARGO_NET_GIT_FETCH_WITH_CLI=true"));
}

#[test]
fn a_folder_with_a_colon_is_not_mounted() {
    assert!(mountable(Path::new("/home/me/.horizon/cloud/setup-fetch/crates-1")));
    assert!(
        mountable(Path::new("/home/Last,First/.horizon")),
        "a comma parts nothing"
    );
    assert!(!mountable(Path::new("/home/me/a:b")));
    if cfg!(windows) {
        assert!(mountable(Path::new(r"C:\Users\me\crates-1")));
    }
}

// Unix only: it ages a folder through its handle, which Windows does not open.
#[test]
fn a_folder_past_its_bound_cancels_the_proof() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("pack"), vec![0_u8; 4096]).unwrap();
    let cancel = Cancellation::default();
    let watch = SizeWatch::start(folder.path(), 1024, cancel.clone());
    for _ in 0..40 {
        if cancel.is_cancelled() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(watch.stop(), "the folder passed its bound");
    assert!(cancel.is_cancelled(), "and the proof stops");
    let small = Cancellation::default();
    assert!(!SizeWatch::start(folder.path(), 1 << 20, small.clone()).stop());
    // A folder that grows past the bound between two looks is caught when watching stops.
    let late_cancel = Cancellation::default();
    let late = SizeWatch::start(folder.path(), 8192, late_cancel.clone());
    std::fs::write(folder.path().join("late"), vec![0_u8; 8192]).unwrap();
    assert!(late.stop());
    assert!(late_cancel.is_cancelled(), "the last look stops the proof too");
    assert!(!small.is_cancelled());
}

#[test]
fn sign_in_settings_keep_their_order_and_values() {
    let records = config_records(
        b"credential.helper\n\0credential.helper\n!gh auth git-credential\0http.proxy\nhttp://proxy:8080\0",
    );
    assert_eq!(
        records,
        [
            ("credential.helper".to_owned(), String::new()),
            ("credential.helper".to_owned(), "!gh auth git-credential".to_owned()),
            ("http.proxy".to_owned(), "http://proxy:8080".to_owned()),
        ]
    );
}

#[test]
fn a_rewritten_address_must_still_name_the_approved_repository() {
    assert_eq!(
        github_key("git@github.com:Peters/Tools.git").as_deref(),
        Some("peters/tools")
    );
    assert_eq!(
        github_key("ssh://git@github.com/peters/tools").as_deref(),
        Some("peters/tools")
    );
    assert_eq!(
        github_key("https://github.com/peters/tools.git").as_deref(),
        Some("peters/tools")
    );
    assert_eq!(
        github_key("https://TOKEN@GitHub.com/peters/tools").as_deref(),
        Some("peters/tools")
    );
    assert_eq!(github_key("https://example.com/peters/tools"), None);
    assert_eq!(github_key("https://github.com@example.com/peters/tools"), None);
    assert_eq!(github_key("https://github.com/peters/tools/extra"), None);
}

#[cfg(unix)]
#[test]
fn an_old_clone_folder_is_swept_and_a_new_one_kept() {
    let parent = tempfile::tempdir().unwrap();
    let old = parent.path().join("crates-old");
    let new = parent.path().join("crates-new");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::create_dir_all(&new).unwrap();
    let long_ago = std::time::SystemTime::now() - Duration::from_hours(48);
    std::fs::File::open(&old).unwrap().set_modified(long_ago).unwrap();
    sweep(parent.path());
    assert!(!old.exists());
    assert!(new.exists());
}

#[test]
fn a_token_in_a_rewritten_address_stays_off_the_command_line() {
    let (address, frozen) = clone_address("https://TOKEN@github.com/peters/tools", "peters/tools");
    assert_eq!(address, "https://github.com/peters/tools");
    assert_eq!(
        frozen,
        Some((
            "url.https://TOKEN@github.com/peters/tools.insteadOf".to_owned(),
            "https://github.com/peters/tools".to_owned()
        ))
    );
    let (address, frozen) = clone_address("git@github.com:peters/tools.git", "peters/tools");
    assert_eq!(address, "git@github.com:peters/tools.git");
    assert!(frozen.is_none());
}

#[test]
fn clearing_the_box_stops_the_clones_not_begun() {
    let repository = tempfile::tempdir().unwrap();
    let path = repository.path();
    run(path, &["init", "-q"]);
    std::fs::write(
        path.join("Cargo.lock"),
        format!("source = \"git+https://github.com/peters/first#{SHA}\"\nsource = \"git+https://github.com/peters/second#{SHA}\"\n"),
    )
    .unwrap();
    commit(path);
    let parent = tempfile::tempdir().unwrap();
    let approved = ["peters/first".to_owned(), "peters/second".to_owned()];
    let allowed = AtomicBool::new(true);
    let mut cloned = Vec::new();
    let mirrors = Mirrors::clone(
        (path, "HEAD", parent.path(), &Cancellation::default()),
        (&approved, &allowed),
        |command| {
            cloned.push(format!("{command:?}"));
            // The person clears the box while the first clone runs.
            allowed.store(false, Ordering::Relaxed);
            Ok(())
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(cloned.len(), 1, "no clone starts after the box is cleared");
    assert!(
        mirrors
            .notes
            .iter()
            .any(|note| note == "peters/second not cloned: the box was cleared")
    );
}

#[test]
fn a_repository_falls_back_to_its_other_locked_address() {
    let repository = tempfile::tempdir().unwrap();
    let path = repository.path();
    run(path, &["init", "-q"]);
    std::fs::write(
        path.join("Cargo.lock"),
        format!(
            "source = \"git+https://github.com/peters/tools#{SHA}\"\n\
             source = \"git+ssh://git@github.com/peters/tools.git#{SHA}\"\n"
        ),
    )
    .unwrap();
    commit(path);
    let parent = tempfile::tempdir().unwrap();
    let approved = ["peters/tools".to_owned()];
    let mut cloned = Vec::new();
    let mirrors = Mirrors::clone(
        (path, "HEAD", parent.path(), &Cancellation::default()),
        (&approved, &AtomicBool::new(true)),
        |command| {
            // Each clone runs in Horizon's folder, never seeing a repository around it.
            assert!(
                command
                    .get_current_dir()
                    .is_some_and(|folder| folder.starts_with(parent.path()))
            );
            assert!(
                command
                    .get_envs()
                    .any(|(name, value)| name == "GIT_CEILING_DIRECTORIES" && value.is_some())
            );
            assert!(
                command
                    .get_envs()
                    .any(|(name, value)| name == "GIT_DIR" && value.is_none())
            );
            cloned.push(format!("{command:?}"));
            // This computer signs in through only one of the two addresses: the first
            // address tried fails.
            if cloned.len() == 1 {
                Err("authentication failed".to_owned())
            } else {
                Ok(())
            }
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(cloned.len(), 2, "the other address is tried after the first fails");
    assert_ne!(cloned[0], cloned[1]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let folder = std::fs::read_dir(parent.path())
            .unwrap()
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().starts_with("crates-"))
            .unwrap();
        assert_eq!(
            folder.metadata().unwrap().permissions().mode() & 0o777,
            0o700,
            "only this user reaches the clones"
        );
    }
    // The fake clone writes nothing, so the commit check is what stops it here.
    assert!(
        mirrors.notes.iter().any(|note| note.starts_with("peters/tools lacks")),
        "{:?}",
        mirrors.notes
    );
    assert!(!mirrors.notes.iter().any(|note| note.contains("not cloned")));
}

#[test]
fn a_repository_around_horizon_s_folder_lends_no_rewrite() {
    let around = tempfile::tempdir().unwrap();
    run(around.path(), &["init", "-q"]);
    run(
        around.path(),
        &[
            "config",
            "url.https://elsewhere.invalid/.insteadOf",
            "https://github.com/peters/",
        ],
    );
    let folder = around.path().join("setup-fetch").join("crates-x");
    std::fs::create_dir_all(&folder).unwrap();
    let effective = effective_address("https://github.com/peters/tools", &folder, Duration::from_secs(60))
        .unwrap()
        .unwrap();
    assert!(!effective.contains("elsewhere.invalid"), "{effective}");
}

#[cfg(unix)]
#[test]
fn a_mirror_made_under_a_closed_umask_opens_to_the_container_user() {
    use std::os::unix::fs::PermissionsExt as _;
    let parent = tempfile::tempdir().unwrap();
    let mirror = parent.path().join("tools.git");
    std::fs::create_dir_all(mirror.join("objects/pack")).unwrap();
    std::fs::write(mirror.join("objects/pack/one.pack"), "pack").unwrap();
    std::os::unix::fs::symlink("/etc/hostname", mirror.join("link")).unwrap();
    for (path, mode) in [
        (mirror.join("objects/pack/one.pack"), 0o600),
        (mirror.join("objects/pack"), 0o700),
        (mirror.join("objects"), 0o700),
        (mirror.clone(), 0o700),
    ] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    readable_to_all(&mirror).unwrap();
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&mirror), 0o755);
    assert_eq!(mode(&mirror.join("objects/pack")), 0o755);
    assert_eq!(mode(&mirror.join("objects/pack/one.pack")), 0o644);
    assert_eq!(mode(parent.path()) & 0o077, 0, "the private parent stays closed");
}

#[test]
fn a_stopped_proof_starts_no_further_clone() {
    let repository = tempfile::tempdir().unwrap();
    let path = repository.path();
    run(path, &["init", "-q"]);
    std::fs::write(
        path.join("Cargo.lock"),
        format!(
            "source = \"git+https://github.com/peters/first#{SHA}\"\n\
             source = \"git+ssh://git@github.com/peters/first.git#{SHA}\"\n\
             source = \"git+https://github.com/peters/second#{SHA}\"\n"
        ),
    )
    .unwrap();
    commit(path);
    let parent = tempfile::tempdir().unwrap();
    let approved = ["peters/first".to_owned(), "peters/second".to_owned()];
    let cancel = Cancellation::default();
    let mut cloned = 0;
    let error = Mirrors::clone(
        (path, "HEAD", parent.path(), &cancel),
        (&approved, &AtomicBool::new(true)),
        |_| {
            cloned += 1;
            // The person presses Stop while the first clone runs.
            cancel.cancel();
            Err("stopped".to_owned())
        },
    )
    .err()
    .unwrap();
    assert_eq!(cloned, 1, "neither the other address nor the other repository is tried");
    assert_eq!(error, "the proof was stopped");
}

// Setting a folder's time opens the folder, which Windows allows only with a flag.
#[cfg(unix)]
#[test]
fn a_stale_clone_folder_goes_even_when_nothing_is_cloned() {
    let repository = tempfile::tempdir().unwrap();
    let path = repository.path();
    run(path, &["init", "-q"]);
    std::fs::write(path.join("README.md"), "no lock\n").unwrap();
    commit(path);
    let parent = tempfile::tempdir().unwrap();
    let stale = parent.path().join("crates-left");
    std::fs::create_dir(&stale).unwrap();
    let old = std::time::SystemTime::now() - Duration::from_hours(48);
    std::fs::File::open(&stale).unwrap().set_modified(old).unwrap();
    let mirrors = Mirrors::clone(
        (path, "HEAD", parent.path(), &Cancellation::default()),
        (&[], &AtomicBool::new(true)),
        |_| Ok(()),
    )
    .unwrap();
    assert!(mirrors.is_none());
    assert!(!stale.exists(), "the folder a stopped Horizon left is swept");
}

#[test]
fn git_in_the_container_trusts_clones_another_user_owns() {
    let folder = tempfile::tempdir().unwrap();
    let clones = vec![
        (
            "peters/horizon.git".to_owned(),
            folder.path().join("peters/horizon.git"),
            Vec::new(),
        ),
        (
            "acme/tools.git".to_owned(),
            folder.path().join("acme/tools.git"),
            Vec::new(),
        ),
    ];
    let path = trust(folder.path(), &clones).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.starts_with("[include]\n\tpath = /etc/gitconfig\n"),
        "the image's own settings stay: {text}"
    );
    assert!(text.contains("\tdirectory = /opt/host-cargo-git/peters/horizon.git\n"));
    assert!(text.contains("\tdirectory = /opt/host-cargo-git/acme/tools.git\n"));
    assert!(!text.contains('*'), "never every folder");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o444, 0o444);
    }
    let mirrors = Mirrors {
        _folder: None,
        clones,
        others: Vec::new(),
        notes: Vec::new(),
        trust: Some(path.clone()),
    };
    let args = mirrors.docker_args().join(" ");
    assert!(args.contains(&format!("-v {}:/opt/host-cargo-git.gitconfig:ro", path.display())));
    assert!(args.contains("-e GIT_CONFIG_SYSTEM=/opt/host-cargo-git.gitconfig"));
}

/// The clone belongs to this computer's user and the CI runs as 10001, as in a proof.
/// Needs Docker and an image with Git: run with `--ignored` and
/// `HORIZON_TEST_GIT_IMAGE` set, for example to Horizon's worker base.
#[cfg(unix)]
#[test]
#[ignore = "needs Docker and HORIZON_TEST_GIT_IMAGE"]
fn cargo_s_fetch_reads_a_clone_another_user_owns() {
    let image = std::env::var("HORIZON_TEST_GIT_IMAGE").expect("HORIZON_TEST_GIT_IMAGE names an image with Git");
    let source = tempfile::tempdir().unwrap();
    run(source.path(), &["init", "-q"]);
    std::fs::write(source.path().join("one"), "1").unwrap();
    commit(source.path());
    let locked = run(source.path(), &["rev-parse", "HEAD"]).trim().to_owned();
    std::fs::write(source.path().join("two"), "2").unwrap();
    commit(source.path());
    let folder = tempfile::tempdir().unwrap();
    let mirror = folder.path().join("peters/tools.git");
    let status = Command::new("git")
        .args(["clone", "--mirror", "--quiet", "--"])
        .arg(source.path())
        .arg(&mirror)
        .status()
        .unwrap();
    assert!(status.success());
    readable_to_all(&mirror).unwrap();
    let clones = vec![(
        "peters/tools.git".to_owned(),
        mirror,
        vec!["https://github.com/peters/tools".to_owned()],
    )];
    let trusted = trust(folder.path(), &clones).unwrap();
    // As Cargo fetches a locked commit that is not a branch tip.
    let script = format!(
        "cd /tmp && git init -q db && cd db && git fetch -q --no-tags --force --update-head-ok https://github.com/peters/tools '+{locked}:refs/commit/{locked}' && git cat-file -t {locked}"
    );
    let fetch = |trust: Option<PathBuf>| {
        let mirrors = Mirrors {
            _folder: None,
            clones: clones.clone(),
            others: Vec::new(),
            notes: Vec::new(),
            trust,
        };
        Command::new("docker")
            .args([
                "run",
                "--rm",
                "--user",
                "10001:10001",
                "--network",
                "none",
                "--entrypoint",
                "bash",
            ])
            .args(mirrors.docker_args())
            .args([image.as_str(), "-c", script.as_str()])
            .output()
            .unwrap()
    };
    let refused = fetch(None);
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("dubious ownership"),
        "the environment alone does not reach upload-pack: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
    let fetched = fetch(Some(trusted));
    assert!(fetched.status.success(), "{}", String::from_utf8_lossy(&fetched.stderr));
    assert_eq!(String::from_utf8_lossy(&fetched.stdout).trim(), "commit");
}

#[cfg(unix)]
#[test]
fn a_git_question_its_configuration_holds_fails_closed_in_time() {
    // A folder whose configuration includes a FIFO: every git command that reads it waits
    // for a writer that never comes.
    let folder = tempfile::tempdir().unwrap();
    run(folder.path(), &["init", "-q"]);
    let fifo = folder.path().join("waits");
    assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let config = folder.path().join(".git/config");
    let plain = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, format!("{plain}[include]\n\tpath = {}\n", fifo.display())).unwrap();
    let limit = Duration::from_millis(300);
    let started = std::time::Instant::now();
    let address = effective_address("https://github.com/peters/tools", folder.path(), limit);
    assert!(
        address.is_err(),
        "no address is trusted from Git that did not answer: {address:?}"
    );
    let settings = sign_in_settings(folder.path(), limit);
    assert!(
        settings.is_err(),
        "no sign-in settings from Git that did not answer: {settings:?}"
    );
    let held = holds_commit(folder.path(), &"a".repeat(40), limit);
    assert!(held.is_err(), "no commit is held by Git that did not answer: {held:?}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "each question ended at its limit: {:?}",
        started.elapsed()
    );
}
