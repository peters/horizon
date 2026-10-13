//! The clones of the approved repositories on this computer, made with its own Git login
//! from addresses it checked, and how the proof container reads them.
use super::Access;
use super::locks::{LOCK_WALK, SOURCE_LIMIT, Source, locked_sources};
use super::watch::{SizeWatch, sweep};
use horizon_core::cloud_runtime::Cancellation;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Where the container finds the clones.
const MOUNT: &str = "/opt/host-cargo-git";

/// Where the container finds the Git system settings that trust the clones.
const TRUST: &str = "/opt/host-cargo-git.gitconfig";

/// Where a proof's clones go: a folder in Horizon's own home, so no folder above them
/// is one another account can write.
fn clone_parent() -> PathBuf {
    horizon_core::HorizonHome::resolve()
        .root()
        .join("cloud")
        .join("setup-fetch")
}

/// The most the clones of one proof may hold on this computer's disk.
const CLONE_BYTES: u64 = 4 << 30;

/// The longest cloning one repository on this computer may take.
const CLONE_TIMEOUT: Duration = Duration::from_mins(10);

/// The longest one of this computer's git questions about a clone may take.
const GIT_LIMIT: Duration = super::super::super::bounded::GIT;

/// The clones of one proof, in a private folder that goes with them.
pub(in crate::app::cloud_panel::production::repository_setup::proof) struct Mirrors {
    pub(super) _folder: Option<tempfile::TempDir>,
    /// Each cloned repository's folder name, folder and every address the lock files
    /// give it.
    pub(super) clones: Vec<(String, PathBuf, Vec<String>)>,
    /// The other GitHub addresses of the lock files, which Git must not send to a clone.
    pub(super) others: Vec<String>,
    /// What was left out and why, for the proof log.
    pub(in crate::app::cloud_panel::production::repository_setup::proof) notes: Vec<String>,
    /// The Git system settings that trust the clones in the container, from `trust`.
    pub(super) trust: Option<PathBuf>,
}

impl Mirrors {
    /// The clones for a proof of `commit`: each approved repository cloned through
    /// `stream` (which shows and limits the command), and what was left out said through
    /// `say`. Best effort, as for packages: a failure here shows up in the CI step.
    pub(in crate::app::cloud_panel::production::repository_setup::proof) fn for_proof(
        (repository, commit, access, cancel): (&Path, Option<&str>, &Access, &Cancellation),
        mut stream: impl FnMut(Command, Duration) -> Result<(), String>,
        mut say: impl FnMut(String),
    ) -> Option<Self> {
        let commit = commit?;
        let allowed = &access.allowed;
        let approved: &[String] = if allowed.load(std::sync::atomic::Ordering::Relaxed) {
            &access.repositories
        } else {
            &[]
        };
        match Self::clone(
            (repository, commit, &clone_parent(), cancel),
            (approved, allowed),
            |clone| stream(clone, CLONE_TIMEOUT),
        ) {
            Ok(mirrors) => {
                for note in mirrors.iter().flat_map(|mirrors| &mirrors.notes) {
                    say(format!("Rust git dependencies: {note}"));
                }
                mirrors
            }
            Err(error) => {
                say(format!("Rust git dependencies not cloned here: {error}"));
                None
            }
        }
    }

