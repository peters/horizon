//! Talking to a repository's host with system Git: looking at it without cloning, and fetching it
//! in steps that a failure or a cancel does not throw away, so a later try picks up from what was
//! already received. Git cannot resume inside one pack, so each step is one pack: the latest
//! commit first, then the rest of the history, then the checkout.
use super::{Cancellation, Failure, Remote, Token, candidates, classify, earlier, git_output};
use std::{
    fs::{File, OpenOptions, TryLockError},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

/// Where a running clone stands, for showing it: the step, Git's phase, how far, and when that
/// phase should end.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    /// The step under way, from 1, and how many there are; 0 before the first starts.
    pub step: u8,
    pub steps: u8,
    /// This clone picks up what an earlier try received.
    pub resumed: bool,
    /// Git's own name for what it is doing, such as `Receiving objects`.
    pub phase: String,
    pub percent: Option<u8>,
    /// Git's size and speed for the phase, such as `12.30 MiB | 4.50 MiB/s`.
    pub detail: String,
    /// How long the phase has left at the pace it has kept, once there is a pace to go by.
    pub eta: Option<Duration>,
}

/// Shared between the clone and whoever shows it.
pub type Progress = Arc<Mutex<Snapshot>>;

fn update(progress: &Progress, change: impl FnOnce(&mut Snapshot)) {
    change(&mut progress.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
}

/// How far back each fetch of a clone reaches into the history, the last one (0) taking all of it
/// with every branch and tag. Git cannot resume inside one download, so the history comes in
/// pieces: each finished piece is kept when a later one fails.
const DEPTHS: [u32; 4] = [1, 100, 3000, 0];

/// The steps of a clone: one fetch for each of [`DEPTHS`], then the checkout.
const STEPS: u8 = 5;

/// A file inside `.git` that says a clone was started here and not finished: the address and the
/// branch it is fetching. Its presence is what makes the folder one that a later try may resume,
/// and one that is ours to remove.
const MARKER: &str = "horizon-clone";

/// The hidden folder, beside the folders being cloned, that holds one small file for each of them.
/// A running clone holds an exclusive lock on its file: a second Horizon process then cannot resume
/// or remove the folder underneath it, and the lock stays put while the folder is deleted. The
/// system drops the lock when the process ends, however it ends. The files are never removed: a
/// lock file that is unlinked while another process waits on it stops excluding anything.
const CLAIMS: &str = ".horizon-clone-claims";

/// How often, and how far apart, a folder that is held is tried before it is called busy.
const CLAIM_TRIES: u32 = 30;
const CLAIM_WAIT: Duration = Duration::from_millis(10);

/// How long a finished clone waits for what Git said last; a helper it left behind may hold the pipe.
const CLONE_GRACE: Duration = Duration::from_secs(2);

/// The most of a command's stdout that is read; what a probe asks for is a line or two.
const STDOUT_LIMIT: u64 = 64 * 1024;

/// The longest a probe may take before the host is taken to be out of reach.
const PROBE_LIMIT: Duration = Duration::from_secs(20);

/// The longest line of Git's progress kept; a remote that never ends one cannot grow it further.
const LINE_LIMIT: usize = 4096;

/// The most of Git's stderr that is kept: enough to name a failure, and all a host that never
/// stops talking can make Horizon hold.
const STDERR_TAIL: usize = 16 * 1024;

/// `git` set up never to prompt, and to carry `token` when there is one.
fn git(token: Option<&Token>) -> Command {
    let mut command = Command::new("git");
    command
        .args(["-c", "http.lowSpeedLimit=1000", "-c", "http.lowSpeedTime=30"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(token) = token {
        command
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", format!("http.{}.extraHeader", token.scope))
            .env("GIT_CONFIG_VALUE_0", token.header());
    }
    // Never ask questions over SSH, but keep a command the person has set up themselves.
    if !ssh_command_is_set() {
        command.env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o StrictHostKeyChecking=yes -o ConnectTimeout=15",
        );
    }
    command
}

/// Whether the person has an SSH command of their own, in the environment or in Git's system or
/// user configuration. A repository's own configuration is not looked at: a new clone has none, and
/// the folder Horizon happens to run in is not where the clone runs.
fn ssh_command_is_set() -> bool {
    static SET: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SET.get_or_init(|| {
        ssh_command_in(std::env::var_os("GIT_SSH_COMMAND").is_some(), |scope| {
            git_output(Path::new("."), &["config", scope, "--get", "core.sshCommand"])
        })
    })
}

fn ssh_command_in(in_environment: bool, configured: impl Fn(&str) -> Option<String>) -> bool {
    in_environment
        || ["--system", "--global"]
            .iter()
            .any(|scope| configured(scope).is_some_and(|set| !set.trim().is_empty()))
}

/// What Git's stderr line says about progress: its phase, how far, and the size and speed.
fn parse_progress(line: &str) -> Option<(String, u8, String)> {
    let line = line.trim().trim_start_matches("remote:").trim();
    let (phase, rest) = line.split_once(':')?;
    let rest = rest.trim();
    let percent: u8 = rest[..rest.find('%')?].trim().parse().ok()?;
    let after = &rest[rest.find('%')? + 1..];
    let detail = after.split_once(')').map_or(after, |(_, tail)| tail);
    let detail = detail
        .trim()
        .trim_start_matches(',')
        .trim()
        .trim_end_matches("done.")
        .trim()
        .trim_end_matches(',')
        .trim();
    // Only a size and a speed say something; Git's closing remarks do not.
    let detail = if detail.starts_with(|c: char| c.is_ascii_digit()) {
        detail
    } else {
        ""
    };
    Some((phase.trim().to_owned(), percent.min(100), detail.to_owned()))
}

/// Whether `line` only reports how a transfer is going, which says nothing about why one failed.
fn is_chatter(line: &str) -> bool {
    let line = line.trim().trim_start_matches("remote:").trim();
    [
        "Enumerating objects",
        "Counting objects",
        "Compressing objects",
        "Total ",
        "Receiving objects",
        "Resolving deltas",
        "Updating files",
        "Checking out",
        "From ",
        "* ",
    ]
    .iter()
    .any(|start| line.starts_with(start))
}

/// How long a phase has left, judged by how long its first `percent` took. Too early to tell is `None`.
fn eta(elapsed: Duration, percent: u8) -> Option<Duration> {
    if !(2..100).contains(&percent) || elapsed < Duration::from_secs(1) {
        return None;
    }
    Some(elapsed.mul_f64(f64::from(100 - percent) / f64::from(percent)))
}

/// Appends `text` to `tail` and drops the oldest text beyond [`STDERR_TAIL`].
fn keep_tail(tail: &mut String, text: &str) {
    tail.push_str(text);
    if tail.len() > STDERR_TAIL {
        let mut cut = tail.len() - STDERR_TAIL;
        while !tail.is_char_boundary(cut) {
            cut += 1;
        }
        tail.drain(..cut);
    }
}

/// What `watch` has seen of one Git process: the phase under way, since when, and what Git said
/// beyond progress.
struct Watched {
    phase: String,
    phase_started: Instant,
    said: String,
}

impl Watched {
    /// Takes one whole line of Git's stderr: progress is shown, chatter dropped, and anything
    /// else kept as what Git said.
    fn line(&mut self, line: &str, progress: &Progress) {
        if line.trim().is_empty() {
            return;
        }
        if let Some((name, percent, detail)) = parse_progress(line) {
            // Progress is not a reason for anything: only what Git says otherwise is kept.
            if name != self.phase {
                self.phase.clone_from(&name);
                self.phase_started = Instant::now();
            }
            let left = eta(self.phase_started.elapsed(), percent);
            update(progress, |snapshot| {
                snapshot.phase = name;
                snapshot.percent = Some(percent);
                snapshot.detail = detail;
                snapshot.eta = left;
            });
        } else if !is_chatter(line) {
            keep_tail(&mut self.said, line);
            keep_tail(&mut self.said, "\n");
        }
    }
}

/// Reads Git's stderr to its end, showing what it says about progress and keeping the tail of it.
fn watch(mut stderr: Option<std::process::ChildStderr>, progress: &Progress, done: &mpsc::Sender<String>) {
    let (mut line, mut buffer) = (String::new(), [0_u8; 512]);
    let mut watched = Watched {
        phase: String::new(),
        phase_started: Instant::now(),
        said: String::new(),
    };
    while let Some(read) = stderr
        .as_mut()
        .and_then(|pipe| pipe.read(&mut buffer).ok())
        .filter(|n| *n > 0)
    {
        for character in String::from_utf8_lossy(&buffer[..read]).chars() {
            if matches!(character, '\r' | '\n') {
                watched.line(&line, progress);
                line.clear();
            } else if line.len() < LINE_LIMIT {
                line.push(character);
            }
        }
    }
    // A last word without a line end is still a word: it is often the reason.
    watched.line(&line, progress);
    let _ = done.send(watched.said);
}

/// Ends `child` and lets go of its stderr reader. A transport helper that Git started may
/// outlive it and keep the pipe open, so waiting for the reader could outlast any deadline;
/// the reader ends on its own when the pipe closes.
fn stop(child: &mut std::process::Child, reader: mpsc::Receiver<String>) {
    let _ = child.kill();
    let _ = child.wait();
    drop(reader);
}

/// What Git wrote to stderr, waiting no longer than `wait` for the reader to reach the end of
/// the pipe: a helper Git left behind can hold it open, and then there is nothing more to read.
fn collected(reader: &mpsc::Receiver<String>, wait: Duration) -> String {
    reader.recv_timeout(wait).unwrap_or_default()
}

/// Whether `remote` can be read without signing in, before anything is cloned. It ends when
/// `cancel` is raised and after [`PROBE_LIMIT`], whatever the network is doing.
///
/// # Errors
/// [`Failure::SignIn`] when it is private, or when the host hides a missing one the same way.
pub fn probe(remote: &Remote, token: Option<&Token>, cancel: &Cancellation) -> Result<(), Failure> {
    let mut command = git(token);
    command
        .args(["ls-remote", "--", &remote.url, "HEAD"])
        .stdout(Stdio::null());
    bounded(command, remote, cancel, PROBE_LIMIT).map(drop)
}

/// Runs `command`, ending it at `limit` or when `cancel` is raised, and gives back what it wrote
/// to stdout if that was piped.
fn bounded(mut command: Command, remote: &Remote, cancel: &Cancellation, limit: Duration) -> Result<String, Failure> {
    let mut child = command
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| Failure::Other(format!("Cannot run git: {error}")))?;
    let mut stdout = child.stdout.take();
    let (out_sender, out) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.take(STDOUT_LIMIT).read_to_string(&mut text);
        }
        let _ = out_sender.send(text);
    });
    let mut stderr = child.stderr.take();
    let (sender, reader) = mpsc::channel();
    std::thread::spawn(move || {
        let mut tail = String::new();
        let mut buffer = [0_u8; 512];
        while let Some(read) = stderr
            .as_mut()
            .and_then(|pipe| pipe.read(&mut buffer).ok())
            .filter(|n| *n > 0)
        {
            keep_tail(&mut tail, &String::from_utf8_lossy(&buffer[..read]));
        }
        let _ = sender.send(tail);
    });
    let started = Instant::now();
    let status = loop {
        if cancel.is_cancelled() || started.elapsed() >= limit {
            stop(&mut child, reader);
            return Err(if cancel.is_cancelled() {
                Failure::Cancelled
            } else {
                Failure::Network
            });
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                stop(&mut child, reader);
                return Err(Failure::Other(error.to_string()));
            }
        }
    };
    // A helper Git started may keep either pipe open past its own end: neither is waited for beyond the limit.
    let stderr = collected(&reader, limit.saturating_sub(started.elapsed()));
    if status.success() {
        Ok(collected(&out, limit.saturating_sub(started.elapsed())))
    } else {
        Err(classify(remote, &stderr))
    }
}

