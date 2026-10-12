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
    git_raw(repository, index, args, input).map(|output| String::from_utf8_lossy(&output).trim().to_owned())
}

/// As [`git`], with the output's bytes exactly as git wrote them.
fn git_raw(repository: &Path, index: Option<&Path>, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, String> {
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
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// The index entries for `.horizon` as it is on disk: mode, object and path. Files are
/// stored without filters and links as links, the way the proof built them.
fn entries(repository: &Path) -> Result<Vec<String>, String> {
    entries_of(repository, &super::untrusted::setup_entries(repository, None)?)
}

/// The index entries for `.horizon` as one reading of it holds them. Each object's
/// content goes into a private file of Horizon's own, and one `git hash-object` reads
/// them all, so git opens nothing the agent wrote and a large setup costs one process.
fn entries_of(repository: &Path, read: &[(String, super::untrusted::Entry)]) -> Result<Vec<String>, String> {
    use super::untrusted::Entry;
    let folder = tempfile::Builder::new()
        .prefix("horizon-setup-objects-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let mut named = Vec::new();
    for (index, (relative, entry)) in read.iter().enumerate() {
        if relative.contains(['\n', '\t']) {
            return Err(format!("{relative:?} has a name git's index cannot take here"));
        }
        let (mode, content) = match entry {
            Entry::Folder => continue,
            Entry::Link(target) => ("120000", super::untrusted::link_bytes(target)),
            Entry::File { executable, content } => (if *executable { "100755" } else { "100644" }, content.clone()),
        };
        let file = folder.path().join(index.to_string());
        std::fs::write(&file, content).map_err(|error| error.to_string())?;
        named.push((mode, relative, file));
    }
    if named.is_empty() {
        return Ok(Vec::new());
    }
    let paths = named.iter().fold(String::new(), |mut paths, (_, _, file)| {
        paths.push_str(&file.display().to_string());
        paths.push('\n');
        paths
    });
    let objects = git(
        repository,
        None,
        &["hash-object", "-w", "--no-filters", "--stdin-paths"],
        Some(paths.as_bytes()),
    )?;
    let objects: Vec<&str> = objects.lines().collect();
    if objects.len() != named.len() {
        return Err("git did not name every object of the setup".to_owned());
    }
    let mut entries: Vec<String> = named
        .iter()
        .zip(objects)
        .map(|((mode, relative, _), object)| format!("{mode} {object}\t{relative}"))
        .collect();
    entries.sort();
    Ok(entries)
}

/// What [`commit`] did.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Committed {
    /// HEAD already held the setup.
    Unchanged,
    /// HEAD moved to a new commit with the setup, and the index has it too.
    New,
    /// HEAD moved to the new commit, but the index could not be updated.
    IndexBehind(String),
}

/// Replaces `.horizon` in an index with the entries: what is gone from disk goes.
fn stage(repository: &Path, index: Option<&Path>, entries: &[String]) -> Result<(), String> {
    // The staged paths go back to git byte for byte, whatever their encoding.
    let staged = git_raw(
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
            Some(&staged),
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

/// Commits `.horizon` exactly as `setup` read it on top of HEAD, and stages the same in
/// the person's index; other staged changes stay staged and out of the commit. HEAD
/// must still be the commit `setup` names. Returns false when HEAD already holds this
/// setup, so there was nothing to commit.
pub(super) fn commit(repository: &Path, message: &str, setup: &super::snapshot::Setup) -> Result<Committed, String> {
    let head = git(repository, None, &["rev-parse", "--verify", "HEAD"], None)?;
    if head != setup.head {
        return Err("HEAD moved after Horizon read the setup; start the setup again".to_owned());
    }
    let entries = entries_of(repository, &setup.entries)?;
    // An index of this commit's own, with a name no other Use, in this Horizon or
    // another one on the same checkout, can share; it goes when the tree is written.
    let git_dir = git(repository, None, &["rev-parse", "--absolute-git-dir"], None)?;
    let index = tempfile::Builder::new()
        .prefix("horizon-setup-index-")
        .tempfile_in(&git_dir)
        .map_err(|error| format!("cannot make a temporary index: {error}"))?
        .into_temp_path();
    // git writes the index itself; it refuses an empty file in its place.
    std::fs::remove_file(&index).map_err(|error| error.to_string())?;
    let tree = git(repository, Some(&index), &["read-tree", &head], None)
        .and_then(|_| stage(repository, Some(&index), &entries))
        .and_then(|()| git(repository, Some(&index), &["write-tree"], None));
    drop(index);
    let tree = tree?;
    if tree == git(repository, None, &["rev-parse", &format!("{head}^{{tree}}")], None)? {
        // Nothing to commit, but the person's index still gets the setup as it is, so a
        // staged deletion or an earlier failed staging does not stay behind.
        stage(repository, None, &entries)?;
        return Ok(Committed::Unchanged);
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
    // The commit is made; an index another git process holds only stays behind.
    Ok(match stage(repository, None, &entries) {
        Ok(()) => Committed::New,
        Err(error) => Committed::IndexBehind(error),
    })
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

/// The repository outside `.horizon` at one moment: HEAD, and each file git tracks or
/// would track with a stamp of its last change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Outside {
    head: String,
    /// Each path as git lists it, byte for byte, so no two names read as one.
    files: BTreeMap<Vec<u8>, Option<Stamp>>,
    /// The ignored files that exist, such as a local .env or files in an ignored
    /// folder. A change to one counts; a new one, such as fresh build output, does not.
    /// A later check stats these same paths again, whatever else appeared.
    ignored: BTreeMap<Vec<u8>, Option<Stamp>>,
    /// Git's own control files outside the work tree, such as its configuration, hooks
    /// and replace refs, which later git commands of the person would follow.
    control: BTreeMap<String, Option<Stamp>>,
    /// The repository the paths are in.
    root: std::path::PathBuf,
}

/// The control files of the repository's git folder that change what git does.
fn control(repository: &Path) -> BTreeMap<String, Option<Stamp>> {
    let git_path = |name: &str| {
        git(repository, None, &["rev-parse", "--git-path", name], None)
            .ok()
            .map(|path| repository.join(path))
    };
    let stamp_at = |path: &Path| std::fs::symlink_metadata(path).ok().map(|metadata| stamp(&metadata));
    let mut control = BTreeMap::new();
    for name in [
        "config",
        "config.worktree",
        "info/attributes",
        "info/exclude",
        "packed-refs",
    ] {
        if let Some(path) = git_path(name) {
            control.insert(format!(".git/{name}"), stamp_at(&path));
        }
    }
    // Hooks from the common git folder itself: Horizon's own git commands point
    // core.hooksPath elsewhere, and a hooksPath the repository sets is in its config.
    let common = git(repository, None, &["rev-parse", "--git-common-dir"], None)
        .ok()
        .map(|path| repository.join(path));
    for folder in ["hooks", "refs/replace"] {
        let Some(path) = common.as_ref().map(|common| common.join(folder)) else {
            continue;
        };
        for entry in std::fs::read_dir(&path).into_iter().flatten().flatten().take(1000) {
            let name = format!(".git/{folder}/{}", entry.file_name().to_string_lossy());
            control.insert(name, stamp_at(&entry.path()));
        }
    }
    control
}

/// The most ignored files a reading stamps, so a large dependency or build folder
/// cannot hold up the dialog. Ignored files past it, in git's order, are not compared.
const IGNORED_LIMIT: usize = 100_000;

/// The most tracked and new files a reading stamps; a repository with more is compared on
/// these, in git's order.
const FILES_LIMIT: usize = 1_000_000;

/// What changes whenever a file is written, replaced or has its times set: on Unix the
/// inode change time, which no one can set back, with the inode, size and mode.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64, u64, u64, u32),
}

fn stamp(metadata: &std::fs::Metadata) -> Stamp {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt as _;
    Stamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        changed: (
            metadata.ctime(),
            metadata.ctime_nsec(),
            metadata.ino(),
            metadata.dev(),
            metadata.mode(),
        ),
    }
}

/// Reads HEAD and the files outside `.horizon`: tracked ones and new ones git does not
/// ignore, each stamped from its metadata. Nothing is opened or hashed, so it stays
/// quick in a large repository and nothing the agent left can make it wait.
pub(super) fn outside(repository: &Path) -> Outside {
    let head = git(repository, None, &["rev-parse", "--verify", "HEAD"], None).unwrap_or_default();
    let stamp_of = |path: &[u8]| {
        let stamp = std::fs::symlink_metadata(repository.join(path_of(path)))
            .ok()
            .map(|metadata| stamp(&metadata));
        (path.to_vec(), stamp)
    };
    // Both listings leave .horizon out in git's own pathspec, are read as git writes
    // them and stop at their limit, so no listing is ever held whole in memory.
    let files = git_paths(
        repository,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            ".",
            ":(exclude).horizon",
        ],
        FILES_LIMIT,
    )
    .iter()
    .map(|path| stamp_of(path))
    .collect();
    let ignored = git_paths(
        repository,
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--",
            ".",
            ":(exclude).horizon",
        ],
        IGNORED_LIMIT,
    )
    .iter()
    .map(|path| stamp_of(path))
    .collect();
    Outside {
        head,
        files,
        ignored,
        control: control(repository),
        root: repository.to_path_buf(),
    }
}

/// A listed path that is not in `.horizon`.
fn outside_setup(path: &[u8]) -> bool {
    !path.is_empty() && path != b".horizon" && !path.starts_with(b".horizon/")
}

/// The first `limit` paths outside `.horizon` that git lists NUL-separated for `args`,
/// read as git writes them; git is stopped once the limit is reached.
fn git_paths(repository: &Path, args: &[&str], limit: usize) -> Vec<Vec<u8>> {
    use std::io::BufRead as _;
    let Ok(mut child) = git_command(repository)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let mut reader = std::io::BufReader::new(stdout);
        let mut path = Vec::new();
        while paths.len() < limit {
            path.clear();
            match reader.read_until(0, &mut path) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if path.last() == Some(&0) {
                        path.pop();
                    }
                    if outside_setup(&path) {
                        paths.push(path.clone());
                    }
                }
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    paths
}

/// A path git listed, as this platform names files.
fn path_of(bytes: &[u8]) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        std::ffi::OsStr::from_bytes(bytes).into()
    }
    #[cfg(not(unix))]
    {
        String::from_utf8_lossy(bytes).into_owned().into()
    }
}