    /// Clones each approved repository that the lock files in `commit` name, once, with
    /// this computer's Git login, through `run` (which streams and limits the command),
    /// and keeps the clones that hold every locked commit, in a new folder in `parent`.
    /// A repository that fails is left out with a note; the others stay. `None` when
    /// none is approved.
    pub(super) fn clone(
        (repository, commit, parent, cancel): (&Path, &str, &Path, &Cancellation),
        (approved, allowed): (&[String], &std::sync::atomic::AtomicBool),
        mut run: impl FnMut(Command) -> Result<(), String>,
    ) -> Result<Option<Self>, String> {
        // Folders a Horizon that stopped mid-proof left go after a day, cloning or not.
        sweep(parent);
        let (sources, cut) = locked_sources(repository, commit);
        let mut repositories: Vec<Vec<Source>> = Vec::new();
        let mut notes = Vec::new();
        if cut {
            notes.push(format!(
                "not every Cargo.lock was read: the search stops {} folders down, after {} entries or {} lock files, and at {SOURCE_LIMIT} git sources",
                LOCK_WALK.0, LOCK_WALK.1, LOCK_WALK.2
            ));
        }
        for source in &sources {
            if !approved.contains(&source.key()) {
                if !notes.iter().any(|note: &String| note.starts_with(&source.key())) {
                    let why = if approved.is_empty() {
                        "the box to fetch private Rust git dependencies is not checked"
                    } else {
                        "the box did not name it"
                    };
                    notes.push(format!("{} not cloned here: {why}", source.key()));
                }
                continue;
            }
            match repositories.iter_mut().find(|group| group[0].key() == source.key()) {
                Some(group) => group.push(source.clone()),
                None => repositories.push(vec![source.clone()]),
            }
        }
        if repositories.is_empty() {
            // Nothing to clone; what was left out is still said, so a CI step that needs
            // a private repository has its reason in the log.
            return Ok((!notes.is_empty()).then(|| Self {
                _folder: None,
                clones: Vec::new(),
                others: Vec::new(),
                notes,
                trust: None,
            }));
        }
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let mut builder = tempfile::Builder::new();
        builder.prefix("crates-");
        // Only this user reaches the clones; each mirror in it opens to the container's.
        #[cfg(unix)]
        builder.permissions(<std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(
            0o700,
        ));
        let folder = builder.tempdir_in(parent).map_err(|error| error.to_string())?;
        if !mountable(folder.path()) {
            return Err("Horizon's folder has a name Docker cannot mount".to_owned());
        }
        let empty_config = folder.path().join("empty.gitconfig");
        std::fs::write(&empty_config, "").map_err(|error| error.to_string())?;
        // Settings that cannot be read in time clone nothing: a clone without them could
        // not sign in anyway, and Git that does not answer is no Git to trust.
        let sign_in = sign_in_settings(folder.path(), GIT_LIMIT)
            .map_err(|error| format!("cannot read this computer's Git sign-in settings: {error}"))?;
        // The clones share one bound on this computer's disk: past it, the proof stops.
        // Nothing between here and its stop returns early.
        let watch = SizeWatch::start(folder.path(), CLONE_BYTES, cancel.clone());
        let mut clones = Vec::new();
        for group in repositories {
            // A stopped proof, by the person or the size bound, starts no further clone.
            if cancel.is_cancelled() {
                break;
            }
            match clone_one(
                (folder.path(), &empty_config, &sign_in),
                (group, allowed, cancel),
                &mut run,
            ) {
                Ok(clone) => clones.push(clone),
                Err(note) => notes.push(note),
            }
        }
        if watch.stop() {
            return Err(format!(
                "the clones grew past {} GiB on this computer, so the proof stopped",
                CLONE_BYTES >> 30
            ));
        }
        if cancel.is_cancelled() {
            return Err("the proof was stopped".to_owned());
        }
        let mut others: Vec<String> = sources
            .into_iter()
            .map(|source| source.address)
            .filter(|address| !clones.iter().any(|(_, _, addresses)| addresses.contains(address)))
            .collect();
        others.sort();
        others.dedup();
        let trust = if clones.is_empty() {
            None
        } else {
            Some(trust(folder.path(), &clones).map_err(|error| format!("cannot write the Git settings: {error}"))?)
        };
        Ok(Some(Self {
            _folder: Some(folder),
            clones,
            others,
            notes,
            trust,
        }))
    }