/// The branch a clone of `remote` checks out, named by the host's `HEAD`.
fn default_branch(remote: &Remote, token: Option<&Token>, cancel: &Cancellation) -> Result<String, Failure> {
    let mut command = git(token);
    command.args(["ls-remote", "--symref", "--", &remote.url, "HEAD"]);
    let listing = bounded(command, remote, cancel, PROBE_LIMIT)?;
    listing
        .lines()
        .find_map(|line| line.strip_prefix("ref: refs/heads/")?.split_whitespace().next())
        .filter(|branch| !branch.starts_with('-') && !branch.contains(['\0', '\\']))
        .map(str::to_owned)
        .ok_or_else(|| Failure::Other("This repository has no commits to clone yet.".into()))
}

/// What a folder's marker says: the address being cloned, the branch, and how many of the fetches
/// in [`DEPTHS`] are done.
struct Marker {
    url: String,
    branch: String,
    done: usize,
}

fn marker(folder: &Path) -> Option<Marker> {
    let text = std::fs::read_to_string(folder.join(".git").join(MARKER)).ok()?;
    let mut lines = text.lines();
    Some(Marker {
        url: lines.next()?.to_owned(),
        branch: lines.next()?.to_owned(),
        done: lines.next()?.parse().ok()?,
    })
}

