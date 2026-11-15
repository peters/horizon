//! A private `tmux` server, the shells of this computer and a run of the text of
//! the watch, for the tests of this module.

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::machine_watch::REMOTE_TEXT;

/// How long one report of the watch may take before a test reads it as missing.
pub(crate) const REPORT_DEADLINE: Duration = Duration::from_secs(30);

/// The number of the private server of the next test of this process.
static NEXT_SERVER: AtomicU32 = AtomicU32::new(1);

/// The name of one private socket. A test of another process of this machine has
/// its own name, so two runs never share a server.
fn socket_name() -> String {
    let number = NEXT_SERVER.fetch_add(1, Ordering::Relaxed);
    format!("hzw-{}-{number}", std::process::id())
}

/// Whether this computer has this program.
pub(crate) fn has(program: &str) -> bool {
    Command::new(program)
        .arg("-c")
        .arg("true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// One private `tmux` server on a socket of its own, which its drop stops.
pub(crate) struct Server {
    socket: String,
}

impl Server {
    /// The server of this test, or `None` when this computer has no `tmux`.
    pub(crate) fn new() -> Option<Self> {
        if !has("tmux") {
            return None;
        }
        let server = Self { socket: socket_name() };
        // The first call makes the server, so a socket that no server holds reads
        // as no server from here on.
        server.run(&[
            "new-session",
            "-d",
            "-s",
            "warm-up",
            "-x",
            "100",
            "-y",
            "20",
            "sleep",
            "120",
        ]);
        server.settle("warm-up");
        Some(server)
    }

    pub(crate) fn socket(&self) -> &str {
        &self.socket
    }

    /// One command of the server, with the output of its client.
    pub(crate) fn tmux(&self, args: &[&str]) -> std::process::Output {
        let mut command = Command::new("tmux");
        command.args(["-L", &self.socket, "-f", "/dev/null"]).args(args);
        command
            .stdin(Stdio::null())
            .output()
            .expect("the tmux client of the test runs")
    }

    /// One command of the server, which its test expects to work.
    pub(crate) fn run(&self, args: &[&str]) {
        let output = self.tmux(args);
        assert!(
            output.status.success(),
            "tmux {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// One session of the server, which runs `command` in its only pane.
    pub(crate) fn session(&self, name: &str, command: &str) {
        self.run(&["new-session", "-d", "-s", name, "-x", "100", "-y", "20", command]);
        self.settle(name);
    }

    /// One session of the server in the folder `folder`.
    pub(crate) fn session_in(&self, name: &str, folder: &str, command: &str) {
        self.run(&[
            "new-session",
            "-d",
            "-s",
            name,
            "-c",
            folder,
            "-x",
            "100",
            "-y",
            "20",
            command,
        ]);
        self.settle(name);
    }

    /// Waits until `tmux` reports the program and the folder of the new pane of
    /// `session`. A pane reports `tmux` and the folder of the client for the first
    /// moment of its life, because `tmux` reads both from the pane once and then
    /// keeps them for a moment.
    pub(crate) fn settle(&self, session: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let output = self.tmux(&["display-message", "-p", "-t", session, "#{pane_current_command}"]);
            let command = String::from_utf8_lossy(&output.stdout);
            let command = command.trim();
            if !command.is_empty() && command != "tmux" {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.tmux(&["kill-server"]);
    }
}

/// One shell that can run the text of the watch.
pub(crate) struct Shell {
    /// The name of the shell, as a test reports it.
    pub(crate) name: &'static str,
    /// The program and the arguments that come before the text.
    pub(crate) line: &'static [&'static str],
}

impl Shell {
    /// The shells of this computer that the text must run on.
    pub(crate) fn available() -> Vec<Self> {
        [
            Self {
                name: "dash",
                line: &["dash", "-c"],
            },
            Self {
                name: "bash",
                line: &["bash", "-c"],
            },
            Self {
                name: "busybox sh",
                line: &["busybox", "sh", "-c"],
            },
            Self {
                name: "sh",
                line: &["sh", "-c"],
            },
        ]
        .into_iter()
        .filter(|shell| {
            Command::new(shell.line[0])
                .args(&shell.line[1..])
                .arg("true")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
        .collect()
    }
}

/// What one run of the text of the watch returned.
pub(crate) struct RunOutput {
    /// Every line that the run printed, in its order.
    pub(crate) lines: Vec<String>,
    /// Everything that the run wrote to its standard error.
    pub(crate) stderr: String,
}

impl RunOutput {
    /// The lines of the report that start with `kind` and a tab, or that are the
    /// kind alone.
    pub(crate) fn of_kind<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        let with_a_field = format!("{kind}\t");
        self.lines
            .iter()
            .filter(move |line| line.starts_with(&with_a_field) || **line == *kind)
            .map(String::as_str)
    }

    /// The one line of the report with this kind, or a failure.
    pub(crate) fn single(&self, kind: &str) -> String {
        let lines: Vec<&str> = self.of_kind(kind).collect();
        assert_eq!(lines.len(), 1, "one {kind} line of the report: {:?}", self.lines);
        lines[0].to_owned()
    }

    /// How many bytes the run printed, with the newline of each line.
    pub(crate) fn bytes(&self) -> usize {
        self.lines.iter().map(|line| line.len() + 1).sum()
    }
}

/// The first report of the text of the watch on the private server of `server`.
pub(crate) fn run_text(server: &Server, shell: &Shell) -> RunOutput {
    run_text_with(server.socket(), shell, |command| command)
}

/// [`run_text`] with a caller that changes the command before it starts, such as
/// a `PATH` without `tmux`.
pub(crate) fn run_text_with(
    socket: &str,
    shell: &Shell,
    prepare: impl FnOnce(&mut Command) -> &mut Command,
) -> RunOutput {
    let mut command = Command::new(shell.line[0]);
    command
        .args(&shell.line[1..])
        .arg(REMOTE_TEXT)
        .env("HORIZON_MACHINE_WATCH_TMUX_SOCKET", socket)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = prepare(&mut command)
        .spawn()
        .unwrap_or_else(|error| panic!("the text of the watch starts under {}: {error}", shell.name));
    let stdout = child.stdout.take().expect("a pipe of the report");
    let (send, received) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let last = line == "Z";
            if send.send(line).is_err() || last {
                break;
            }
        }
    });
    let deadline = Instant::now() + REPORT_DEADLINE;
    let mut lines = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match received.recv_timeout(left) {
            Ok(line) => {
                let last = line == "Z";
                lines.push(line);
                if last {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let stderr = finish(child, reader);
    assert!(
        !lines.is_empty(),
        "{} reported nothing within {REPORT_DEADLINE:?}: {stderr}",
        shell.name
    );
    RunOutput { lines, stderr }
}

/// Every line of the text of the watch on `server` for `window`, then a kill.
///
/// One run of this length shows what a machine sends while nothing changes: the
/// report of the first interval and one short heartbeat line per interval after
/// it.
pub(crate) fn run_text_for(server: &Server, shell: &Shell, window: Duration) -> RunOutput {
    let mut command = Command::new(shell.line[0]);
    command
        .args(&shell.line[1..])
        .arg(REMOTE_TEXT)
        .env("HORIZON_MACHINE_WATCH_TMUX_SOCKET", server.socket())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("the text of the watch starts");
    let stdout = child.stdout.take().expect("a pipe of the report");
    let (send, received) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if send.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + window;
    let mut lines = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match received.recv_timeout(left) {
            Ok(line) => lines.push(line),
            Err(_) => break,
        }
    }
    let stderr = finish(child, reader);
    RunOutput { lines, stderr }
}

/// Kills the run of a test, reads what it wrote to its standard error, and joins
/// its reader.
fn finish(mut child: std::process::Child, reader: std::thread::JoinHandle<()>) -> String {
    let _ = child.kill();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let _ = child.wait();
    let _ = reader.join();
    stderr
}

/// The one pane line of the session `session` of a report.
///
/// A `tmux` server ends when its last session ends, so every server of a test holds
/// the session `warm-up` from its start, and a case reads the pane of its own
/// session rather than the count of the panes of the server.
pub(crate) fn pane_of<'a>(run: &'a RunOutput, session: &str) -> &'a str {
    let mut panes = run.of_kind("P").filter(|line| field(line, 2) == session);
    let first = panes
        .next()
        .unwrap_or_else(|| panic!("the report holds one pane of {session}: {:?}", run.lines));
    assert!(panes.next().is_none(), "one pane of {session}: {:?}", run.lines);
    first
}

/// The field `index` of one line of a report, with 0 as the kind.
pub(crate) fn field(line: &str, index: usize) -> &str {
    line.split('\t').nth(index).unwrap_or_default()
}
