//! The pin check Horizon owns: the setup agent says in `.horizon/pins.json` which
//! repository file each toolchain version in `.horizon/Dockerfile` comes from, Horizon
//! writes the one verifier beside it, and the repository's CI can run it, so a change
//! that moves a toolchain without the setup fails in its own pull request.
//!
//! The setup agent can write the repository, so Horizon never runs the verifier file
//! there and never reads the live files: it runs its own copy with `python3 -I -c` in a
//! private tree made from the proof's snapshot, `.horizon` as the proof read it and each
//! named source file from the snapshot's commit.
use super::snapshot::Setup;
use super::untrusted::Entry;
use std::io::Write as _;
use std::path::{Component, Path};
use std::process::Command;

/// The verifier every repository gets, byte for byte.
pub(super) const VERIFIER: &str = include_str!("pins/verify-pins.py");

const DECLARATION: &str = ".horizon/pins.json";
const FILE: &str = "verify-pins.py";

/// The most source files one declaration names, and the largest one read.
const SOURCE_LIMIT: usize = 64;
const SOURCE_BYTES: u64 = 1024 * 1024;

/// The longest link target read from a commit, as long as a path may be.
const LINK_BYTES: u64 = 4096;

/// Replaces `.horizon/verify-pins.py` with the verifier when the repository declares
/// pins or already holds a file by that name, before the proof reads its snapshot, so
/// the snapshot (and the commit Use makes) holds Horizon's file, never the agent's. The
/// file is replaced by a rename: a link left in its place is replaced, never followed.
pub(super) fn write_verifier(repository: &Path, expected: super::untrusted::Identity) -> Result<(), String> {
    let horizon = repository.join(".horizon");
    let wanted = std::fs::symlink_metadata(repository.join(DECLARATION)).is_ok()
        || std::fs::symlink_metadata(horizon.join(FILE)).is_ok();
    if !wanted {
        return Ok(());
    }
    replace_in(repository, expected).map_err(|error| format!("cannot write .horizon/{FILE}: {error}"))
}

/// Writes the verifier through a handle on `.horizon`, opened without following a link
/// from a handle on the repository folder that must still be the run's (`expected`),
/// then renames it into place inside that same folder: swapping either folder for a
/// link or another folder meanwhile cannot send the write anywhere else.
#[cfg(unix)]
fn replace_in(repository: &Path, expected: super::untrusted::Identity) -> std::io::Result<()> {
    use rustix::fs::{AtFlags, Mode, OFlags};
    let folder_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let root = rustix::fs::open(repository, folder_flags, Mode::empty())?;
    if let Some((device, inode)) = expected {
        let stat = rustix::fs::fstat(&root)?;
        // The fields are u64 on Linux but not on every Unix, such as macOS.
        #[allow(clippy::useless_conversion)]
        let found = (u64::try_from(stat.st_dev).ok(), u64::try_from(stat.st_ino).ok());
        if found != (Some(device), Some(inode)) {
            return Err(std::io::Error::other("the repository folder is no longer the run's"));
        }
    }
    let folder = rustix::fs::openat(&root, ".horizon", folder_flags, Mode::empty())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let temporary = format!(".verify-pins-{}-{stamp}", std::process::id());
    let created = rustix::fs::openat(
        &folder,
        temporary.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o644),
    )?;
    let written = std::fs::File::from(created).write_all(VERIFIER.as_bytes());
    let renamed = written.and_then(|()| Ok(rustix::fs::renameat(&folder, temporary.as_str(), &folder, FILE)?));
    if renamed.is_err() {
        let _ = rustix::fs::unlinkat(&folder, temporary.as_str(), AtFlags::empty());
    }
    renamed
}

/// Elsewhere the setup agent does not run; a plain replace by rename is enough.
#[cfg(not(unix))]
fn replace_in(repository: &Path, _expected: super::untrusted::Identity) -> std::io::Result<()> {
    let horizon = repository.join(".horizon");
    let horizon = horizon.as_path();
    let mut file = tempfile::Builder::new().prefix(".verify-pins-").tempfile_in(horizon)?;
    file.write_all(VERIFIER.as_bytes())?;
    file.persist(horizon.join(FILE))
        .map(|_| ())
        .map_err(|error| error.error)
}

/// The longest the pin check may take; it reads a few small files.
const TIMEOUT: std::time::Duration = std::time::Duration::from_mins(1);

