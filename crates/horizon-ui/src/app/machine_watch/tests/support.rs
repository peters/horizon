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
        server.run(&["new-session", "-d", "-s", "warm-up", "-x", "100", "-y", "20", "sleep", "300"]);
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

    /// One session of the server, which runs `command` in its only pane.
    pub(crate) fn session(&self, name: &str, command: &str) {
        self.run(&["new-session", "-d", "-s", name, "-x", "100", "-y", "20", command]);
    }

    /// One word for the program of `session`, as a person at its keyboard types it.
    pub(crate) fn send(&self, session: &str, word: &str) {
        self.run(&["send-keys", "-t", session, word, "Enter"]);
    }

    /// The title of the pane of `session` now.
    pub(crate) fn title(&self, session: &str) -> String {
        let output = self.tmux(&["display-message", "-p", "-t", session, "#{pane_title}"]);
        String::from_utf8_lossy(&output.stdout).trim_end().to_owned()
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
    /// The stand-in that runs the text of the watch on `server`.
    pub(crate) fn new(server: &Server) -> Option<Self> {
        if !has("sh") {
            return None;
        }
        let folder = tempfile::tempdir().ok()?;
        let calls = folder.path().join("calls");
        let marker = format!("hzw-ui-ssh-{}-{}", std::process::id(), server.socket());
        let body = format!(
            "#!/bin/sh\nprintf 'c' >> {calls}\nmarker={marker}\n\
             for a in \"$@\"; do last=$a; done\n\
             HORIZON_MACHINE_WATCH_TMUX_SOCKET={socket}\nexport HORIZON_MACHINE_WATCH_TMUX_SOCKET\n\
             exec sh -c \"$last\" \"$marker\"\n",
            calls = quoted(&calls.to_string_lossy()),
            socket = quoted(server.socket()),
        );
        horizon_core::stand_in_program_in(folder.path(), "ssh", &body).ok()?;
        Some(Self {
            folder,
            calls,
            marker,
        })
    }

    /// The program of the stand-in.
    pub(crate) fn program(&self) -> String {
        self.folder.path().join("ssh").to_string_lossy().into_owned()
    }

    /// How many times the watch started this program.
    pub(crate) fn calls(&self) -> usize {
        std::fs::metadata(&self.calls).map_or(0, |held| usize::try_from(held.len()).unwrap_or(0))
    }

    /// How many `sh` children of this stand-in run now.
    pub(crate) fn children(&self) -> usize {
        let output = Command::new("ps")
            .args(["-A", "-o", "args="])
            .output()
            .expect("the process list of this computer");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.contains(&self.marker))
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

/// Runs frames until the startup read of the machine list of the home answered, so a
/// list that the test sets is not replaced by that read.
pub(crate) fn settle_machine_load(ctx: &Context, app: &mut HorizonApp) {
    let deadline = Instant::now() + DEADLINE;
    while app.machine_load_rx.is_some() {
        assert!(
            Instant::now() < deadline,
            "the machine list of the home did not load"
        );
        run_app_frame(ctx, app);
        std::thread::sleep(POLL);
    }
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

/// Waits until `wanted` holds, without a frame of the window.
pub(crate) fn wait_until(what: &str, wanted: impl Fn() -> bool) {
    let deadline = Instant::now() + DEADLINE;
    while !wanted() {
        assert!(Instant::now() < deadline, "the test waited for {what}");
        std::thread::sleep(POLL);
    }
}
