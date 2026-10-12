//! The setup a proof tests, read once when the proof starts: `.horizon` as the
//! untrusted walk read it and the commit HEAD named then. The proof fingerprints this
//! snapshot and builds every image from it, never from the live folder, which the
//! agent can still change while the proof runs.
use super::proof::Spec;
use super::untrusted::{Entry, setup_entries};
use std::path::{Component, Path, PathBuf};

/// `.horizon` and HEAD as one proof reads them.
pub(super) struct Setup {
    pub(super) entries: Vec<(String, Entry)>,
    /// The commit, empty when the repository has none yet.
    pub(super) head: String,
}

impl Setup {
    pub(super) fn read(repository: &Path, expected: super::untrusted::Identity) -> Result<Self, String> {
        let entries = setup_entries(repository, expected)?;
        let head = super::commit::git_command(repository)
            .args(["rev-parse", "--verify", "HEAD"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_default();
        Ok(Self { entries, head })
    }

    /// The content of the snapshot's file at `relative`, such as `.horizon/cloud.yml`.
    pub(super) fn file(&self, relative: &str) -> Option<&[u8]> {
        self.entries.iter().find_map(|(path, entry)| match entry {
            Entry::File { content, .. } if path == relative => Some(content.as_slice()),
            _ => None,
        })
    }

    /// The snapshot's `.horizon/cloud.yml`.
    pub(super) fn config(&self) -> Result<horizon_core::cloud_panel::CloudConfig, String> {
        let text = self
            .file(".horizon/cloud.yml")
            .ok_or("the setup has no .horizon/cloud.yml")?;
        horizon_core::cloud_panel::CloudConfig::parse(&String::from_utf8_lossy(text)).map_err(|error| error.to_string())
    }

    /// What a proof tests: every entry of `.horizon`, the commit and the proof request,
    /// so a passed proof says nothing about a setup that changed after it. SHA-256 over
    /// framed records (kind, path, mode and content, each with its length), so no two
    /// setups read as the same bytes.
    pub(super) fn fingerprint(&self, spec: &Spec) -> [u8; 32] {
        use sha2::{Digest as _, Sha256};
        let mut hash = Sha256::new();
        let mut record = |kind: &[u8], bytes: &[u8]| {
            hash.update(kind);
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        };
        record(b"spec", format!("{spec:?}").as_bytes());
        // The commit too: a context outside .horizon builds from the committed tree.
        record(b"head", self.head.as_bytes());
        for (relative, entry) in &self.entries {
            record(b"path", relative.as_bytes());
            // A link counts by its target's name, never followed: no loop, nothing outside.
            match entry {
                Entry::Folder => record(b"folder", b""),
                Entry::Link(target) => record(b"link", &super::untrusted::link_bytes(target)),
                Entry::File { executable, content } => {
                    record(b"mode", if *executable { b"755" } else { b"644" });
                    record(b"file", content);
                }
            }
        }
        hash.finalize().into()
    }

    /// A private build context for `build` made from this snapshot, and the context and
    /// Dockerfile paths in it. With both paths under `.horizon` it holds the snapshot's
    /// `.horizon` only; otherwise it is the committed tree of the snapshot's commit, made
    /// the way deployment makes it (Git LFS files and submodules included), with the
    /// snapshot's `.horizon` in place of the committed one: the image deployment builds.
    pub(super) fn context(
        &self,
        repository: &Path,
        build: &horizon_core::cloud_panel::Build,
    ) -> Result<(tempfile::TempDir, PathBuf, PathBuf), String> {
        let context = within(&build.context, "build.context")?;
        let dockerfile = within(&build.dockerfile, "build.dockerfile")?;
        let folder = tempfile::Builder::new()
            .prefix("horizon-setup-context-")
            .tempdir()
            .map_err(|error| error.to_string())?;
        let tree = if context.starts_with(".horizon") && dockerfile.starts_with(".horizon") {
            folder.path().to_path_buf()
        } else {
            if self.head.is_empty() {
                return Err("the repository has no commit to build from".to_owned());
            }
            let cancel = horizon_core::cloud_runtime::Cancellation::default();
            let runner = horizon_core::cloud_runtime::command::Runner {
                cancel: &cancel,
                emit: &|_| {},
                secrets: Vec::new(),
            };
            let tree =
                horizon_core::cloud_runtime::repository::snapshot(repository, &self.head, folder.path(), &runner)
                    .map_err(|error| format!("cannot make the committed tree: {error}"))?;
            // The proposed .horizon replaces the committed one exactly: deletions and links
            // too, and a committed file or link of that name as well as a folder.
            let committed = tree.join(".horizon");
            match std::fs::symlink_metadata(&committed) {
                Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(&committed),
                Ok(_) => std::fs::remove_file(&committed),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            }
            .map_err(|error| format!("cannot replace the committed .horizon: {error}"))?;
            tree
        };
        self.write_into(&tree)?;
        // No part of either path may be a link, which could lead the build outside.
        for path in [&context, &dockerfile] {
            let mut walked = tree.clone();
            for part in path.components() {
                walked.push(part);
                if std::fs::symlink_metadata(&walked).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
                    return Err(format!("{} goes through a link", path.display()));
                }
            }
        }
        Ok((folder, tree.join(context), tree.join(dockerfile)))
    }

    /// Writes the snapshot's `.horizon` into `tree`: files with the mode git commits for
    /// them, 0644 or 0755 whatever the umask, as deployment's snapshot extracts them; links
    /// as links.
    fn write_into(&self, tree: &Path) -> Result<(), String> {
        std::fs::create_dir_all(tree.join(".horizon")).map_err(|error| error.to_string())?;
        for (relative, entry) in &self.entries {
            let target = tree.join(relative);
            match entry {
                Entry::Folder => std::fs::create_dir_all(&target).map_err(|error| error.to_string())?,
                #[cfg(unix)]
                Entry::Link(link) => std::os::unix::fs::symlink(link, &target).map_err(|error| error.to_string())?,
                #[cfg(not(unix))]
                Entry::Link(_) => return Err(format!("{relative} is a link")),
                Entry::File { executable, content } => {
                    std::fs::write(&target, content).map_err(|error| error.to_string())?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt as _;
                        let mode = if *executable { 0o755 } else { 0o644 };
                        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode))
                            .map_err(|error| error.to_string())?;
                    }
                    #[cfg(not(unix))]
                    let _ = executable;
                }
            }
        }
        Ok(())
    }
}

