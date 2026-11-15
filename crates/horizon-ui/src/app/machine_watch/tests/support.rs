//! A private `tmux` server, a stand-in `ssh` and the stand-in agents of
//! `scripts/device-smoke`, for the tests of the watch of the machines.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use egui::Context;

use crate::app::HorizonApp;
use crate::app::test_support::run_app_frame;

/// How long a test waits for the watch to reach the state that it expects.
pub(crate) const DEADLINE: Duration = Duration::from_secs(30);

/// How often a test reads the state while it waits.
const POLL: Duration = Duration::from_millis(25);

/// The number of the private server of the next test of this process.
static NEXT_SERVER: AtomicU32 = AtomicU32::new(1);

/// Whether this computer has this program.
pub(crate) fn has(program: &str) -> bool {
    Command::new(program)
        .arg("-V")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// One private `tmux` server on a socket of its own, which its drop stops.
pub(crate) struct Server {
    socket: String,
}

impl Server {
    /// The server of this test, or `None` when this computer has no `tmux`.
    pub(crate) fn new() -> Option<Self> {
        if !has("tmux") {
            eprintln!("the tests of the watch skip their tmux lane: this computer has no tmux");
            return None;
        }
        let number = NEXT_SERVER.fetch_add(1, Ordering::Relaxed);
        let server = Self {
            socket: format!("hzw-ui-{}-{number}", std::process::id()),
        };
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
            "300",
        ]);
        Some(server)
    }

    pub(crate) fn socket(&self) -> &str {
        &self.socket
    }

    /// One command of the server, with the output of its client.
    pub(crate) fn tmux(&self, args: &[&str]) -> std::process::Output {
        Command::new("tmux")
            .args(["-L", &self.socket, "-f", "/dev/null"])
            .args(args)
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

    /// One session of the server in the folder `folder`, which runs `command` in its
    /// only pane. The name of the agent of the pane is the last part of that folder.
    pub(crate) fn session_in(&self, name: &str, folder: &Path, command: &str) {
        let folder = folder.canonicalize().unwrap_or_else(|_| folder.to_path_buf());
        self.run(&[
            "new-session",
            "-d",
            "-s",
            name,
            "-c",
            folder.to_str().expect("a folder of this computer"),
            "-x",
            "100",
            "-y",
            "20",
            command,
        ]);
    }

    /// One word for the program of `session`, as a person at its keyboard types it.
    pub(crate) fn send(&self, session: &str, word: &str) {
        self.run(&["send-keys", "-t", session, word, "Enter"]);
    }

    /// The state that the program of the pane of `session` reports in the pane option
    /// `@horizon_state`, as the hook of an agent sets it, or the removal of it.
    pub(crate) fn report(&self, session: &str, word: Option<&str>) {
        let output = self.tmux(&["display-message", "-p", "-t", session, "#{pane_id}"]);
        let pane = String::from_utf8_lossy(&output.stdout).trim_end().to_owned();
        match word {
            Some(word) => self.run(&["set-option", "-p", "-t", &pane, "@horizon_state", word]),
            None => self.run(&["set-option", "-p", "-u", "-t", &pane, "@horizon_state"]),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.tmux(&["kill-server"]);
    }
}

/// One stand-in `ssh` of a test: it runs the text of the watch on the private server
/// of the test, counts its calls and marks its children.
pub(crate) struct Ssh {
    folder: tempfile::TempDir,
    calls: PathBuf,
    marker: String,
}

impl Ssh {
    /// The stand-in that runs the text of the watch on `server`: the one word of the
    /// remote command reads that text on the input of the stand-in, and `-s` keeps the
    /// marker of this test in the process list.
    pub(crate) fn new(server: &Server) -> Option<Self> {
        Self::of(server, "exec $last -s \"$marker\"")
    }

    /// The stand-in whose link ends `seconds` after each start, as the link of a
    /// machine that goes away does, and whose next attempt comes back: a killer of the
    /// stand-in's own process ends the shell that runs the text.
    pub(crate) fn unsteady(server: &Server, seconds: u32) -> Option<Self> {
        Self::of(
            server,
            &format!("{{ sleep {seconds}; kill $$; }} &\nexec $last -s \"$marker\""),
        )
    }

    /// The stand-in that runs `body` after it counted its call.
    fn of(server: &Server, body: &str) -> Option<Self> {
        if !has("sh") {
            return None;
        }
        let folder = tempfile::tempdir().ok()?;
        let calls = folder.path().join("calls");
        // The marker ends with its own word, so the marker of another test of this
        // process, whose socket number starts with the same digits, is another word.
        let marker = format!("hzw-ui-ssh-{}-{}-end", std::process::id(), server.socket());
        let body = format!(
            "#!/bin/sh\nprintf 'c' >> {calls}\nmarker={marker}\n\
             for a in \"$@\"; do last=$a; done\n\
             HORIZON_MACHINE_WATCH_TMUX_SOCKET={socket}\nexport HORIZON_MACHINE_WATCH_TMUX_SOCKET\n\
             {body}\n",
            calls = quoted(&calls.to_string_lossy()),
            socket = quoted(server.socket()),
        );
        horizon_core::stand_in_program_in(folder.path(), "ssh", &body).ok()?;
        Some(Self { folder, calls, marker })
    }

    /// The program of the stand-in.
    pub(crate) fn program(&self) -> String {
        self.folder.path().join("ssh").to_string_lossy().into_owned()
    }

    /// How many times the watch started this program.
    pub(crate) fn calls(&self) -> usize {
        std::fs::metadata(&self.calls).map_or(0, |held| usize::try_from(held.len()).unwrap_or(0))
    }

    /// How many `sh` children of this stand-in run now: the shells that run the text,
    /// and not the subshells that a command substitution or a pipeline of that text
    /// forks for a moment, which carry the same command line.
    pub(crate) fn children(&self) -> usize {
        let output = Command::new("ps")
            .args(["-A", "-o", "pid=,ppid=,args="])
            .output()
            .expect("the process list of this computer");
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        // The pid column is right-aligned to the width of the widest pid of this
        // computer, so a line of a shorter pid starts with spaces and the words of a
        // line are read without them.
        let rows: Vec<(u32, u32, bool)> = text
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                let pid = words.next()?.parse().ok()?;
                let parent = words.next()?.parse().ok()?;
                Some((pid, parent, line.contains(&self.marker)))
            })
            .collect();
        let marked: std::collections::BTreeSet<u32> = rows
            .iter()
            .filter(|(_, _, marked)| *marked)
            .map(|(pid, _, _)| *pid)
            .collect();
        rows.iter()
            .filter(|(pid, parent, _)| marked.contains(pid) && !marked.contains(parent))
            .count()
    }
}

