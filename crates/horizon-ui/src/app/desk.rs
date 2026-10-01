//! Desktop-workspace mode (prototype, Linux only, opt in with
//! `HORIZON_DESK_MODE=1`).
//!
//! Horizon's own window chrome is dropped. Every Horizon workspace lives in its
//! own native window (a detached workspace) on its own desktop workspace, and
//! the root window becomes a command bar that stays on every desktop. A small
//! GNOME Shell extension does what Wayland leaves to the compositor: it puts
//! windows on workspaces, keeps one on all of them and switches between them.
//! This module talks to that extension over D-Bus through the `gdbus` tool,
//! on a worker thread so a slow call never stalls a frame.

use std::process::Command;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DEST: &str = "dev.horizon.Desk";
const PATH: &str = "/dev/horizon/Desk";
const POLL: Duration = Duration::from_millis(350);
pub(super) const BAR_TITLE: &str = "Horizon Command Bar";

/// Whether desktop-workspace mode is switched on for this process.
pub(super) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("HORIZON_DESK_MODE").is_some_and(|value| !value.is_empty()))
}

/// What the shell reports about desktop workspaces.
#[derive(Clone, Debug, Default)]
pub(super) struct DeskState {
    pub active: usize,
}

impl DeskState {
    fn from_json(value: &serde_json::Value) -> Self {
        Self {
            active: value["active"]
                .as_u64()
                .and_then(|index| usize::try_from(index).ok())
                .unwrap_or(0),
        }
    }
}

enum Command_ {
    Ensure(u32),
    Move(String, u32),
    Place(String, [i32; 4]),
    Stick(String),
    Above(String),
    Switch(u32),
}

pub(super) struct Desk {
    commands: Sender<Command_>,
    state: Arc<Mutex<DeskState>>,
    started: Instant,
    last_setup: Option<Instant>,
}

impl Desk {
    /// The bridge, when desktop mode is switched on.
    pub(super) fn from_env() -> Option<Self> {
        if !enabled() {
            return None;
        }
        let (commands, receiver) = channel();
        let state = Arc::new(Mutex::new(DeskState::default()));
        let shared = Arc::clone(&state);
        std::thread::Builder::new()
            .name("horizon-desk".to_string())
            .spawn(move || worker(&receiver, &shared))
            .ok()?;
        Some(Self {
            commands,
            state,
            started: Instant::now(),
            last_setup: None,
        })
    }

    /// Makes the next `place` call apply immediately, for example after a resize.
    pub(super) fn invalidate(&mut self) {
        self.last_setup = None;
    }

    pub(super) fn snapshot(&self) -> DeskState {
        self.state.lock().map(|state| state.clone()).unwrap_or_default()
    }

    pub(super) fn switch(&self, workspace: usize) {
        let _ = self
            .commands
            .send(Command_::Switch(u32::try_from(workspace).unwrap_or(0)));
    }

    /// Re-applies where every window belongs. Idempotent, so it is repeated for
    /// the first seconds while windows are still being created.
    pub(super) fn place(&mut self, workspaces: &[String], bar: [i32; 4], monitor: [i32; 2]) {
        let now = Instant::now();
        let early = self.started.elapsed() < Duration::from_secs(40);
        let due = self.last_setup.is_none_or(|last| {
            now.duration_since(last)
                >= if early {
                    Duration::from_secs(2)
                } else {
                    Duration::from_secs(30)
                }
        });
        if !due {
            return;
        }
        self.last_setup = Some(now);
        let count = u32::try_from(workspaces.len().max(1)).unwrap_or(1);
        let _ = self.commands.send(Command_::Ensure(count));
        for (index, name) in workspaces.iter().enumerate() {
            let title = format!("{name} · Horizon");
            let _ = self
                .commands
                .send(Command_::Move(title.clone(), u32::try_from(index).unwrap_or(0)));
            // Below the shell's top bar, filling the rest of the monitor.
            let _ = self
                .commands
                .send(Command_::Place(title, [0, 32, monitor[0], monitor[1] - 32]));
        }
        let _ = self.commands.send(Command_::Stick(BAR_TITLE.to_string()));
        let _ = self.commands.send(Command_::Above(BAR_TITLE.to_string()));
        let _ = self.commands.send(Command_::Place(BAR_TITLE.to_string(), bar));
    }
}

fn worker(commands: &Receiver<Command_>, state: &Arc<Mutex<DeskState>>) {
    loop {
        match commands.recv_timeout(POLL) {
            Ok(command) => run(&command),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
        while let Ok(command) = commands.try_recv() {
            run(&command);
        }
        if let Some(fresh) = read_state()
            && let Ok(mut current) = state.lock()
        {
            *current = fresh;
        }
    }
}

fn call(method: &str, arguments: &[String]) -> Option<String> {
    let output = Command::new("gdbus")
        .args(["call", "--session", "--dest", DEST, "--object-path", PATH, "--method"])
        .arg(format!("{DEST}.{method}"))
        .args(arguments)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn read_state() -> Option<DeskState> {
    let text = call("State", &[])?;
    // The reply is a one-element tuple holding a quoted JSON string.
    let start = text.find('\'')? + 1;
    let end = text.rfind('\'')?;
    let value: serde_json::Value = serde_json::from_str(text.get(start..end)?).ok()?;
    Some(DeskState::from_json(&value))
}

fn run(command: &Command_) {
    let _ = match command {
        Command_::Ensure(count) => call("EnsureWorkspaces", &[count.to_string()]),
        Command_::Move(title, workspace) => call("MoveWindow", &[title.clone(), workspace.to_string()]),
        Command_::Place(title, [x, y, w, h]) => call(
            "Place",
            &[
                title.clone(),
                x.to_string(),
                y.to_string(),
                w.to_string(),
                h.to_string(),
            ],
        ),
        Command_::Stick(title) => call("StickWindow", &[title.clone(), "true".to_string()]),
        Command_::Above(title) => call("KeepAbove", &[title.clone(), "true".to_string()]),
        Command_::Switch(workspace) => call("Switch", &[workspace.to_string()]),
    };
}
