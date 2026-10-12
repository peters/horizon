//! The commit **Use this setup** makes, and the check before it. The setup agent can
//! write the repository's git configuration, so Horizon's own git commands here run no
//! hooks, filters, file monitors or signing programs: plumbing only, those settings
//! overridden, and every file hashed as it is on disk.
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::SystemTime;

/// A path that holds no hooks on any platform.
#[cfg(windows)]
const NO_HOOKS: &str = "NUL";
#[cfg(not(windows))]
const NO_HOOKS: &str = "/dev/null";

/// A git command in the repository that runs no hook or file monitor the repository's
/// configuration names and reads no replace refs, so HEAD is the real commit. Every git
/// command Horizon runs in a setup's repository is one.
pub(super) fn git_command(repository: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("--no-replace-objects")
        .arg("-C")
        .arg(repository)
        .args(["-c", &format!("core.hooksPath={NO_HOOKS}")])
        .args(["-c", "core.fsmonitor=false", "-c", "core.untrackedCache=false"]);
    command
}

/// Runs one git command in the repository without configured programs, optionally on
/// another index and with input, and returns its trimmed output.
fn git(repository: &Path, index: Option<&Path>, args: &[&str], input: Option<&[u8]>) -> Result<String, String> {
    let mut command = git_command(repository);
    command
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    if let (Some(bytes), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(bytes).map_err(|error| error.to_string())?;
    }
    let output = child.wait_with_output().map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// The index entries for `.horizon` as it is on disk: mode, object and path. Files are
/// stored without filters and links as links, the way the proof built them.
fn entries(repository: &Path) -> Result<Vec<String>, String> {
    use super::untrusted::{Entry, setup_entries};
    let mut entries = Vec::new();
    for (relative, entry) in setup_entries(repository)? {
        if relative.contains(['\n', '\t']) {
            return Err(format!("{relative:?} has a name git's index cannot take here"));
        }
        // Every object goes in through standard input: git opens nothing the agent wrote.
        let (mode, content) = match entry {
            Entry::Folder => continue,
            Entry::Link(target) => ("120000", target.to_string_lossy().replace('\\', "/").into_bytes()),
            Entry::File { executable, content } => (if executable { "100755" } else { "100644" }, content),
        };
        let object = git(
            repository,
            None,
            &["hash-object", "-w", "--no-filters", "--stdin"],
            Some(&content),
        )?;
        entries.push(format!("{mode} {object}\t{relative}"));
    }
    entries.sort();
    Ok(entries)
}

/// Replaces `.horizon` in an index with the entries: what is gone from disk goes.
fn stage(repository: &Path, index: Option<&Path>, entries: &[String]) -> Result<(), String> {
    let staged = git(
        repository,
        index,
        &["ls-files", "-z", "--cached", "--", ".horizon"],
        None,
    )?;
    if !staged.is_empty() {
        git(
            repository,
            index,
            &["update-index", "-z", "--force-remove", "--stdin"],
            Some(staged.as_bytes()),
        )?;
    }
    let mut info = entries.join("\n");
    info.push('\n');
    git(
        repository,
        index,
        &["update-index", "--index-info"],
        Some(info.as_bytes()),
    )?;
    Ok(())
}

/// Commits `.horizon` exactly as it is on disk on top of HEAD, and stages the same in
/// the person's index; other staged changes stay staged and out of the commit. Returns
/// false when HEAD already holds this setup, so there was nothing to commit.
pub(super) fn commit(repository: &Path, message: &str) -> Result<bool, String> {
    let head = git(repository, None, &["rev-parse", "--verify", "HEAD"], None)?;
    let entries = entries(repository)?;
    let index = git(
        repository,
        None,
        &["rev-parse", "--git-path", "horizon-setup-index"],
        None,
    )?;
    let index = repository.join(index);
    let _ = std::fs::remove_file(&index);
    let tree = git(repository, Some(&index), &["read-tree", &head], None)
        .and_then(|_| stage(repository, Some(&index), &entries))
        .and_then(|()| git(repository, Some(&index), &["write-tree"], None));
    let _ = std::fs::remove_file(&index);
    let tree = tree?;
    if tree == git(repository, None, &["rev-parse", &format!("{head}^{{tree}}")], None)? {
        return Ok(false);
    }
    let commit = git(
        repository,
        None,
        &["commit-tree", "--no-gpg-sign", &tree, "-p", &head, "-m", message],
        None,
    )?;
    let reason = format!("commit: {message}");
    git(
        repository,
        None,
        &["update-ref", "-m", &reason, "HEAD", &commit, &head],
        None,
    )?;
    stage(repository, None, &entries)?;
    Ok(true)
}

/// The `.horizon` files that differ from HEAD, as the finished card lists them: each
/// name with "new", "changed" or "removed". Compared by object, so no filter runs.
pub(super) fn changes(repository: &Path) -> Vec<String> {
    let committed: BTreeMap<String, String> = git(
        repository,
        None,
        &["ls-tree", "-r", "-z", "HEAD", "--", ".horizon"],
        None,
    )
    .unwrap_or_default()
    .split('\0')
    .filter_map(|line| {
        let (meta, path) = line.split_once('\t')?;
        Some((path.to_owned(), meta.to_owned()))
    })
    .collect();
    let current: BTreeMap<String, String> = entries(repository)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            let (mode, object) = meta.split_once(' ')?;
            Some((path.to_owned(), format!("{mode} blob {object}")))
        })
        .collect();
    let name = |path: &str| path.trim_start_matches(".horizon/").to_owned();
    let mut changes: Vec<String> = current
        .iter()
        .filter_map(|(path, meta)| match committed.get(path) {
            None => Some(format!("{} new", name(path))),
            Some(before) if before != meta => Some(format!("{} changed", name(path))),
            Some(_) => None,
        })
        .collect();
    changes.extend(
        committed
            .keys()
            .filter(|path| !current.contains_key(*path))
            .map(|path| format!("{} removed", name(path))),
    );
    changes
}