/// What changed between two readings: the paths added, removed or written, and HEAD
/// when it moved.
pub(super) fn changed(before: &Outside, after: &Outside) -> Vec<String> {
    let mut paths: Vec<&Vec<u8>> = before
        .files
        .iter()
        .filter(|(path, stamp)| after.files.get(*path) != Some(stamp))
        .map(|(path, _)| path)
        .chain(after.files.keys().filter(|path| !before.files.contains_key(*path)))
        // An ignored file that existed and changed or went counts; a new one does not.
        // Each is stat'ed again by its path, so files that appeared since cannot push
        // it out of the reading.
        .chain(
            before
                .ignored
                .iter()
                .filter(|(path, was)| {
                    let now = std::fs::symlink_metadata(before.root.join(path_of(path)))
                        .ok()
                        .map(|metadata| stamp(&metadata));
                    now != **was && !after.files.contains_key(*path)
                })
                .map(|(path, _)| path),
        )
        .collect();
    paths.sort();
    paths.dedup();
    // Only the names shown are made readable; the comparison used the bytes.
    let mut paths: Vec<String> = paths
        .into_iter()
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect();
    // Git's control files, added, changed or removed.
    let mut control: Vec<String> = before
        .control
        .iter()
        .filter(|(name, stamp)| after.control.get(*name) != Some(stamp))
        .map(|(name, _)| name.clone())
        .chain(
            after
                .control
                .keys()
                .filter(|name| !before.control.contains_key(*name))
                .cloned(),
        )
        .collect();
    control.sort();
    control.dedup();
    paths.splice(0..0, control);
    if before.head != after.head {
        paths.insert(0, "the commit HEAD names".to_owned());
    }
    paths
}