/// Writes the marker whole or not at all: a process ended in the middle of a write leaves the
/// marker before it, never half of the one after.
impl Marker {
    /// Every fetch and the checkout are done; only the removal of the marker was left.
    fn complete(&self) -> bool {
        self.done > DEPTHS.len()
    }
}

fn write_marker(folder: &Path, marker: &Marker) -> Result<(), Failure> {
    let git = folder.join(".git");
    let staged = git.join(format!("{MARKER}.tmp"));
    std::fs::write(&staged, format!("{}\n{}\n{}\n", marker.url, marker.branch, marker.done))
        .and_then(|()| std::fs::rename(&staged, git.join(MARKER)))
        .map_err(|error| Failure::Other(error.to_string()))
}

/// Where the claim on `folder` lives: a file named for it, in a hidden folder beside it.
fn claim_path(folder: &Path) -> Option<PathBuf> {
    Some(folder.parent()?.join(CLAIMS).join(folder.file_name()?))
}

/// The hold a running clone keeps on its folder.
struct Claim {
    /// Held open for as long as the clone runs.
    _file: File,
}

impl Claim {
    /// Takes the folder for this process, or says another one has it.
    fn take(folder: &Path) -> Result<Self, Failure> {
        let path =
            claim_path(folder).ok_or_else(|| Failure::Other("That folder has no place to be cloned into.".into()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| Failure::Other(error.to_string()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| Failure::Other(error.to_string()))?;
        // A process that is being started elsewhere can hold a copy of the lock for a moment after
        // its owner let go, so "busy" is only said after waiting that long.
        for attempt in 0..CLAIM_TRIES {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(TryLockError::WouldBlock) if attempt + 1 < CLAIM_TRIES => std::thread::sleep(CLAIM_WAIT),
                Err(TryLockError::WouldBlock) => break,
                Err(TryLockError::Error(error)) => return Err(Failure::Other(error.to_string())),
            }
        }
        Err(Failure::Other(
            "Another Horizon window is cloning into that folder. Wait for it, or choose another folder.".into(),
        ))
    }
}

/// Whether `folder` is a clone that was started and not finished.
pub(super) fn unfinished(folder: &Path) -> bool {
    marker(folder).is_some_and(|marker| !marker.complete())
}

/// A folder under `parent` where an earlier try at cloning `remote` stopped, and that a new try
/// picks up from what it received.
#[must_use]
pub fn resumable(parent: &Path, remote: &Remote) -> Option<PathBuf> {
    // A link is not a candidate: two parents could reach one checkout by different names, and the
    // claim on it is kept by name.
    candidates(parent, remote)
        .chain(earlier(parent, remote))
        .filter(|path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()))
        .find(|path| marker(path).is_some_and(|marker| marker.url == remote.url && !marker.complete()))
}