/// When each file outside `.horizon` that git tracks or would track last changed.
pub(super) type Outside = BTreeMap<String, Option<(u64, SystemTime)>>;

/// Reads the files outside `.horizon`: tracked ones and new ones git does not ignore,
/// each with its size and modification time. Nothing is hashed, so it stays quick in a
/// large repository.
pub(super) fn outside(repository: &Path) -> Outside {
    let listed = git(
        repository,
        None,
        &["ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        None,
    )
    .unwrap_or_default();
    listed
        .split('\0')
        .filter(|path| !path.is_empty() && *path != ".horizon" && !path.starts_with(".horizon/"))
        .map(|path| {
            let stamp = std::fs::symlink_metadata(repository.join(path))
                .ok()
                .and_then(|metadata| Some((metadata.len(), metadata.modified().ok()?)));
            (path.to_owned(), stamp)
        })
        .collect()
}

/// The paths that were added, removed or written between two readings.
pub(super) fn changed(before: &Outside, after: &Outside) -> Vec<String> {
    let mut paths: Vec<String> = before
        .iter()
        .filter(|(path, stamp)| after.get(*path) != Some(stamp))
        .map(|(path, _)| path.clone())
        .chain(after.keys().filter(|path| !before.contains_key(*path)).cloned())
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(repository: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repository)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn repository() -> tempfile::TempDir {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path();
        run(path, &["init", "-q"]);
        run(path, &["config", "user.name", "Test"]);
        run(path, &["config", "user.email", "test@example.com"]);
        std::fs::create_dir_all(path.join(".horizon")).unwrap();
        std::fs::write(path.join(".horizon/old.yml"), "old\n").unwrap();
        std::fs::write(path.join("README.md"), "readme\n").unwrap();
        run(path, &["add", "."]);
        run(path, &["commit", "-q", "-m", "first"]);
        folder
    }

    // The hooks and the filter are shell scripts.
    #[cfg(unix)]
    #[test]
    fn use_commits_horizon_exactly_without_hooks_or_filters() {
        let folder = repository();
        let path = folder.path();
        // What a prompt-injected agent could leave: a hook and a filter that leave marks.
        std::fs::create_dir_all(path.join("hooks")).unwrap();
        let mark = path.join("ran");
        for hook in ["pre-commit", "commit-msg", "post-commit", "reference-transaction"] {
            let script = path.join("hooks").join(hook);
            std::fs::write(&script, format!("#!/bin/sh\ntouch '{}'\n", mark.display())).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        run(path, &["config", "core.hooksPath", "hooks"]);
        run(
            path,
            &[
                "config",
                "filter.mark.clean",
                &format!("touch '{}'; cat", mark.display()),
            ],
        );
        std::fs::write(path.join(".gitattributes"), ".horizon/* filter=mark\n").unwrap();
        std::fs::remove_file(path.join(".horizon/old.yml")).unwrap();
        std::fs::write(path.join(".horizon/cloud.yml"), "version: 1\n").unwrap();
        std::fs::write(path.join("README.md"), "staged elsewhere\n").unwrap();
        run(
            path,
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "filter.mark.clean=cat",
                "add",
                "README.md",
            ],
        );

        assert!(commit(path, "Add the cloud setup").unwrap(), "a changed setup commits");
        assert!(!mark.exists(), "no hook or filter ran");
        assert_eq!(run(path, &["log", "-1", "--format=%s"]), "Add the cloud setup");
        assert_eq!(
            run(path, &["ls-tree", "-r", "--name-only", "HEAD", "--", ".horizon"]),
            ".horizon/cloud.yml"
        );
        assert_eq!(
            run(path, &["show", "HEAD:README.md"]),
            "readme",
            "other staged changes stay out"
        );
        assert_eq!(
            run(
                path,
                &["-c", "filter.mark.clean=cat", "diff", "--cached", "--name-only"]
            ),
            "README.md",
            "and stay staged"
        );
        assert!(
            !commit(path, "Again").unwrap(),
            "an unchanged setup has nothing to commit"
        );
        assert!(changes(path).is_empty());
        std::fs::write(path.join(".horizon/cloud.yml"), "version: 2\n").unwrap();
        std::fs::write(path.join(".horizon/Dockerfile"), "FROM scratch\n").unwrap();
        assert_eq!(changes(path), ["Dockerfile new", "cloud.yml changed"]);
        assert!(!mark.exists(), "listing the changes runs no filter either");
    }

    #[test]
    fn a_replace_ref_does_not_change_what_use_commits_on() {
        let folder = repository();
        let path = folder.path();
        let real = run(path, &["rev-parse", "HEAD"]);
        std::fs::write(path.join("README.md"), "planted\n").unwrap();
        run(path, &["commit", "-q", "-am", "planted"]);
        let planted = run(path, &["rev-parse", "HEAD"]);
        run(path, &["reset", "-q", "--hard", &real]);
        run(path, &["replace", &real, &planted]);
        std::fs::write(path.join(".horizon/cloud.yml"), "version: 1\n").unwrap();
        assert!(commit(path, "Add the cloud setup").unwrap());
        assert_eq!(
            run(path, &["--no-replace-objects", "show", "HEAD:README.md"]),
            "readme",
            "the commit builds on the real HEAD"
        );
        assert_eq!(run(path, &["--no-replace-objects", "rev-parse", "HEAD^"]), real);
    }

    #[test]
    fn changes_outside_the_setup_are_named() {
        let folder = repository();
        let path = folder.path();
        let before = outside(path);
        assert!(before.contains_key("README.md"));
        std::fs::write(path.join(".horizon/cloud.yml"), "version: 1\n").unwrap();
        assert!(
            changed(&before, &outside(path)).is_empty(),
            "the setup itself does not count"
        );
        std::fs::write(path.join("README.md"), "changed by the agent, longer\n").unwrap();
        std::fs::write(path.join("new.txt"), "new\n").unwrap();
        assert_eq!(changed(&before, &outside(path)), ["README.md", "new.txt"]);
    }
}