    /// The `docker run` arguments: each clone mounted read-only, and Git in the
    /// container told to read it in place of its addresses. Git rewrites by the longest
    /// matching prefix, so each other address that starts with a cloned one maps to
    /// itself. Settings go in as environment, so nothing is written to the shared home
    /// and nothing passes through a shell.
    pub(in crate::app::cloud_panel::production::repository_setup::proof) fn docker_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        let mut settings = Vec::new();
        for (folder, path, addresses) in &self.clones {
            let inside = format!("{MOUNT}/{folder}");
            args.extend(["-v".to_owned(), format!("{}:{inside}:ro", path.display())]);
            for address in addresses {
                settings.push((format!("url.file://{inside}.insteadOf"), address.clone()));
            }
            settings.push(("safe.directory".to_owned(), inside));
        }
        // The CI runs as another user than the one who owns the clones, so Git must trust
        // them. The environment above reaches Git, but Git drops it for the upload-pack it
        // starts to fetch from a local clone, which then refuses it as of dubious
        // ownership: the same trust goes in a system settings file, which reaches both.
        if let Some(trust) = &self.trust {
            args.extend(["-v".to_owned(), format!("{}:{TRUST}:ro", trust.display())]);
            args.extend(["-e".to_owned(), format!("GIT_CONFIG_SYSTEM={TRUST}")]);
        }
        for other in &self.others {
            let captured = self
                .clones
                .iter()
                .flat_map(|(_, _, addresses)| addresses)
                .any(|address| other.starts_with(address.as_str()));
            if captured {
                settings.push((format!("url.{other}.insteadOf"), other.clone()));
            }
        }
        if settings.is_empty() {
            return args;
        }
        args.extend(["-e".to_owned(), "CARGO_NET_GIT_FETCH_WITH_CLI=true".to_owned()]);
        args.extend(["-e".to_owned(), format!("GIT_CONFIG_COUNT={}", settings.len())]);
        for (index, (key, value)) in settings.into_iter().enumerate() {
            args.extend(["-e".to_owned(), format!("GIT_CONFIG_KEY_{index}={key}")]);
            args.extend(["-e".to_owned(), format!("GIT_CONFIG_VALUE_{index}={value}")]);
        }
        args
    }
}

/// Writes in `folder` the Git system settings the container reads: the image's own
/// `/etc/gitconfig`, and trust in each clone at its path in the container. Readable to
/// the container's user; the folder stays closed to other users of this computer.
pub(super) fn trust(folder: &Path, clones: &[(String, PathBuf, Vec<String>)]) -> std::io::Result<PathBuf> {
    let mut text = String::from("[include]\n\tpath = /etc/gitconfig\n[safe]\n");
    for (name, _, _) in clones {
        let _ = writeln!(text, "\tdirectory = {MOUNT}/{name}");
    }
    let path = folder.join("system.gitconfig");
    std::fs::write(&path, text)?;
    readable_to_all(&path)?;
    Ok(path)
}

/// Whether Docker's `-v SOURCE:TARGET:ro` reads `path` as one source: no colon after a
/// Windows drive letter, since only colons part the fields.
pub(super) fn mountable(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    let rest = if cfg!(windows) && bytes.len() > 1 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        &text[2..]
    } else {
        &text[..]
    };
    !rest.contains(':')
}

/// A `git` command in `folder`, Horizon's own folder that is no repository, that never
/// looks above it for one: a repository Horizon runs inside, or one around its data
/// folder, must not lend its settings, such as a `url.*.insteadOf` an agent wrote.
fn git_outside(folder: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(folder).env("GIT_TERMINAL_PROMPT", "0");
    if let Some(parent) = folder.parent() {
        command.env("GIT_CEILING_DIRECTORIES", parent);
    }
    for name in ["GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR"] {
        command.env_remove(name);
    }
    command
}

/// Runs one git command of the clones within `limit`: its exit status and output, or why
/// it gave none. A configuration Git reads can name an `include` that leads to a FIFO,
/// which holds any git command that reads it; past `limit` the command is ended with
/// everything it started, and the caller refuses what it asked for.
fn bounded_git(command: Command, limit: Duration) -> Result<std::process::Output, String> {
    super::super::super::bounded::output(command, None, limit)
}

/// The address Git would fetch for `address` after this computer's URL rewrites, asked
/// in `folder` without the network within `limit`: none when Git names none, and an
/// error when it gave no answer in time, so a held configuration clones nothing.
pub(super) fn effective_address(address: &str, folder: &Path, limit: Duration) -> Result<Option<String>, String> {
    let mut command = git_outside(folder);
    command.args(["ls-remote", "--get-url", "--", address]);
    let output = bounded_git(command, limit)?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()))
}

