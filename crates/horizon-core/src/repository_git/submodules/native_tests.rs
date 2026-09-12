use super::*;
use crate::repository_git::{
    GitPreparationState as State,
    git::{Commands, Git},
    linux,
    tests::{local_git, request, roots},
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    time::{Duration, Instant},
};

const PAYLOAD: &[u8] = b"synthetic nested LFS bytes";

fn source(child: Option<(&str, &str)>, lfs: bool) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    local_git(root.path(), &["init", "--template=", "--initial-branch=main"]);
    if lfs {
        fs::write(root.path().join(".gitattributes"), "tracked filter=lfs\n").unwrap();
        fs::write(
            root.path().join("tracked"),
            format!(
                "version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize {}\n",
                crate::cloud_run::ArtifactDigest::sha256(PAYLOAD).as_str(),
                PAYLOAD.len()
            ),
        )
        .unwrap();
    } else {
        fs::write(root.path().join("tracked"), "retained synthetic bytes").unwrap();
    }
    if let Some((url, sha)) = child {
        fs::write(
            root.path().join(".gitmodules"),
            format!("[submodule \"child\"]\npath = nested/child\nurl = {url}\nbranch = wrong-tip\n"),
        )
        .unwrap();
        local_git(
            root.path(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{sha},nested/child"),
            ],
        );
    }
    local_git(root.path(), &["add", "tracked"]);
    if lfs {
        local_git(root.path(), &["add", ".gitattributes"]);
    }
    if child.is_some() {
        local_git(root.path(), &["add", ".gitmodules"]);
    }
    local_git(root.path(), &["commit", "-m", "Synthetic recorded source"]);
    root
}

struct Sources {
    git: Git,
    by_commit: BTreeMap<String, PathBuf>,
    fetches: usize,
    fail_at: Option<usize>,
    fail_lfs: bool,
    delay_at: Option<usize>,
    calls: usize,
    lfs_repositories: Vec<String>,
    last_command: Vec<String>,
    last_output: Vec<u8>,
}

impl Sources {
    fn new(sources: &[&tempfile::TempDir]) -> Self {
        Self {
            git: Git::new(),
            by_commit: sources
                .iter()
                .map(|source| {
                    (
                        local_git(source.path(), &["rev-parse", "HEAD"]),
                        source.path().to_owned(),
                    )
                })
                .collect(),
            fetches: 0,
            fail_at: None,
            fail_lfs: false,
            delay_at: None,
            calls: 0,
            lfs_repositories: Vec::new(),
            last_command: Vec::new(),
            last_output: Vec::new(),
        }
    }
}

