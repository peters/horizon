use super::git::{Commands, Git};
use super::{
    GitPreparation as Request, GitPreparationError as Error, GitPreparationResponse as Response,
    GitPreparationState as State, GitSource, REQUEST_LIMIT, git, linux,
};

use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Command,
};

fn request() -> Request {
    Request {
        version: 1,
        workspace_local_id: "synthetic-workspace".into(),
        runtime_id: uuid::Uuid::new_v4(),
        source: GitSource {
            repository: "fixture/repository".into(),
            commit: crate::cloud_run::GitCommitSha::parse("a".repeat(40)).unwrap(),
            branch: Some("main".into()),
        },
        work_branch: "work/explicit".into(),
    }
}

fn roots() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for suffix in ["", ".horizon-worker", "horizon"] {
        let path = root.path().join(suffix);
        fs::create_dir_all(&path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    root
}

#[test]
fn task_binding_is_canonical_and_includes_every_preparation_field() {
    let original = request();
    let binding = original.binding().unwrap();
    assert_eq!(
        Request::decode(&original.encode().unwrap()).unwrap().binding().unwrap(),
        binding
    );
    for field in [
        "workspace",
        "runtime",
        "repository",
        "commit",
        "source_branch",
        "work_branch",
    ] {
        let mut changed = original.clone();
        match field {
            "workspace" => changed.workspace_local_id = "another-workspace".into(),
            "runtime" => changed.runtime_id = uuid::Uuid::new_v4(),
            "repository" => changed.source.repository = "fixture/another".into(),
            "commit" => changed.source.commit = crate::cloud_run::GitCommitSha::parse("b".repeat(40)).unwrap(),
            "source_branch" => changed.source.branch = Some("another".into()),
            _ => changed.work_branch = "work/another".into(),
        }
        assert_ne!(changed.binding().unwrap(), binding, "{field}");
    }
    let mut invalid = original;
    invalid.work_branch = "HEAD".into();
    assert_eq!(invalid.binding(), Err(Error::Invalid));
    assert_eq!(invalid.inspect_checkout(), Err(Error::Invalid));
}

#[test]
fn task_checkout_requires_exact_completion_without_git_or_dirty_file_changes() {
    let root = roots();
    let request = request();
    assert_eq!(linux::inspect_checkout(root.path(), &request), Err(Error::Conflict));
    assert!(!root.path().join(".horizon-worker/git-workspace").exists());
    let mut git = Fake::new();
    assert_eq!(execute(root.path(), &request, false, &mut git).state, State::Complete);
    let checkout = root.path().join("horizon/repository");
    fs::write(checkout.join("dirty"), b"uncommitted task bytes").unwrap();
    let before = git.calls;
    let location = linux::inspect_checkout(root.path(), &request).unwrap();
    let meta = checkout.metadata().unwrap();
    assert_eq!(
        (location.path, location.device, location.inode),
        (super::CHECKOUT, meta.dev(), meta.ino())
    );
    assert_eq!(git.calls, before);
    assert_eq!(fs::read(checkout.join("dirty")).unwrap(), b"uncommitted task bytes");
    let mut other = request.clone();
    other.work_branch = "different".into();
    assert_eq!(linux::inspect_checkout(root.path(), &other), Err(Error::Conflict));
    fs::rename(&checkout, root.path().join("horizon/retained")).unwrap();
    fs::create_dir(&checkout).unwrap();
    fs::set_permissions(&checkout, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(linux::inspect_checkout(root.path(), &request), Err(Error::Conflict));
    fs::remove_dir(&checkout).unwrap();
    symlink(root.path().join("horizon/retained"), &checkout).unwrap();
    assert_eq!(linux::inspect_checkout(root.path(), &request), Err(Error::UnsafeRoot));
}

#[test]
fn task_checkout_rejects_partial_corrupt_and_insecure_records() {
    for fault in ["partial", "completion", "claim", "mode", "ancestry"] {
        let root = roots();
        let request = request();
        let mut git = Fake::new();
        assert_eq!(execute(root.path(), &request, false, &mut git).state, State::Complete);
        let slot = root.path().join(".horizon-worker/git-workspace");
        match fault {
            "partial" => fs::remove_file(slot.join("complete.json")).unwrap(),
            "completion" => fs::write(slot.join("complete.json"), b"{}").unwrap(),
            "claim" => fs::write(slot.join("claim.json"), b"{}").unwrap(),
            "mode" => fs::set_permissions(
                root.path().join("horizon/repository"),
                fs::Permissions::from_mode(0o755),
            )
            .unwrap(),
            _ => fs::set_permissions(root.path(), fs::Permissions::from_mode(0o777)).unwrap(),
        }
        assert!(linux::inspect_checkout(root.path(), &request).is_err(), "{fault}");
        assert!(slot.exists());
    }
}

struct Fake {
    calls: usize,
    fail: Option<usize>,
    mode: &'static str,
}
impl Fake {
    fn new() -> Self {
        Self {
            calls: 0,
            fail: None,
            mode: "100644\n",
        }
    }
}
impl Commands for Fake {
    fn run(&mut self, _: &Path, args: &[&str], _: &[u8], _: bool, _: &dyn Fn() -> bool) -> Result<Vec<u8>, Error> {
        self.calls += 1;
        if self.fail == Some(self.calls) {
            return Err(Error::Git);
        }
        Ok(match args[0] {
            "rev-parse" => format!("{}\n", "a".repeat(40)).into_bytes(),
            "ls-tree" if args[1] == "-r" => self.mode.as_bytes().to_vec(),
            _ => vec![],
        })
    }
}

fn execute(root: &Path, request: &Request, observe: bool, git: &mut impl Commands) -> Response {
    linux::execute(root, request, observe, &|| false, git)
}

#[test]
fn strict_request_framing_identity_and_branch() {
    let original = request();
    let encoded = original.encode().unwrap();
    assert_eq!(Request::decode(&encoded).unwrap(), original);
    for bytes in [
        b"{}".to_vec(),
        vec![b' '; REQUEST_LIMIT + 1],
        [encoded.clone(), b"{}".to_vec()].concat(),
        String::from_utf8(encoded.clone())
            .unwrap()
            .replacen('{', "{\"version\":1,", 1)
            .into_bytes(),
    ] {
        assert_eq!(Request::decode(&bytes), Err(Error::Invalid));
    }
    for field in ["version", "workspace_local_id", "runtime_id", "source", "work_branch"] {
        let mut value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        value.as_object_mut().unwrap().remove(field);
        assert!(Request::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    for branch in ["-bad", "a..b", "a.lock", "x\ny", "", "refs/../bad", "HEAD"] {
        let mut changed = original.clone();
        changed.work_branch = branch.into();
        assert!(changed.encode().is_err());
    }
    let mut changed = original;
    changed.source.repository = "https://private@elsewhere/repo".into();
    assert!(changed.encode().is_err());
}

#[test]
fn reserved_head_is_rejected_before_worker_state_or_git() {
    let root = roots();
    let mut request = request();
    request.work_branch = "HEAD".into();
    assert_eq!(
        Request::decode(&serde_json::to_vec(&request).unwrap()),
        Err(Error::Invalid)
    );
    let response = super::prepare(&request, || panic!("invalid request must not enter preparation"));
    assert_eq!(
        (response.state, response.reason, response.exit_code()),
        (State::Error, Some(Error::Invalid), 2)
    );
    let mut git = Fake::new();
    let response = execute(root.path(), &request, false, &mut git);
    assert_eq!((response.state, response.reason), (State::Error, Some(Error::Invalid)));
    assert_eq!(git.calls, 0);
    for directory in [".horizon-worker", "horizon"] {
        assert!(fs::read_dir(root.path().join(directory)).unwrap().next().is_none());
    }
    request.work_branch = "work/explicit".into();
    request.source.branch = Some("HEAD".into());
    assert!(request.encode().is_ok());
}

#[test]
fn observation_absent_is_read_only_and_repeat_preserves_dirty_checkout() {
    let root = roots();
    let request = request();
    let mut git = Fake::new();
    assert_eq!(execute(root.path(), &request, true, &mut git).state, State::Absent);
    assert!(!root.path().join(".horizon-worker/git-workspace").exists());
    assert_eq!(execute(root.path(), &request, false, &mut git).state, State::Complete);
    let calls = git.calls;
    let user = root.path().join("horizon/repository/user-change");
    fs::write(&user, "retained user bytes").unwrap();
    for observe in [true, false] {
        assert_eq!(execute(root.path(), &request, observe, &mut git).state, State::Complete);
    }
    assert_eq!(git.calls, calls);
    assert_eq!(fs::read_to_string(user).unwrap(), "retained user bytes");
    let mut conflicting = request;
    conflicting.runtime_id = uuid::Uuid::new_v4();
    assert_eq!(
        execute(root.path(), &conflicting, false, &mut git).reason,
        Some(Error::Conflict)
    );
    assert_eq!(git.calls, calls);
}

#[test]
fn every_failed_command_is_a_permanent_replay_barrier() {
    for failure in 1..=10 {
        let root = roots();
        let request = request();
        let mut git = Fake::new();
        git.fail = Some(failure);
        let result = execute(root.path(), &request, false, &mut git);
        assert_eq!(result.state, State::ClaimedUnknown);
        assert_eq!(result.reason, Some(Error::Git));
        let calls = git.calls;
        git.fail = None;
        assert_eq!(
            execute(root.path(), &request, false, &mut git).state,
            State::ClaimedUnknown
        );
        assert_eq!(git.calls, calls);
    }
}

#[test]
fn missing_or_corrupt_claim_never_grants_replay() {
    for claim in [None, Some("{}"), Some("private-invalid-claim")] {
        let root = roots();
        let mut git = Fake::new();
        let slot = root.path().join(".horizon-worker/git-workspace");
        fs::create_dir(&slot).unwrap();
        fs::set_permissions(&slot, fs::Permissions::from_mode(0o700)).unwrap();
        if let Some(claim) = claim {
            fs::write(slot.join("claim.json"), claim).unwrap();
        }
        assert_eq!(
            execute(root.path(), &request(), false, &mut git).reason,
            Some(Error::Conflict)
        );
        assert_eq!(git.calls, 0);
    }
}

#[test]
fn unsafe_roots_preexisting_checkout_and_replacement_are_not_adopted() {
    for relative in [".horizon-worker", "horizon"] {
        let root = roots();
        let mut git = Fake::new();
        fs::set_permissions(root.path().join(relative), fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(
            execute(root.path(), &request(), false, &mut git).reason,
            Some(Error::UnsafeRoot)
        );
        assert_eq!(git.calls, 0);
    }
    for link in [false, true] {
        let root = roots();
        let mut git = Fake::new();
        let checkout = root.path().join("horizon/repository");
        if link {
            symlink(root.path(), &checkout).unwrap();
        } else {
            fs::create_dir(&checkout).unwrap();
        }
        assert_eq!(
            execute(root.path(), &request(), false, &mut git).reason,
            Some(Error::Conflict)
        );
        assert_eq!(git.calls, 0);
        assert!(fs::symlink_metadata(checkout).is_ok());
    }
    let root = roots();
    let mut git = Fake::new();
    let request = request();
    assert_eq!(execute(root.path(), &request, false, &mut git).state, State::Complete);
    let checkout = root.path().join("horizon/repository");
    fs::rename(&checkout, root.path().join("retained-original")).unwrap();
    fs::create_dir(&checkout).unwrap();
    fs::set_permissions(&checkout, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        execute(root.path(), &request, true, &mut git).reason,
        Some(Error::Conflict)
    );
}

#[test]
fn submodules_fail_explicitly_before_checkout() {
    let root = roots();
    let mut git = Fake::new();
    git.mode = "100644\n160000\n";
    let result = execute(root.path(), &request(), false, &mut git);
    assert_eq!(result.reason, Some(Error::UnsupportedRepository));
    assert_eq!(git.calls, 5);
}

fn local_git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("/usr/bin/git")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args([
            "-c",
            "user.name=Synthetic",
            "-c",
            "user.email=synthetic@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "synthetic Git fixture failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

struct Local {
    git: Git,
    source: PathBuf,
}
impl Commands for Local {
    fn run(
        &mut self,
        directory: &Path,
        args: &[&str],
        input: &[u8],
        missing: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, Error> {
        if args[0] == "fetch" {
            // Test-only transport substitution. All other commands use the actual runner.
            self.git.run(
                directory,
                &[
                    "-c",
                    "protocol.file.allow=always",
                    "fetch",
                    self.source.to_str().unwrap(),
                    args[5],
                ],
                input,
                missing,
                cancelled,
            )
        } else {
            self.git.run(directory, args, input, missing, cancelled)
        }
    }
}

#[test]
fn actual_git_exact_commit_explicit_branch_and_lfs_rejection() {
    for kind in ["plain", "filter", "pointer", "lfsconfig"] {
        let source = tempfile::tempdir().unwrap();
        local_git(source.path(), &["init", "--template=", "--initial-branch=main"]);
        fs::write(source.path().join("tracked"), "first bytes").unwrap();
        match kind {
            "filter" => fs::write(source.path().join(".gitattributes"), "tracked filter=lfs\n").unwrap(),
            "pointer" => fs::write(
                source.path().join("tracked"),
                "version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 1\n",
            )
            .unwrap(),
            "lfsconfig" => fs::write(
                source.path().join(".lfsconfig"),
                "[lfs]\nurl = https://untrusted.invalid\n",
            )
            .unwrap(),
            _ => {}
        }
        local_git(source.path(), &["add", "."]);
        local_git(source.path(), &["commit", "-m", "Synthetic initial"]);
        let mut request = request();
        request.source.commit =
            crate::cloud_run::GitCommitSha::parse(local_git(source.path(), &["rev-parse", "HEAD"])).unwrap();
        fs::write(source.path().join("later"), "moving branch must not select these bytes").unwrap();
        local_git(source.path(), &["add", "."]);
        local_git(source.path(), &["commit", "-m", "Synthetic later"]);
        let root = roots();
        let mut git = Local {
            git: Git::new(),
            source: source.path().to_owned(),
        };
        let result = execute(root.path(), &request, false, &mut git);
        if kind == "plain" {
            assert_eq!(result.reason, None);
            assert_eq!(result.state, State::Complete);
            let checkout = root.path().join("horizon/repository");
            assert_eq!(
                local_git(&checkout, &["rev-parse", "HEAD"]),
                request.source.commit.as_str()
            );
            assert_eq!(local_git(&checkout, &["branch", "--show-current"]), request.work_branch);
            assert!(!checkout.join("later").exists());
            assert_eq!(fs::metadata(checkout).unwrap().mode() & 0o777, 0o700);
        } else {
            assert_eq!(result.reason, Some(Error::UnsupportedRepository), "{kind}");
        }
    }
}

#[test]
fn command_has_no_inherited_configuration_or_token_and_deadline_prevents_spawn() {
    let command = git::environment(Path::new("/synthetic"));
    let env: Vec<_> = command.get_envs().collect();
    assert!(!env.iter().any(|(key, _)| key.to_str().unwrap().contains("TOKEN")));
    assert!(
        env.iter()
            .any(|(key, value)| *key == "GIT_CONFIG_GLOBAL" && *value == Some(std::ffi::OsStr::new("/dev/null")))
    );
    let args: Vec<_> = command.get_args().map(|arg| arg.to_str().unwrap()).collect();
    assert!(args.contains(&"http.followRedirects=false"));
    assert!(args.contains(&"protocol.allow=never"));
    assert!(args.contains(&"credential.helper=/usr/local/bin/horizon-github-credential"));
    let root = roots();
    let mut git = Git::new();
    assert_eq!(
        git.run(root.path(), &["version"], &[], false, &|| true),
        Err(Error::Interrupted)
    );
}