/// Runs the snapshot's pin check, if it declares pins, through `stream` (which shows
/// and limits the command), and says through `say` when this computer cannot run it.
pub(super) fn run(
    (repository, setup): (&Path, &Setup),
    stream: impl FnOnce(Command, std::time::Duration) -> Result<(), String>,
    say: impl FnOnce(String),
) -> Result<(), String> {
    match check(repository, setup) {
        None => Ok(()),
        Some(Err(error)) => Err(format!("the pin check could not run: {error}")),
        Some(Ok(Check::Skipped(note))) => {
            say(note);
            Ok(())
        }
        Some(Ok(Check::Run(_tree, command))) => stream(command, TIMEOUT)
            .map_err(|error| format!("the pin check failed ({error}); its output above names each pin")),
    }
}

/// The pin check of one proof.
pub(super) enum Check {
    /// The private tree, kept while the command runs, and the command.
    Run(tempfile::TempDir, Command),
    /// Why this computer cannot run the check; the repository's CI still can.
    Skipped(String),
}

/// The pin check of the snapshot. `None` when the snapshot declares no pins; a
/// declaration that is a link or a folder is refused, so it cannot turn the check off
/// while Use still commits it.
pub(super) fn check(repository: &Path, setup: &Setup) -> Option<Result<Check, String>> {
    // The snapshot is what Use commits, so its verifier must be Horizon's, byte for byte,
    // even if the agent changed the file after Horizon wrote it, and with pins declared
    // it must be there at all: the CI step the summary names runs it.
    let verifier = format!(".horizon/{FILE}");
    let declared = setup.entries.iter().any(|(path, _)| path == DECLARATION);
    let horizons =
        setup.entries.iter().find(|(path, _)| *path == verifier).map(
            |(_, entry)| matches!(entry, Entry::File { content, .. } if content.as_slice() == VERIFIER.as_bytes()),
        );
    if horizons == Some(false) || (declared && horizons.is_none()) {
        return Some(Err(format!(
            "{verifier} is not Horizon's verifier; prove again so Horizon writes it"
        )));
    }
    let (_, entry) = setup.entries.iter().find(|(path, _)| path == DECLARATION)?;
    let Entry::File {
        content: declaration, ..
    } = entry
    else {
        return Some(Err(format!("{DECLARATION} must be a regular file")));
    };
    let Some(python) = ["python3", "python", "py"].into_iter().find(|python| {
        Command::new(python)
            // Isolated, and from a folder no repository writes, as the check itself runs.
            .current_dir(std::env::temp_dir())
            .args(["-I", "-c", "import sys; sys.exit(sys.version_info < (3, 8))"])
            .output()
            .is_ok_and(|output| output.status.success())
    }) else {
        return Some(Ok(Check::Skipped(
            "The pin check needs Python 3.8 or newer on this computer; it was skipped here.".to_owned(),
        )));
    };
    Some(tree(repository, setup, declaration).map(|(tree, dockerfiles)| {
        let mut command = Command::new(python);
        command
            .current_dir(tree.path())
            .args(["-I", "-c", VERIFIER])
            .arg(tree.path().join(".horizon"))
            .args(dockerfiles);
        Check::Run(tree, command)
    }))
}