/// Gives up on a clone that stopped, removing what it received. Only a folder a clone started
/// and did not finish is touched, and not one that another Horizon window is working on.
///
/// # Errors
/// Says so when another process holds the folder, or when it could not be removed.
pub fn discard(folder: &Path) -> Result<(), Failure> {
    if !unfinished(folder) {
        return Ok(());
    }
    let claim = Claim::take(folder)?;
    // Asked again once held: another window may have finished or removed it in the meantime.
    let removed = if unfinished(folder) {
        std::fs::remove_dir_all(folder)
    } else {
        Ok(())
    };
    drop(claim);
    removed.map_err(|error| Failure::Other(format!("Could not remove {}: {error}", folder.display())))
}

/// Removes what a Git process that was ended mid-step leaves in a clone's own folder: lock files
/// that would make the next try refuse to start, and pack files that were never finished. Only a
/// folder a clone started is ever passed here, and never while a Git process of it runs.
fn clear_leftovers(folder: &Path) {
    fn locks(dir: &Path) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            // A link is never followed: only what the checkout itself holds is ours to clean.
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => locks(&path),
                Ok(kind) if kind.is_file() && path.extension().is_some_and(|extension| extension == "lock") => {
                    let _ = std::fs::remove_file(path);
                }
                _ => {}
            }
        }
    }
    let git = folder.join(".git");
    for name in [
        "shallow.lock",
        "index.lock",
        "HEAD.lock",
        "config.lock",
        "packed-refs.lock",
        "horizon-clone.tmp",
    ] {
        let _ = std::fs::remove_file(git.join(name));
    }
    locks(&git.join("refs"));
    for entry in std::fs::read_dir(git.join("objects").join("pack"))
        .into_iter()
        .flatten()
        .flatten()
    {
        if entry.file_name().to_string_lossy().starts_with("tmp_pack_") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Runs one Git command of a clone in `folder`, showing its progress and ending it when `cancel`
/// is raised.
fn run(
    mut command: Command,
    folder: &Path,
    remote: &Remote,
    cancel: &Cancellation,
    progress: &Progress,
) -> Result<(), Failure> {
    let mut child = command
        .current_dir(folder)
        .stdout(Stdio::null())
        .spawn()
        .map_err(|error| Failure::Other(format!("Cannot run git: {error}")))?;
    let (sender, reader) = mpsc::channel();
    std::thread::spawn({
        let (stderr, progress) = (child.stderr.take(), Arc::clone(progress));
        move || watch(stderr, &progress, &sender)
    });
    let status = loop {
        if cancel.is_cancelled() {
            stop(&mut child, reader);
            clear_leftovers(folder);
            return Err(Failure::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                stop(&mut child, reader);
                clear_leftovers(folder);
                return Err(Failure::Other(error.to_string()));
            }
        }
    };
    let stderr = collected(&reader, CLONE_GRACE);
    if status.success() {
        Ok(())
    } else if stderr.trim().is_empty() {
        // Ended from outside, or by a dropped connection, with nothing to say about it.
        Err(Failure::Other("Git stopped before it finished.".into()))
    } else {
        Err(classify(remote, &stderr))
    }
}

/// A Git command run in `folder` with no progress to show.
fn quietly(mut command: Command, folder: &Path, remote: &Remote, cancel: &Cancellation) -> Result<(), Failure> {
    command.current_dir(folder).stdout(Stdio::null());
    bounded(command, remote, cancel, PROBE_LIMIT).map(drop)
}

fn announce(progress: &Progress, step: u8, resumed: bool) {
    update(progress, |snapshot| {
        *snapshot = Snapshot {
            step,
            steps: STEPS,
            resumed,
            phase: "Connecting".into(),
            ..Snapshot::default()
        };
    });
}

/// Clones `remote` into `destination` without ever prompting, in steps: the latest commit, the
/// history a piece at a time, then the checkout. A folder an earlier try left behind is picked up from what it
/// received.
///
/// # Errors
/// Names why the clone failed. What a step already received is kept, and the error says a new try
/// resumes from it; a folder that never received anything is removed.
pub fn clone(
    remote: &Remote,
    destination: &Path,
    token: Option<&Token>,
    cancel: &Cancellation,
    progress: &Progress,
) -> Result<(), Failure> {
    in_steps(remote, destination, token, cancel, progress, STEPS)
}

/// [`clone`], ending after `last_step` as a lost connection would.
fn in_steps(
    remote: &Remote,
    destination: &Path,
    token: Option<&Token>,
    cancel: &Cancellation,
    progress: &Progress,
    last_step: u8,
) -> Result<(), Failure> {
    let claim = Claim::take(destination)?;
    let outcome = claimed_steps(remote, destination, token, cancel, progress, last_step);
    drop(claim);
    outcome
}

/// [`in_steps`] once the folder is held.
fn claimed_steps(
    remote: &Remote,
    destination: &Path,
    token: Option<&Token>,
    cancel: &Cancellation,
    progress: &Progress,
    last_step: u8,
) -> Result<(), Failure> {
    let earlier = marker(destination).filter(|marker| marker.url == remote.url);
    if earlier.as_ref().is_some_and(Marker::complete) {
        // A clone that was finished, and ended before it took its marker away.
        let _ = std::fs::remove_file(destination.join(".git").join(MARKER));
        return Ok(());
    }
    announce(progress, 1, earlier.is_some());
    let marker = if let Some(marker) = earlier {
        // The try before may have been ended by a crash or a kill, mid-write.
        clear_leftovers(destination);
        marker
    } else {
        let branch = default_branch(remote, token, cancel)?;
        begin(remote, destination, &branch, cancel)?
    };
    let outcome = steps(remote, destination, &marker, token, cancel, progress, last_step);
    match outcome {
        Ok(()) => {
            let _ = std::fs::remove_file(destination.join(".git").join(MARKER));
            Ok(())
        }
        Err(failure) if marker_done(destination) > 0 => Err(interrupted(failure)),
        Err(failure) => Err(removed(destination, failure)),
    }
}

/// How many fetches of a clone in `folder` are done.
fn marker_done(folder: &Path) -> usize {
    marker(folder).map_or(0, |marker| marker.done)
}

/// Makes the folder a clone starts in, claiming it: creating it is what makes it this clone's to
/// remove, and a folder that is already there is refused.
fn begin(remote: &Remote, destination: &Path, branch: &str, cancel: &Cancellation) -> Result<Marker, Failure> {
    // A default parent such as `~/Horizon` does not exist on a fresh machine.
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|error| Failure::Other(error.to_string()))?;
    }
    match std::fs::create_dir(destination) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(Failure::Other(
                "That folder already exists. Choose another clone folder and continue again.".into(),
            ));
        }
        Err(error) => return Err(Failure::Other(error.to_string())),
    }
    let ready = (|| {
        let mut init = git(None);
        init.args(["init", "--quiet"]);
        quietly(init, destination, remote, cancel)?;
        let mut add = git(None);
        add.args(["remote", "add", "origin", "--", &remote.url]);
        quietly(add, destination, remote, cancel)?;
        let marker = Marker {
            url: remote.url.clone(),
            branch: branch.to_owned(),
            done: 0,
        };
        write_marker(destination, &marker).map(|()| marker)
    })();
    ready.map_err(|failure| removed(destination, failure))
}