/// One value as a shell reads it: one word, with no expansion.
fn quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// A copy of the Codex stand-in of `scripts/device-smoke` under the name `codex`, so
/// the watch finds the agent kind in the command line of the interpreter of its pane.
pub(crate) fn codex_stand_in(folder: &Path) -> Option<PathBuf> {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/device-smoke/codex_attention_standin.py")
        .canonicalize()
        .ok()?;
    let body = std::fs::read_to_string(script).ok()?;
    let program = folder.join("codex");
    let mut file = std::fs::File::create(&program).ok()?;
    file.write_all(body.as_bytes()).ok()?;
    drop(file);
    Some(program)
}

/// The command line of the Codex stand-in in a pane of a test.
pub(crate) fn codex_command(folder: &Path) -> Option<String> {
    let program = codex_stand_in(folder)?;
    let python = ["python3", "python"].into_iter().find(|program| has(program))?;
    Some(format!("{python} {}", program.display()))
}

/// One machine record of a test: a route that no person reaches.
pub(crate) fn record(name: &str) -> horizon_core::Machine {
    horizon_core::Machine {
        name: name.to_owned(),
        host: "127.0.0.1".to_owned(),
        port: Some(2222),
        added_at_millis: 0,
        ..horizon_core::Machine::default()
    }
}

/// The machine list of `app` with the records of `records`.
pub(crate) fn with_machines(app: &mut HorizonApp, records: Vec<horizon_core::Machine>) {
    app.machine_list = horizon_core::Machines::with_records(
        horizon_core::MachineRegistry::from_home(app.session_store.home()),
        records,
    );
}

/// Runs frames of the window until `wanted` holds, and names what it waited for.
pub(crate) fn wait_for(ctx: &Context, app: &mut HorizonApp, what: &str, wanted: impl Fn(&mut HorizonApp) -> bool) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        run_app_frame(ctx, app);
        if wanted(app) {
            return;
        }
        assert!(Instant::now() < deadline, "the test waited for {what}");
        std::thread::sleep(POLL);
    }
}

/// Runs frames of a window that is not in front until `wanted` holds, so the notice of a
/// change of an agent of a machine is queued.
pub(crate) fn wait_for_unfocused(
    ctx: &Context,
    app: &mut HorizonApp,
    what: &str,
    wanted: impl Fn(&mut HorizonApp) -> bool,
) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let mut input = crate::app::test_support::raw_input([1400.0, 900.0], None);
        input.focused = false;
        crate::app::test_support::run_app_frame_with_input(ctx, app, input);
        if wanted(app) {
            return;
        }
        assert!(Instant::now() < deadline, "the test waited for {what}");
        std::thread::sleep(POLL);
    }
}

/// Waits until `wanted` holds, without a frame of the window.
pub(crate) fn wait_until(what: &str, wanted: impl Fn() -> bool) {
    let deadline = Instant::now() + DEADLINE;
    while !wanted() {
        assert!(Instant::now() < deadline, "the test waited for {what}");
        std::thread::sleep(POLL);
    }
}