/// This computer's Git settings that signing in to GitHub needs: credential helpers,
/// HTTP and SSH settings, read once from the user's and the system's configuration (run
/// in `folder`, which is no repository, so no repository's settings join them), within
/// `limit`. `--get-regexp` exits 1 when nothing matches, so only a command that gave no
/// answer is an error.
pub(super) fn sign_in_settings(folder: &Path, limit: Duration) -> Result<Vec<(String, String)>, String> {
    let mut command = git_outside(folder);
    command
        .args(["config", "--includes", "--null", "--get-regexp"])
        .arg(r"^(credential|http)\.|^core\.(sshcommand|askpass)$");
    bounded_git(command, limit).map(|output| config_records(&output.stdout))
}

/// Whether the clone at `target` holds `commit`, asked within `limit`; an error when Git
/// gave no answer in time.
pub(super) fn holds_commit(target: &Path, commit: &str, limit: Duration) -> Result<bool, String> {
    let mut command = git(target);
    command.args(["cat-file", "-e", &format!("{commit}^{{commit}}")]);
    bounded_git(command, limit).map(|output| output.status.success())
}

/// The `name` and `value` of each record of `git config --null --get-regexp`.
pub(super) fn config_records(output: &[u8]) -> Vec<(String, String)> {
    output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| {
            let record = String::from_utf8_lossy(record);
            let (name, value) = record.split_once('\n').unwrap_or((&record, ""));
            (name.to_owned(), value.to_owned())
        })
        .collect()
}

/// `owner/name` in lower case for a GitHub address over HTTPS or SSH, in any of the
/// forms Git takes; `None` for any other host.
pub(super) fn github_key(address: &str) -> Option<String> {
    let (rest, scp) = match address.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("ssh") => {
            (rest, false)
        }
        Some(_) => return None,
        None => (address, true),
    };
    // The host, after any user information, and the path: `host/path`, or `host:path`
    // in Git's short SSH form.
    let (authority, path) = if scp {
        rest.split_once(':')?
    } else {
        rest.split_once('/')?
    };
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    if !host.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let (owner, name) = path.trim_end_matches('/').split_once('/')?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    (!owner.is_empty() && !name.is_empty() && !name.contains('/'))
        .then(|| format!("{owner}/{name}").to_ascii_lowercase())
}

/// The address to give `git clone` for `effective` and, when `effective` carries user
/// information such as a token, the rule that sends the plain address to it: the token
/// then goes to Git as environment only, never on the command line.
pub(super) fn clone_address(effective: &str, key: &str) -> (String, Option<(String, String)>) {
    let userinfo = effective
        .split_once("://")
        .and_then(|(_, rest)| rest.split('/').next())
        .is_some_and(|authority| authority.contains('@'));
    if effective.to_ascii_lowercase().starts_with("https://") && userinfo {
        let plain = format!("https://github.com/{key}");
        (plain.clone(), Some((format!("url.{effective}.insteadOf"), plain)))
    } else {
        (effective.to_owned(), None)
    }
}

/// Clones one approved repository into `folder` through `run`, and returns its folder
/// name, folder and addresses, or the note that says why it is left out. Each address
/// the locks give it is tried in turn, since this computer may sign in to only one of
/// them, such as SSH and not HTTPS.
fn clone_one(
    (folder, empty_config, sign_in): (&Path, &Path, &[(String, String)]),
    (group, allowed, cancel): (Vec<Source>, &std::sync::atomic::AtomicBool, &Cancellation),
    run: &mut impl FnMut(Command) -> Result<(), String>,
) -> Result<(String, PathBuf, Vec<String>), String> {
    let key = group[0].key();
    // `owner/name.git`: an owner holds no slash, so no two repositories share it.
    let folder_name = format!("{key}.git");
    let target = folder.join(&folder_name);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("{key} not cloned: {error}"))?;
    }
    let mut addresses: Vec<String> = group.iter().map(|source| source.address.clone()).collect();
    addresses.sort();
    addresses.dedup();
    let mut failures: Vec<String> = Vec::new();
    for address in &addresses {
        // Clearing the box while the proof runs stops every clone not begun yet.
        if !allowed.load(std::sync::atomic::Ordering::Relaxed) {
            failures.push("the box was cleared".to_owned());
            break;
        }
        if cancel.is_cancelled() {
            failures.push("the proof was stopped".to_owned());
            break;
        }
        // A failed clone can leave part of its folder behind.
        if target.exists() {
            let _ = std::fs::remove_dir_all(&target);
        }
        match clone_from((address, &target, &key), (folder, empty_config, sign_in), run) {
            Ok(()) => {
                failures.clear();
                break;
            }
            Err(failure) => {
                if !failures.contains(&failure) {
                    failures.push(failure);
                }
            }
        }
    }
    if !failures.is_empty() {
        return Err(format!("{key} not cloned: {}", failures.join("; ")));
    }
    for source in &group {
        if !holds_commit(&target, &source.commit, GIT_LIMIT).map_err(|error| format!("{key} not checked: {error}"))? {
            return Err(format!("{key} lacks the locked commit {}", source.commit));
        }
    }
    readable_to_all(&target).map_err(|error| format!("{key} not cloned: {error}"))?;
    Ok((folder_name, target, addresses))
}

