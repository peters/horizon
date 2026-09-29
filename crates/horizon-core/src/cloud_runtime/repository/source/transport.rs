//! Talking to a repository's host with system Git: looking at it without cloning, and fetching it
//! in steps that a failure or a cancel does not throw away, so a later try picks up from what was
//! already received. Git cannot resume inside one pack, so each step is one pack: the latest
//! commit first, then the rest of the history, then the checkout.
use super::{Cancellation, Failure, Remote, Token, candidates, classify, git_output};
use std::{
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

/// How long a finished clone waits for what Git said last; a helper it left behind may hold the pipe.
const CLONE_GRACE: Duration = Duration::from_secs(2);

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
    if std::env::var_os("GIT_SSH_COMMAND").is_none()
        && git_output(Path::new("."), &["config", "--get", "core.sshCommand"]).is_none_or(|set| set.trim().is_empty())
    {
        command.env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o StrictHostKeyChecking=yes -o ConnectTimeout=15",
        );
    }
    command
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
    let out = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.read_to_string(&mut text);
        }
        text
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
    let stderr = collected(&reader, limit.saturating_sub(started.elapsed()));
    if status.success() {
        Ok(out.join().unwrap_or_default())
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
fn write_marker(folder: &Path, marker: &Marker) -> Result<(), Failure> {
    let git = folder.join(".git");
    let staged = git.join(format!("{MARKER}.tmp"));
    std::fs::write(&staged, format!("{}\n{}\n{}\n", marker.url, marker.branch, marker.done))
        .and_then(|()| std::fs::rename(&staged, git.join(MARKER)))
        .map_err(|error| Failure::Other(error.to_string()))
}

/// Whether `folder` is a clone that was started and not finished.
pub(super) fn unfinished(folder: &Path) -> bool {
    marker(folder).is_some()
}

/// A folder under `parent` where an earlier try at cloning `remote` stopped, and that a new try
/// picks up from what it received.
#[must_use]
pub fn resumable(parent: &Path, remote: &Remote) -> Option<PathBuf> {
    candidates(parent, remote)
        .filter(|path| path.is_dir())
        .find(|path| marker(path).is_some_and(|marker| marker.url == remote.url))
}

/// Gives up on a clone that stopped, removing what it received. Only a folder a clone started
/// and did not finish is touched.
pub fn discard(folder: &Path) {
    if marker(folder).is_some() {
        let _ = std::fs::remove_dir_all(folder);
    }
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
    let earlier = marker(destination).filter(|marker| marker.url == remote.url);
    let resumed = earlier.is_some();
    if resumed {
        // The try before may have been ended by a crash or a kill, mid-write.
        clear_leftovers(destination);
    }
    announce(progress, 1, resumed);
    let marker = if let Some(marker) = earlier {
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
        Err(failure) => {
            let _ = std::fs::remove_dir_all(destination);
            Err(failure)
        }
    }
}

/// How many fetches of a clone in `folder` are done.
fn marker_done(folder: &Path) -> usize {
    marker(folder).map_or(0, |marker| marker.done)
}

/// Makes the folder a clone starts in, claiming it: creating it is what makes it this clone's to
/// remove, and a folder that is already there is refused.
fn begin(remote: &Remote, destination: &Path, branch: &str, cancel: &Cancellation) -> Result<Marker, Failure> {
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
    if ready.is_err() {
        let _ = std::fs::remove_dir_all(destination);
    }
    ready
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
    run(checkout, folder, remote, cancel, progress)
}

/// The checkout of `branch`, run with the token like the fetches: a repository whose files live
/// in a filter that reads from the origin, such as Git LFS, needs it here as well.
fn checkout_command(branch: &str, token: Option<&Token>) -> Command {
    let mut checkout = git(token);
    checkout.args([
        "checkout",
        "--progress",
        "--quiet",
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
mod tests {
    use super::*;

    #[test]
    fn git_progress_is_read_for_its_phase_how_far_and_the_speed() {
        assert_eq!(
            parse_progress("Receiving objects:  45% (450/1000), 12.30 MiB | 4.50 MiB/s"),
            Some(("Receiving objects".into(), 45, "12.30 MiB | 4.50 MiB/s".into()))
        );
        assert_eq!(
            parse_progress("remote: Counting objects: 100% (5/5), done."),
            Some(("Counting objects".into(), 100, String::new()))
        );
        assert_eq!(
            parse_progress("Resolving deltas: 100% (10/10), done."),
            Some(("Resolving deltas".into(), 100, String::new()))
        );
        assert_eq!(
            parse_progress("Resolving deltas: 100% (1234/1234), completed with 2710 local objects."),
            Some(("Resolving deltas".into(), 100, String::new()))
        );
        assert!(is_chatter("remote: Enumerating objects: 34191, done."));
        assert!(is_chatter(
            "remote: Total 34191 (delta 0), reused 0 (delta 0), pack-reused 34191"
        ));
        assert!(!is_chatter("remote: Repository not found."));
        assert!(!is_chatter("fatal: early EOF"));
        assert_eq!(parse_progress("fatal: repository not found"), None);
        assert_eq!(parse_progress("Cloning into 'x'..."), None);
    }

    #[test]
    fn the_time_left_follows_the_pace_and_waits_for_one() {
        assert_eq!(eta(Duration::from_secs(10), 50), Some(Duration::from_secs(10)));
        assert_eq!(eta(Duration::from_secs(9), 90), Some(Duration::from_secs(1)));
        assert_eq!(eta(Duration::from_millis(500), 50), None, "too early to know");
        assert_eq!(eta(Duration::from_secs(10), 1), None);
        assert_eq!(eta(Duration::from_secs(10), 100), None, "nothing left");
    }

    #[test]
    fn a_host_that_never_stops_talking_leaves_only_a_bounded_tail() {
        let mut tail = String::new();
        for _ in 0..10_000 {
            keep_tail(&mut tail, "fatal: a very talkative remote, éééé\n");
        }
        keep_tail(&mut tail, "fatal: could not read Username");
        assert!(tail.len() <= STDERR_TAIL);
        assert!(
            tail.ends_with("could not read Username"),
            "the end is what names the failure"
        );
    }

    #[test]
    fn a_pipe_that_never_closes_cannot_hold_the_deadline() {
        let (_sender, reader) = mpsc::channel::<String>();
        let started = Instant::now();
        assert_eq!(collected(&reader, Duration::from_millis(100)), "");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// A repository with a few commits and a second branch, reachable by a `file://` address so
    /// that Git honours a depth.
    fn origin(temp: &Path) -> Remote {
        origin_with(temp, 3)
    }

    fn origin_with(temp: &Path, commits: usize) -> Remote {
        let origin = temp.join("origin");
        std::fs::create_dir(&origin).unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(&origin)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "{args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        for number in 1..=commits {
            git(&["commit", "-q", "--allow-empty", "-m", &format!("commit {number}")]);
        }
        git(&["branch", "extra"]);
        // A tag on a commit that no branch reaches, which Git does not follow on its own.
        git(&["checkout", "-q", "--detach"]);
        git(&["commit", "-q", "--allow-empty", "-m", "off every branch"]);
        git(&["tag", "lonely"]);
        git(&["checkout", "-q", "main"]);
        Remote {
            url: {
                let path = origin.to_string_lossy().replace('\\', "/");
                if path.starts_with('/') {
                    format!("file://{path}")
                } else {
                    format!("file:///{path}")
                }
            },
            host: "example.com".into(),
            name: "origin".into(),
        }
    }

    fn count(folder: &Path, args: &[&str]) -> String {
        git_output(folder, args).unwrap().trim().to_owned()
    }

    #[test]
    fn a_clone_that_stops_after_a_step_is_picked_up_where_it_stopped() {
        let temp = tempfile::tempdir().unwrap();
        let remote = origin(temp.path());
        let target = temp.path().join("clones").join("origin");
        let (cancel, progress) = (Cancellation::default(), Progress::default());
        let stopped = in_steps(&remote, &target, None, &cancel, &progress, 1);
        assert!(
            matches!(&stopped, Err(Failure::Interrupted(text)) if text.contains("Continue resumes")),
            "{stopped:?}"
        );
        assert!(
            target.join(".git").join(MARKER).is_file(),
            "the folder says it is unfinished"
        );
        assert_eq!(resumable(target.parent().unwrap(), &remote), Some(target.clone()));
        assert!(!super::super::is_checkout(&target));
        assert_eq!(
            count(&target, &["rev-list", "--count", "refs/remotes/origin/main"]),
            "1",
            "the latest commit only"
        );

        clone(&remote, &target, None, &cancel, &progress).unwrap();
        assert!(super::super::is_checkout(&target));
        assert!(!target.join(".git").join(MARKER).exists(), "finished");
        assert!(!target.join(".git").join("shallow").exists(), "the whole history");
        assert_eq!(count(&target, &["rev-list", "--count", "HEAD"]), "3");
        assert_eq!(count(&target, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
        assert_eq!(
            count(&target, &["rev-parse", "--abbrev-ref", "main@{upstream}"]),
            "origin/main"
        );
        assert_eq!(
            count(&target, &["rev-parse", "--verify", "refs/remotes/origin/extra"]).len(),
            40,
            "every branch"
        );
        assert!(progress.lock().unwrap().resumed || progress.lock().unwrap().step == STEPS);
        assert_eq!(resumable(target.parent().unwrap(), &remote), None);
    }

    #[cfg(unix)]
    #[test]
    fn progress_is_shown_but_never_becomes_the_reason_a_step_failed() {
        let temp = tempfile::tempdir().unwrap();
        let remote = origin(temp.path());
        let step = |script: &str| {
            let mut command = Command::new("sh");
            command.args(["-c", script]).stderr(Stdio::piped());
            let progress = Progress::default();
            let result = run(command, temp.path(), &remote, &Cancellation::default(), &progress);
            (result, progress.lock().unwrap().clone())
        };
        let (result, shown) =
            step("printf 'Receiving objects:  50%% (1/2), 1.00 MiB | 1.00 MiB/s\\r' >&2; kill -KILL $$");
        assert_eq!(result, Err(Failure::Other("Git stopped before it finished.".into())));
        assert_eq!((shown.phase.as_str(), shown.percent), ("Receiving objects", Some(50)));
        let (result, _) = step("printf 'remote: Enumerating objects: 5, done.\\n' >&2; kill -KILL $$");
        assert_eq!(
            result,
            Err(Failure::Other("Git stopped before it finished.".into())),
            "chatter is not a reason"
        );
        let (result, _) = step("printf 'Receiving objects:  50%% (1/2)\\rfatal: early EOF\\n' >&2; exit 128");
        assert_eq!(result, Err(Failure::Other("fatal: early EOF".into())));
        let (result, _) = step("printf 'fatal: no newline at the end' >&2; exit 128");
        assert_eq!(
            result,
            Err(Failure::Other("fatal: no newline at the end".into())),
            "the last word counts without a line end"
        );
    }

    #[test]
    fn the_history_comes_in_pieces_and_each_finished_piece_is_kept() {
        assert_eq!(usize::from(STEPS), DEPTHS.len() + 1);
        let temp = tempfile::tempdir().unwrap();
        let remote = origin_with(temp.path(), 130);
        let target = temp.path().join("deep");
        let (cancel, progress) = (Cancellation::default(), Progress::default());
        let commits = |folder: &Path| count(folder, &["rev-list", "--count", "refs/remotes/origin/main"]);
        assert!(in_steps(&remote, &target, None, &cancel, &progress, 1).is_err());
        assert_eq!((commits(&target), marker_done(&target)), ("1".to_owned(), 1));
        assert!(in_steps(&remote, &target, None, &cancel, &progress, 2).is_err());
        assert_eq!((commits(&target), marker_done(&target)), ("100".to_owned(), 2));
        // A drop during the third piece keeps the two before it, and the try after starts at the third.
        assert!(in_steps(&remote, &target, None, &cancel, &progress, 3).is_err());
        assert_eq!((commits(&target), marker_done(&target)), ("130".to_owned(), 3));
        clone(&remote, &target, None, &cancel, &progress).unwrap();
        assert!(super::super::is_checkout(&target) && !target.join(".git").join(MARKER).exists());
        assert!(!target.join(".git").join("shallow").exists());
        assert_eq!(count(&target, &["rev-list", "--count", "HEAD"]), "130");
        let shown = progress.lock().unwrap().clone();
        assert_eq!((shown.step, shown.steps, shown.resumed), (STEPS, STEPS, true));
    }

    #[test]
    fn the_checkout_carries_the_token_like_the_fetches_do() {
        let remote = super::super::parse("github.com/demo-org/demo").unwrap();
        let token = Token::new(&remote, "demo_token").unwrap();
        let envs = |command: &Command| {
            command
                .get_envs()
                .filter_map(|(key, value)| Some((key.to_str()?.to_owned(), value?.to_str()?.to_owned())))
                .collect::<std::collections::HashMap<_, _>>()
        };
        let with = envs(&checkout_command("main", Some(&token)));
        assert_eq!(with.get("GIT_CONFIG_COUNT").map(String::as_str), Some("1"));
        assert!(with["GIT_CONFIG_KEY_0"].starts_with("http.https://github.com"));
        assert!(!envs(&checkout_command("main", None)).contains_key("GIT_CONFIG_COUNT"));
        let args: Vec<_> = checkout_command("main", Some(&token))
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(args.iter().all(|arg| !arg.contains("demo_token")), "never argv");
    }

    #[test]
    fn the_marker_is_replaced_whole_and_a_leftover_staging_file_changes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        let first = Marker {
            url: "https://github.com/demo-org/demo.git".into(),
            branch: "main".into(),
            done: 1,
        };
        write_marker(temp.path(), &first).unwrap();
        // A write cut off half way only ever touches the staging file.
        std::fs::write(
            temp.path().join(".git").join("horizon-clone.tmp"),
            "https://github.com/demo-or",
        )
        .unwrap();
        assert_eq!(marker_done(temp.path()), 1);
        clear_leftovers(temp.path());
        assert!(!temp.path().join(".git").join("horizon-clone.tmp").exists());
        write_marker(temp.path(), &Marker { done: 3, ..first }).unwrap();
        assert_eq!(marker_done(temp.path()), 3);
        assert!(!temp.path().join(".git").join("horizon-clone.tmp").exists());
    }

    #[test]
    fn a_resume_clears_the_locks_a_killed_git_left_behind() {
        let temp = tempfile::tempdir().unwrap();
        let remote = origin(temp.path());
        let target = temp.path().join("origin-clone");
        let (cancel, progress) = (Cancellation::default(), Progress::default());
        assert!(in_steps(&remote, &target, None, &cancel, &progress, 1).is_err());
        let git = target.join(".git");
        for stale in ["shallow.lock", "index.lock", "HEAD.lock"] {
            std::fs::write(git.join(stale), "").unwrap();
        }
        std::fs::write(git.join("refs/remotes/origin/main.lock"), "").unwrap();
        std::fs::write(git.join("objects/pack/tmp_pack_abc123"), "partial").unwrap();
        clone(&remote, &target, None, &cancel, &progress).unwrap();
        assert!(super::super::is_checkout(&target));
        assert!(!git.join("shallow.lock").exists() && !git.join("objects/pack/tmp_pack_abc123").exists());
    }

    #[cfg(unix)]
    #[test]
    fn clearing_leftovers_never_follows_a_link_out_of_the_checkout() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("theirs.lock"), "not ours").unwrap();
        let folder = temp.path().join("checkout");
        std::fs::create_dir_all(folder.join(".git").join("refs").join("heads")).unwrap();
        std::fs::write(folder.join(".git").join("refs").join("heads").join("main.lock"), "").unwrap();
        std::os::unix::fs::symlink(&outside, folder.join(".git").join("refs").join("link")).unwrap();
        clear_leftovers(&folder);
        assert!(
            outside.join("theirs.lock").is_file(),
            "a link leads outside, and is left alone"
        );
        assert!(
            !folder
                .join(".git")
                .join("refs")
                .join("heads")
                .join("main.lock")
                .exists()
        );
    }

    #[test]
    fn a_clone_that_never_received_anything_leaves_no_folder_and_a_discard_removes_a_stopped_one() {
        let temp = tempfile::tempdir().unwrap();
        let remote = origin(temp.path());
        let cancelled = Cancellation::default();
        cancelled.cancel();
        let target = temp.path().join("early");
        assert_eq!(
            clone(&remote, &target, None, &cancelled, &Progress::default()),
            Err(Failure::Cancelled)
        );
        assert!(!target.exists());
        let stopped = temp.path().join("stopped");
        let _ = in_steps(
            &remote,
            &stopped,
            None,
            &Cancellation::default(),
            &Progress::default(),
            1,
        );
        assert!(stopped.is_dir());
        discard(&stopped);
        assert!(!stopped.exists());
        let theirs = temp.path().join("theirs");
        std::fs::create_dir(&theirs).unwrap();
        discard(&theirs);
        assert!(
            theirs.is_dir(),
            "a folder that is not an unfinished clone is left alone"
        );
    }
}