/// A private tree with the snapshot's declaration and Dockerfile, and each source file
/// the declaration names as the snapshot's commit holds it. A name that is not a plain
/// relative path, a file the commit lacks, or one that cannot be written beside the
/// others is left out: the verifier reports it.
///
/// The second value names the Dockerfiles in `.horizon` that the profiles build, the
/// default profile's first, for the verifier to check them all.
fn tree(repository: &Path, setup: &Setup, declaration: &[u8]) -> Result<(tempfile::TempDir, Vec<String>), String> {
    let tree = tempfile::Builder::new()
        .prefix("horizon-setup-pins-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let write = |relative: &str, content: &[u8]| -> Result<(), String> {
        let target = tree.path().join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(target, content).map_err(|error| error.to_string())
    };
    write(DECLARATION, declaration)?;
    let commit = Commit::open(repository, &setup.head);
    // As the verifier reads it, a byte order mark first is allowed.
    let declared: Option<serde_json::Value> =
        serde_json::from_slice(declaration.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(declaration)).ok();
    // Every Dockerfile the profiles build, the default profile's first. One that cannot
    // be placed fails the check: skipping it would check a decoy instead.
    let mut dockerfiles: Vec<String> = Vec::new();
    if let Ok(config) = setup.config() {
        let default = config.profiles.get(&config.default).into_iter();
        for profile in default.chain(config.profiles.values()) {
            let Some(build) = &profile.build else { continue };
            let name = normal(&build.dockerfile).ok_or_else(|| {
                format!(
                    "the Dockerfile {:?} of a profile leaves the repository",
                    build.dockerfile
                )
            })?;
            if dockerfiles.contains(&name) {
                continue;
            }
            // A Dockerfile outside .horizon is built from the snapshot's commit.
            let content = match setup.file(&name) {
                Some(content) => content.to_vec(),
                None => commit
                    .as_ref()
                    .and_then(|commit| commit.blob(&name, SOURCE_BYTES))
                    .ok_or_else(|| format!("the Dockerfile {name} is in neither the setup nor its commit"))?,
            };
            write(&name, &content)?;
            dockerfiles.push(name);
        }
    }
    if dockerfiles.is_empty()
        && let Some(content) = setup.file(".horizon/Dockerfile")
    {
        write(".horizon/Dockerfile", content)?;
        dockerfiles.push(".horizon/Dockerfile".to_owned());
    }
    // The CI step runs the verifier with the declaration's own list, or .horizon/Dockerfile
    // without one, so that list must hold plain paths of exactly the built Dockerfiles,
    // the default profile's first; the verifier here then gets the same list.
    let listed: Vec<String> = match declared.as_ref().and_then(|value| value.get("dockerfiles")) {
        None => vec![".horizon/Dockerfile".to_owned()],
        Some(names) => names
            .as_array()
            .and_then(|names| {
                names
                    .iter()
                    .map(|name| {
                        name.as_str()
                            .filter(|name| normal(name).as_deref() == Some(*name))
                            .map(str::to_owned)
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .ok_or_else(|| format!("\"dockerfiles\" in {DECLARATION} must list plain paths in the repository"))?,
    };
    let same = |mut left: Vec<String>, mut right: Vec<String>| {
        left.sort();
        right.sort();
        left == right
    };
    if !dockerfiles.is_empty() && (listed.first() != dockerfiles.first() || !same(listed.clone(), dockerfiles.clone()))
    {
        return Err(format!(
            "{DECLARATION} names the Dockerfiles {listed:?}, but the profiles build {dockerfiles:?}, the default profile's first: list them in \"dockerfiles\" so the CI step checks the same files"
        ));
    }
    let dockerfiles = if dockerfiles.is_empty() { dockerfiles } else { listed };
    let names: Vec<String> = declared
        .as_ref()
        .and_then(|value| value.get("pins")?.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|pin| pin.get("file")?.as_str().map(str::to_owned))
        .take(SOURCE_LIMIT)
        .collect();
    for name in names {
        // As the verifier reads it in CI: `./global.json` is `global.json`, and a file in
        // `.horizon` is the snapshot's, everything else the commit's.
        let Some(name) = normal(&name) else { continue };
        if name == DECLARATION || name == format!(".horizon/{FILE}") {
            continue;
        }
        let content = source(setup, commit.as_ref(), &name);
        if let Some(content) = content {
            let _ = write(&name, &content);
        }
    }
    Ok((tree, dockerfiles))
}

/// The content of pin source `name` as the verifier reads it in CI: a file in `.horizon`
/// from the snapshot, any other from the commit, and a link followed inside the
/// repository, from either, up to a few links; one that leaves it reads as missing.
fn source(setup: &Setup, commit: Option<&Commit>, name: &str) -> Option<Vec<u8>> {
    let mut name = name.to_owned();
    // One link at a time: a path that enters .horizon reads the snapshot from there on,
    // which is what Use commits, and any other path the commit.
    for _ in 0..8 {
        let target = if name.starts_with(".horizon/") {
            match &setup.entries.iter().find(|(path, _)| *path == name)?.1 {
                Entry::File { content, .. } => {
                    return (content.len() as u64 <= SOURCE_BYTES).then(|| content.clone());
                }
                Entry::Link(target) => target.to_string_lossy().into_owned(),
                Entry::Folder => return None,
            }
        } else {
            match commit?.step(&name, SOURCE_BYTES)? {
                Step::Content(content) => return Some(content),
                Step::Link(target) => target,
            }
        };
        let parent = Path::new(&name).parent().unwrap_or(Path::new(""));
        name = normal(&parent.join(target).to_string_lossy())?;
    }
    None
}

/// `name` as a plain path in the repository, read the way the verifier and Docker read
/// it: `.` parts dropped and `..` parts applied. `None` when it leaves the repository,
/// is absolute or enters `.git`, spelled in any case or with the trailing dots and
/// spaces Windows drops.
fn normal(name: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in Path::new(name).components() {
        match part {
            Component::Normal(part) => parts.push(part.to_str()?),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    let git = |part: &&str| part.trim_end_matches(['.', ' ']).eq_ignore_ascii_case(".git");
    (!parts.is_empty() && !parts.iter().any(git)).then(|| parts.join("/"))
}

/// One step of a path in a commit.
pub(super) enum Step {
    Content(Vec<u8>),
    Link(String),
}

/// Files of one commit, read with libgit2: it never fetches a missing object, which a
/// partial clone's configured upload command would do, and runs nothing else the
/// repository's configuration names.
pub(super) struct Commit {
    repository: git2::Repository,
    tree: git2::Oid,
}

impl Commit {
    pub(super) fn open(repository: &Path, commit: &str) -> Option<Self> {
        if commit.is_empty() {
            return None;
        }
        let opened = git2::Repository::open(repository).ok()?;
        let tree = opened.revparse_single(commit).ok()?.peel_to_tree().ok()?.id();
        Some(Self {
            repository: opened,
            tree,
        })
    }

    /// The blob at `path`, when it is one and holds at most `limit` bytes.
    /// What is at `path` in the commit, one step: a blob of at most `limit` bytes, or a
    /// symbolic link's target, not followed.
    pub(super) fn step(&self, path: &str, limit: u64) -> Option<Step> {
        let tree = self.repository.find_tree(self.tree).ok()?;
        let entry = tree.get_path(Path::new(path)).ok()?;
        if entry.kind() != Some(git2::ObjectType::Blob) {
            return None;
        }
        // The size first, for a link too: git lets a link's blob hold anything.
        let (size, _) = self.repository.odb().ok()?.read_header(entry.id()).ok()?;
        let link = entry.filemode() == 0o120_000;
        if u64::try_from(size).ok()? > if link { LINK_BYTES } else { limit } {
            return None;
        }
        let content = self.repository.find_blob(entry.id()).ok()?.content().to_vec();
        Some(if link {
            Step::Link(String::from_utf8(content).ok()?)
        } else {
            Step::Content(content)
        })
    }

    /// A committed symbolic link is followed inside the commit, as a checkout would, up to
    /// a few links; one that leaves the repository reads as missing.
    pub(super) fn blob(&self, path: &str, limit: u64) -> Option<Vec<u8>> {
        let tree = self.repository.find_tree(self.tree).ok()?;
        let mut path = path.to_owned();
        let mut entry = tree.get_path(Path::new(&path)).ok()?;
        for _ in 0..8 {
            if entry.filemode() != 0o120_000 {
                break;
            }
            let (size, _) = self.repository.odb().ok()?.read_header(entry.id()).ok()?;
            if u64::try_from(size).ok()? > LINK_BYTES {
                return None;
            }
            let target = self.repository.find_blob(entry.id()).ok()?;
            let target = std::str::from_utf8(target.content()).ok()?;
            let parent = Path::new(&path).parent().unwrap_or(Path::new(""));
            path = normal(&parent.join(target).to_string_lossy())?;
            entry = tree.get_path(Path::new(&path)).ok()?;
        }
        if entry.kind() != Some(git2::ObjectType::Blob) || entry.filemode() == 0o120_000 {
            return None;
        }
        let (size, _) = self.repository.odb().ok()?.read_header(entry.id()).ok()?;
        if u64::try_from(size).ok()? > limit {
            return None;
        }
        Some(self.repository.find_blob(entry.id()).ok()?.content().to_vec())
    }

    /// The paths of the files called `name`, at most `depth` folders down and among
    /// the first `entries` tree entries, at most `files` of them; the flag says whether
    /// a limit cut the walk short.
    pub(super) fn files_named(
        &self,
        name: &str,
        (depth, entries, files): (usize, usize, usize),
    ) -> (Vec<String>, bool) {
        let Ok(tree) = self.repository.find_tree(self.tree) else {
            return (Vec::new(), false);
        };
        let (mut found, mut seen, mut cut) = (Vec::new(), 0_usize, false);
        let _ = tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
            seen += 1;
            if seen > entries || found.len() >= files {
                cut = true;
                return git2::TreeWalkResult::Abort;
            }
            let level = root.matches('/').count();
            match entry.kind() {
                Some(git2::ObjectType::Tree) if level >= depth => {
                    cut = true;
                    git2::TreeWalkResult::Skip
                }
                Some(git2::ObjectType::Blob) if entry.name().is_ok_and(|entry| entry == name) => {
                    found.push(format!("{root}{name}"));
                    git2::TreeWalkResult::Ok
                }
                _ => git2::TreeWalkResult::Ok,
            }
        });
        (found, cut)
    }
}

// Unix only: these tests run the verifier with python3 and make repositories with the
// git CLI, which Windows runners name and configure differently (py, CRLF checkouts),
// and several plant symbolic links. The check is the same code on every platform.
#[cfg(all(test, unix))]
mod tests;