/// Lets every user read the mirror at `target`: the proof container reads it as UID
/// 10001, and Git made it with this computer's umask, which may be 077. Its private
/// `crates-*` parent stays closed to other users of this computer. Never follows a link.
#[cfg(unix)]
pub(super) fn readable_to_all(target: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut pending = vec![target.to_path_buf()];
    while let Some(next) = pending.pop() {
        let metadata = std::fs::symlink_metadata(&next)?;
        let mode = metadata.permissions().mode();
        let wanted = if metadata.is_dir() {
            pending.extend(
                std::fs::read_dir(&next)?
                    .map(|entry| entry.map(|entry| entry.path()))
                    .collect::<Result<Vec<_>, _>>()?,
            );
            mode | 0o555
        } else if metadata.is_file() {
            mode | 0o444
        } else {
            continue;
        };
        if wanted != mode {
            std::fs::set_permissions(&next, std::fs::Permissions::from_mode(wanted))?;
        }
    }
    Ok(())
}

/// Windows has no mode bits to open; Docker Desktop reads the mirror as this user.
#[cfg(not(unix))]
pub(super) fn readable_to_all(_target: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Clones the locked `address` of the approved repository `key` into `target`, or says
/// why it did not.
fn clone_from(
    (address, target, key): (&str, &Path, &str),
    (folder, empty_config, sign_in): (&Path, &Path, &[(String, String)]),
    run: &mut impl FnMut(Command) -> Result<(), String>,
) -> Result<(), String> {
    // Git rewrites an address by `url.*.insteadOf` in this computer's own configuration,
    // often to SSH: the clone must still come from the approved GitHub repository. The
    // rewritten address is not shown: it can carry a credential.
    let effective = effective_address(address, folder, GIT_LIMIT)
        .map_err(|error| format!("Git did not name the address it fetches: {error}"))?;
    let Some(effective) = effective.filter(|address| github_key(address).as_deref() == Some(key)) else {
        return Err(
            "a url.insteadOf rule in this computer's Git configuration sends it outside the approved repository"
                .to_owned(),
        );
    };
    let (address, frozen) = clone_address(&effective, key);
    // The validated address is cloned without this computer's Git configuration, so no
    // rule written after the check can send it elsewhere; only the sign-in settings go
    // along, as environment, never on the command line.
    // In Horizon's own folder, like the check, so no repository around it adds a rule.
    let mut clone = git_outside(folder);
    clone
        .args(["-c", "core.hooksPath=/dev/null", "-c", "protocol.allow=never"])
        .args(["-c", "protocol.https.allow=always", "-c", "protocol.ssh.allow=always"])
        .args(["clone", "--mirror", "--quiet", "--", &address])
        .arg(target)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", empty_config);
    let settings: Vec<&(String, String)> = sign_in.iter().chain(frozen.as_ref()).collect();
    clone.env("GIT_CONFIG_COUNT", settings.len().to_string());
    for (index, (name, value)) in settings.into_iter().enumerate() {
        clone
            .env(format!("GIT_CONFIG_KEY_{index}"), name)
            .env(format!("GIT_CONFIG_VALUE_{index}"), value);
    }
    run(clone)
}

/// A git command that runs no hook or file monitor a repository's configuration names.
/// Only for the clones Horizon made itself.
fn git(directory: &Path) -> Command {
    super::super::super::commit::git_command(directory)
}