// The setup agent, and so its commit, runs on Linux and macOS only.
#[cfg(all(test, unix))]
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

    fn setup(path: &Path) -> crate::app::cloud_panel::production::repository_setup::snapshot::Setup {
        crate::app::cloud_panel::production::repository_setup::snapshot::Setup::read(path, None).unwrap()
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

        assert_eq!(
            commit(path, "Add the cloud setup", &setup(path)).unwrap(),
            Committed::New,
            "a changed setup commits"
        );
        assert!(!mark.exists(), "no hook or filter ran");
        let leftovers = std::fs::read_dir(path.join(".git"))
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("horizon-setup-index"))
            .count();
        assert_eq!(leftovers, 0, "the commit's own index goes with it");
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
            commit(path, "Again", &setup(path)).unwrap() == Committed::Unchanged,
            "an unchanged setup has nothing to commit"
        );
        assert!(changes(path).is_empty());
        std::fs::write(path.join(".horizon/cloud.yml"), "version: 2\n").unwrap();
        std::fs::write(path.join(".horizon/Dockerfile"), "FROM scratch\n").unwrap();
        assert_eq!(changes(path), ["Dockerfile new", "cloud.yml changed"]);
        assert!(!mark.exists(), "listing the changes runs no filter either");
    }

    // The setup agent runs on Linux and macOS only.
    #[cfg(unix)]
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
        assert_eq!(
            commit(path, "Add the cloud setup", &setup(path)).unwrap(),
            Committed::New
        );
        assert_eq!(
            run(path, &["--no-replace-objects", "show", "HEAD:README.md"]),
            "readme",
            "the commit builds on the real HEAD"
        );
        assert_eq!(run(path, &["--no-replace-objects", "rev-parse", "HEAD^"]), real);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn names_that_are_not_utf8_stay_apart() {
        use std::os::unix::ffi::OsStrExt as _;
        let folder = repository();
        let path = folder.path();
        let first = path.join(std::ffi::OsStr::from_bytes(b"a\xff"));
        let second = path.join(std::ffi::OsStr::from_bytes(b"a\xfe"));
        std::fs::write(&first, "one\n").unwrap();
        std::fs::write(&second, "two\n").unwrap();
        let before = outside(path);
        std::fs::write(&second, "changed, and longer\n").unwrap();
        assert_eq!(
            changed(&before, &outside(path)).len(),
            1,
            "one name changed, not neither"
        );
    }

    // The setup agent runs on Linux and macOS only.
    #[cfg(unix)]
    #[test]
    fn changes_outside_the_setup_are_named() {
        let folder = repository();
        let path = folder.path();
        let before = outside(path);
        assert!(before.files.contains_key(b"README.md".as_slice()));
        std::fs::write(path.join(".horizon/cloud.yml"), "version: 1\n").unwrap();
        assert!(
            changed(&before, &outside(path)).is_empty(),
            "the setup itself does not count"
        );
        // The same size with the time set back still counts: the change time moves.
        let readme = path.join("README.md");
        let before_time = std::fs::metadata(&readme).unwrap().modified().unwrap();
        std::fs::write(&readme, "README\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&readme)
            .unwrap()
            .set_modified(before_time)
            .unwrap();
        #[cfg(unix)]
        assert_eq!(changed(&before, &outside(path)), ["README.md"]);
        std::fs::write(path.join("README.md"), "changed by the agent, longer\n").unwrap();
        std::fs::write(path.join("new.txt"), "new\n").unwrap();
        assert_eq!(changed(&before, &outside(path)), ["README.md", "new.txt"]);
        let before = outside(path);
        run(path, &["config", "core.sshCommand", "touch /tmp/never-run"]);
        std::fs::create_dir_all(path.join(".git/hooks")).unwrap();
        std::fs::write(path.join(".git/hooks/post-checkout"), "#!/bin/sh\n").unwrap();
        assert_eq!(
            changed(&before, &outside(path)),
            [".git/config", ".git/hooks/post-checkout"],
            "git's own configuration and hooks count too"
        );
        std::fs::write(path.join(".gitignore"), ".env\ntarget/\n").unwrap();
        std::fs::write(path.join(".env"), "TOKEN=x\n").unwrap();
        let before = outside(path);
        std::fs::create_dir_all(path.join("target")).unwrap();
        std::fs::write(path.join("target/out"), "build\n").unwrap();
        assert!(
            changed(&before, &outside(path)).is_empty(),
            "new build output does not count"
        );
        std::fs::write(path.join(".env"), "TOKEN=changed by the agent\n").unwrap();
        assert_eq!(
            changed(&before, &outside(path)),
            [".env"],
            "a change to an ignored file does"
        );
        let before = outside(path);
        std::fs::write(path.join("target/out"), "changed by the agent, longer\n").unwrap();
        assert_eq!(
            changed(&before, &outside(path)),
            ["target/out"],
            "also in an ignored folder"
        );
    }
}