/// `failure`, after what the clone made in `folder` is taken away. When that cannot be done (files
/// still open on Windows, say) the folder stays and the message says where. A refusal to sign in
/// stays one, so that its prompt still appears: the folder is then offered for resuming, or
/// refused as an occupied destination, by the dialog itself.
fn removed(folder: &Path, failure: Failure) -> Failure {
    match std::fs::remove_dir_all(folder) {
        Ok(()) => failure,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => failure,
        Err(_) if matches!(failure, Failure::SignIn(_) | Failure::NotFound) => failure,
        Err(error) => {
            let said = match &failure {
                Failure::Interrupted(text) => text.clone(),
                other => other.to_string(),
            };
            let left = format!(
                "{said} What it made in {} could not be removed: {error}.",
                folder.display()
            );
            // Only a folder that still says it is an unfinished clone can be resumed.
            if unfinished(folder) {
                Failure::Interrupted(format!("{left} Continue resumes from it."))
            } else {
                Failure::Other(format!("{left} Remove it, or choose another folder."))
            }
        }
    }
}

fn steps(
    remote: &Remote,
    folder: &Path,
    marker: &Marker,
    token: Option<&Token>,
    cancel: &Cancellation,
    progress: &Progress,
    last_step: u8,
) -> Result<(), Failure> {
    let branch = &marker.branch;
    for (index, depth) in DEPTHS.iter().enumerate().skip(marker.done) {
        let step = u8::try_from(index + 1).unwrap_or(STEPS);
        if last_step < step {
            return Err(Failure::Network);
        }
        announce(progress, step, marker.done > 0);
        let mut fetch = git(token);
        fetch.args(["fetch", "--progress"]);
        if *depth > 0 {
            fetch.arg(format!("--depth={depth}")).arg("--no-tags").arg("origin");
            fetch.arg(format!("+refs/heads/{branch}:refs/remotes/origin/{branch}"));
        } else {
            if folder.join(".git").join("shallow").exists() {
                fetch.arg("--unshallow");
            }
            // Every tag, also one on a commit no branch reaches, which Git's own following of tags skips.
            fetch.args(["--tags", "origin"]);
        }
        run(fetch, folder, remote, cancel, progress)?;
        write_marker(
            folder,
            &Marker {
                url: marker.url.clone(),
                branch: marker.branch.clone(),
                done: index + 1,
            },
        )?;
    }
    if last_step < STEPS {
        return Err(Failure::Network);
    }
    announce(progress, STEPS, marker.done > 0);
    let mut head = git(None);
    head.args([
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        &format!("refs/remotes/origin/{branch}"),
    ]);
    quietly(head, folder, remote, cancel)?;
    let checkout = checkout_command(branch, token);
    run(checkout, folder, remote, cancel, progress)?;
    // Written before the marker goes: a process ended in between leaves a clone that says it is
    // finished, which nothing resumes and nothing discards.
    write_marker(
        folder,
        &Marker {
            url: marker.url.clone(),
            branch: marker.branch.clone(),
            done: DEPTHS.len() + 1,
        },
    )
}

/// The checkout of `branch`, run with the token like the fetches: a repository whose files live
/// in a filter that reads from the origin, such as Git LFS, needs it here as well.
fn checkout_command(branch: &str, token: Option<&Token>) -> Command {
    let mut checkout = git(token);
    checkout.args([
        "checkout",
        "--progress",
        "--quiet",
        "--track",
        "-B",
        branch,
        &format!("origin/{branch}"),
    ]);
    checkout
}

/// A failure of a clone that kept what it received: the same reason, with the way on.
fn interrupted(failure: Failure) -> Failure {
    const RESUME: &str = "What was received is kept, and Continue resumes from it.";
    match failure {
        Failure::SignIn(_) | Failure::NotFound | Failure::Interrupted(_) => failure,
        Failure::Cancelled => Failure::Interrupted(format!("Stopped. {RESUME}")),
        other => Failure::Interrupted(format!("{other} {RESUME}")),
    }
}

#[cfg(test)]
mod tests;