impl Commands for Sources {
    fn run(
        &mut self,
        directory: &Path,
        args: &[&str],
        input: &[u8],
        missing: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, Error> {
        self.calls += 1;
        self.last_command = args.iter().map(|s| (*s).to_owned()).collect();
        if args[0] == "fetch" {
            self.fetches += 1;
            assert_eq!(
                &args[1..5],
                &["--no-tags", "--no-recurse-submodules", "--depth=1", "origin"]
            );
            if self.fail_at == Some(self.fetches) {
                return Err(Error::Git);
            }
            if self.delay_at == Some(self.fetches) {
                std::thread::sleep(Duration::from_millis(600));
            }
            let source = self.by_commit.get(args[5]).ok_or(Error::Git)?;
            // Local fixture transport only, matching the existing ordinary-Git tests.
            // No production protocol allowlist or HTTPS endpoint is changed.
            self.git.run(
                directory,
                &[
                    "-c",
                    "protocol.file.allow=always",
                    "fetch",
                    source.to_str().unwrap(),
                    args[5],
                ],
                input,
                missing,
                cancelled,
            )
        } else {
            let result = self.git.run(directory, args, input, missing, cancelled)?;
            self.last_output.clone_from(&result);
            if args[0] == "checkout" {
                // Fixed synthetic tracked file only: model entrypoint umask077,
                // as the existing LFS integration fixture does, without global umask changes.
                let path = directory.join("tracked");
                assert!(fs::symlink_metadata(&path).unwrap().is_file());
                fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
            }
            Ok(result)
        }
    }

    fn smudge(
        &mut self,
        _: &Path,
        request: &GitPreparation,
        _: &lfs::Pointer,
        _: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, Error> {
        self.lfs_repositories.push(request.source.repository.clone());
        if self.fail_lfs {
            return Err(Error::Git);
        }
        Ok(PAYLOAD.to_vec())
    }
}

#[test]
fn actual_git_nested_recorded_commits_lfs_and_existing_dirty_receipt() {
    let leaf = source(None, true);
    let leaf_sha = local_git(leaf.path(), &["rev-parse", "HEAD"]);
    let child = source(Some(("../leaf.git", &leaf_sha)), false);
    let child_sha = local_git(child.path(), &["rev-parse", "HEAD"]);
    let parent = source(Some(("https://github.com/fixture/child.git", &child_sha)), false);
    let mut transport = Sources::new(&[&parent, &child, &leaf]);
    let mut request = request();
    request.source.commit = GitCommitSha::parse(local_git(parent.path(), &["rev-parse", "HEAD"])).unwrap();
    // Branch tips move independently; the gitlinks remain authoritative.
    for source in [&parent, &child] {
        fs::write(source.path().join("later"), b"not selected").unwrap();
        local_git(source.path(), &["add", "later"]);
        local_git(source.path(), &["commit", "-m", "Synthetic later tip"]);
    }
    let root = roots();
    let result = linux::execute(root.path(), &request, false, &|| false, &mut transport);
    assert_eq!(
        (result.state, result.reason),
        (State::Complete, None),
        "last synthetic command {:?}, output {:?}",
        transport.last_command,
        String::from_utf8_lossy(&transport.last_output)
    );
    assert_eq!(transport.fetches, 3);
    assert_eq!(transport.lfs_repositories, ["fixture/leaf"]);
    let checkout = root.path().join("horizon/repository");
    for (path, sha) in [
        (&checkout, request.source.commit.as_str()),
        (&checkout.join("nested/child"), &child_sha),
        (&checkout.join("nested/child/nested/child"), &leaf_sha),
    ] {
        assert_eq!(local_git(path, &["rev-parse", "HEAD"]), sha);
        assert!(local_git(path, &["status", "--porcelain", "--ignore-submodules=none"]).is_empty());
        assert!(!path.join("later").exists());
    }
    assert!(local_git(&checkout.join("nested/child"), &["branch", "--show-current"]).is_empty());
    assert_eq!(
        fs::read(checkout.join("nested/child/nested/child/tracked")).unwrap(),
        PAYLOAD
    );
    assert!(local_git(&checkout, &["submodule", "status", "--recursive"]).contains(&leaf_sha));
    fs::write(checkout.join("nested/child/tracked"), b"dirty child").unwrap();
    fs::write(checkout.join("nested/child/untracked"), b"untracked child").unwrap();
    let calls = transport.calls;
    for observe in [true, false] {
        assert_eq!(
            linux::execute(root.path(), &request, observe, &|| false, &mut transport).state,
            State::Complete
        );
        assert_eq!(transport.calls, calls);
    }
    assert_eq!(fs::read(checkout.join("nested/child/tracked")).unwrap(), b"dirty child");
    assert_eq!(
        fs::read(checkout.join("nested/child/untracked")).unwrap(),
        b"untracked child"
    );
}

#[test]
fn missing_private_child_and_shared_deadline_never_complete_or_replay() {
    for fault in ["git-denied", "lfs-denied", "deadline"] {
        let child = source(None, fault == "lfs-denied");
        let parent = source(
            Some(("../child.git", &local_git(child.path(), &["rev-parse", "HEAD"]))),
            false,
        );
        let mut transport = Sources::new(&[&parent, &child]);
        if fault == "deadline" {
            transport.git = Git::with_timeout(Duration::from_millis(500));
            transport.delay_at = Some(2);
        } else if fault == "git-denied" {
            transport.fail_at = Some(2);
        } else {
            transport.fail_lfs = true;
        }
        let mut request = request();
        request.source.commit = GitCommitSha::parse(local_git(parent.path(), &["rev-parse", "HEAD"])).unwrap();
        let root = roots();
        let start = Instant::now();
        let result = linux::execute(root.path(), &request, false, &|| false, &mut transport);
        if fault == "deadline" {
            assert_eq!(result.reason, Some(Error::Interrupted));
        }
        if fault == "lfs-denied" {
            assert_eq!(transport.lfs_repositories, ["fixture/child"]);
        }
        assert_eq!(result.state, State::ClaimedUnknown);
        assert!(result.reason.is_some());
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(!root.path().join(".horizon-worker/git-workspace/complete.json").exists());
        assert!(linux::inspect_checkout(root.path(), &request).is_err());
        let calls = transport.calls;
        assert_eq!(
            linux::execute(root.path(), &request, false, &|| false, &mut transport).state,
            State::ClaimedUnknown
        );
        assert_eq!(transport.calls, calls);
    }
}

#[test]
fn gitmodules_include_is_parsed_as_data_and_never_followed() {
    let child = source(None, false);
    let parent = source(
        Some(("../child.git", &local_git(child.path(), &["rev-parse", "HEAD"]))),
        false,
    );
    let modules = parent.path().join(".gitmodules");
    let mut bytes = fs::read(&modules).unwrap();
    bytes.extend_from_slice(b"[include]\npath = /nonexistent-private-submodule-config\n");
    fs::write(modules, bytes).unwrap();
    local_git(parent.path(), &["add", ".gitmodules"]);
    local_git(parent.path(), &["commit", "-m", "Synthetic rejected include"]);
    let mut transport = Sources::new(&[&parent, &child]);
    let mut request = request();
    request.source.commit = GitCommitSha::parse(local_git(parent.path(), &["rev-parse", "HEAD"])).unwrap();
    let root = roots();
    let result = linux::execute(root.path(), &request, false, &|| false, &mut transport);
    assert_eq!(result.reason, Some(Error::UnsupportedRepository));
    assert_eq!(transport.fetches, 1);
    assert!(!root.path().join("horizon/repository/.gitmodules").exists());
}