/// The fingerprint of the setup as it is now, to compare with the proof's.
pub(super) fn fingerprint(
    repository: &Path,
    spec: &Spec,
    expected: super::untrusted::Identity,
) -> Result<[u8; 32], String> {
    Ok(Setup::read(repository, expected)?.fingerprint(spec))
}

/// A build path the agent wrote, made relative to the repository without reading the
/// disk: `.` and `..` resolved, and refused when it leaves the repository.
fn within(path: &str, field: &str) -> Result<PathBuf, String> {
    let mut normal = PathBuf::new();
    for part in Path::new(path).components() {
        match part {
            Component::CurDir => {}
            Component::Normal(name) => normal.push(name),
            Component::ParentDir => {
                if !normal.pop() {
                    return Err(format!("{field} {path} leaves the repository"));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("{field} {path} must be relative to the repository"));
            }
        }
    }
    Ok(normal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proof_fingerprint_follows_the_setup_and_the_request() {
        let repository = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repository.path().join(".horizon")).unwrap();
        std::fs::write(repository.path().join(".horizon/Dockerfile"), "FROM base\n").unwrap();
        let spec = Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": ["test"]})).unwrap();
        let first = fingerprint(repository.path(), &spec, None);
        assert_eq!(first, fingerprint(repository.path(), &spec, None), "stable");
        std::fs::write(repository.path().join(".horizon/Dockerfile"), "FROM base\nRUN true\n").unwrap();
        let edited = fingerprint(repository.path(), &spec, None);
        assert_ne!(first, edited, "an edited Dockerfile is not the proved one");
        let other = Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": ["lint"]})).unwrap();
        assert_ne!(
            edited,
            fingerprint(repository.path(), &other, None),
            "nor another request"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_the_setup_counts_by_its_target_and_is_never_followed() {
        let repository = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repository.path().join(".horizon")).unwrap();
        std::fs::write(repository.path().join(".horizon/Dockerfile"), "FROM base\n").unwrap();
        std::os::unix::fs::symlink(
            repository.path().join(".horizon"),
            repository.path().join(".horizon/loop"),
        )
        .unwrap();
        let spec = Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": ["test"]})).unwrap();
        let first = fingerprint(repository.path(), &spec, None);
        std::fs::remove_file(repository.path().join(".horizon/loop")).unwrap();
        std::os::unix::fs::symlink("/etc", repository.path().join(".horizon/loop")).unwrap();
        assert_ne!(
            first,
            fingerprint(repository.path(), &spec, None),
            "a link to elsewhere is a change"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn link_targets_count_by_their_raw_bytes() {
        use std::os::unix::ffi::OsStrExt as _;
        let spec = Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": ["test"]})).unwrap();
        let read = |target: &[u8]| {
            let repository = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(repository.path().join(".horizon")).unwrap();
            std::os::unix::fs::symlink(
                std::ffi::OsStr::from_bytes(target),
                repository.path().join(".horizon/x"),
            )
            .unwrap();
            fingerprint(repository.path(), &spec, None).unwrap()
        };
        assert_ne!(
            read(b"a\xff"),
            read(b"a\xfe"),
            "two targets that are not UTF-8 stay apart"
        );
        assert_ne!(read(b"a\\b"), read(b"a/b"), "a backslash is its own byte");
    }

    // The setup agent runs on Linux and macOS only.
    #[cfg(unix)]
    #[test]
    fn a_committed_horizon_file_gives_way_to_the_proposed_folder() {
        let repository = tempfile::tempdir().unwrap();
        let path = repository.path();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid"])
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(path.join(".horizon"), "an old file\n").unwrap();
        std::fs::write(path.join("app.txt"), "app\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "first"]);
        std::fs::remove_file(path.join(".horizon")).unwrap();
        std::fs::create_dir(path.join(".horizon")).unwrap();
        std::fs::write(path.join(".horizon/Dockerfile"), "FROM scratch\n").unwrap();
        let setup = Setup::read(path, None).unwrap();
        let build: horizon_core::cloud_panel::Build =
            serde_yaml::from_str("{context: ., dockerfile: .horizon/Dockerfile}").unwrap();
        let (_folder, context, dockerfile) = setup.context(path, &build).unwrap();
        assert_eq!(std::fs::read_to_string(dockerfile).unwrap(), "FROM scratch\n");
        assert!(context.join("app.txt").is_file(), "the committed tree is the context");
    }

    #[test]
    fn a_build_path_must_stay_in_the_repository() {
        assert_eq!(within(".horizon", "build.context").unwrap(), Path::new(".horizon"));
        assert_eq!(
            within("./.horizon/../.horizon/x", "build.context").unwrap(),
            Path::new(".horizon/x")
        );
        assert_eq!(within(".", "build.context").unwrap(), Path::new(""));
        assert!(
            within("..", "build.context")
                .unwrap_err()
                .contains("leaves the repository")
        );
        assert!(within("/etc", "build.context").unwrap_err().contains("relative"));
    }

    #[test]
    fn the_fingerprint_frames_each_file() {
        let spec = Spec::parse(&serde_json::json!({"workflow": "ci.yml", "jobs": ["test"]})).unwrap();
        let read = |files: &[(&str, &str)]| {
            let repository = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(repository.path().join(".horizon")).unwrap();
            for (name, content) in files {
                std::fs::write(repository.path().join(".horizon").join(name), content).unwrap();
            }
            (fingerprint(repository.path(), &spec, None), repository)
        };
        let (one, _kept) = read(&[("a", "b")]);
        #[cfg_attr(not(unix), allow(unused_variables))]
        let (other, repository) = read(&[("ab", "")]);
        assert_ne!(one, other, "a path and its content never run together");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let file = repository.path().join(".horizon/ab");
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_ne!(
                fingerprint(repository.path(), &spec, None),
                other,
                "the executable bit counts"
            );
        }
    }
}
