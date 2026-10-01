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

/// A window as the shell reports it.
#[derive(Clone, Debug, Default)]
pub(super) struct DeskWindow {
    pub app_id: String,
    /// The desktop workspace it is on, or -1 when it is on all of them.
    pub workspace: i64,
    /// Left, top, width and height in monitor pixels.
    pub rect: [i32; 4],
}

/// How far a panel window has got on its way to its desktop.
#[derive(Clone, Copy, Debug)]
pub(super) struct Placement {
    pub since: Instant,
    /// The shell has reported the window on the desktop it was sent to; from then on a
    /// different desktop means the person moved it.
    pub confirmed: bool,
}

/// What the shell reports about desktop workspaces.
#[derive(Clone, Debug, Default)]
pub(super) struct DeskState {
    pub active: usize,
    pub windows: Vec<DeskWindow>,
    /// Left, top, width and height of the area windows may use: the monitor without the
    /// top bar and the dock. All zero until the shell has reported it.
    pub workarea: [i32; 4],
}

impl DeskState {
    fn from_json(value: &serde_json::Value) -> Self {
        let windows = value["windows"]
            .as_array()
            .map(|list| {
                list.iter()
                    .map(|window| {
                        let rect = window["rect"].as_array().map_or([0; 4], |numbers| {
                            let mut out = [0; 4];
                            for (slot, number) in out.iter_mut().zip(numbers) {
                                *slot = number.as_i64().and_then(|n| i32::try_from(n).ok()).unwrap_or(0);
                            }
                            out
                        });
                        DeskWindow {
                            app_id: window["app_id"].as_str().unwrap_or_default().to_string(),
                            workspace: window["workspace"].as_i64().unwrap_or(-1),
                            rect,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut workarea = [0; 4];
        if let Some(numbers) = value["workarea"].as_array() {
            for (slot, number) in workarea.iter_mut().zip(numbers) {
                *slot = number.as_i64().and_then(|n| i32::try_from(n).ok()).unwrap_or(0);
            }
        }
        Self {
            active: value["active"]
                .as_u64()
                .and_then(|index| usize::try_from(index).ok())
                .unwrap_or(0),
            windows,
            workarea,
        }
    }
}

/// The window class a panel's native window is given, so the shell can find it.
pub(super) const PANEL_APP_PREFIX: &str = "horizon-panel-";

pub(super) fn panel_app_id(local_id: &str) -> String {
    let clean: String = local_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("{PANEL_APP_PREFIX}{clean}")
}

enum Command_ {
    Ensure(u32),
    Place(String, [i32; 4]),
    Stick(String),
    Above(String),
    Switch(u32),
    Key(bool),
    MoveClass(String, u32),
    PlaceClass(String, [i32; 4]),
    Overview,
}

pub(super) struct Desk {
    commands: Sender<Command_>,
    state: Arc<Mutex<DeskState>>,
    started: Instant,
    last_setup: Option<Instant>,
    /// Until then the bar is placed again every moment: a window that is being resized can undo a first placement.
    settle_until: Option<Instant>,
    /// When each panel window was first put on its desktop; after a moment the shell owns it.
    pub placed: std::collections::HashMap<horizon_core::PanelId, Placement>,
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
            settle_until: None,
            placed: std::collections::HashMap::new(),
        })
    }

    /// Puts another window (found by its title) at this rectangle and above the rest.
    pub(super) fn place_titled(&self, title: &str, rect: [i32; 4]) {
        let _ = self.commands.send(Command_::Place(title.to_string(), rect));
        let _ = self.commands.send(Command_::Above(title.to_string()));
    }

    /// Makes the next `place` call apply immediately, for example after a resize.
    pub(super) fn invalidate(&mut self) {
        self.last_setup = None;
        self.settle_until = Some(Instant::now() + Duration::from_secs(5));
    }

    pub(super) fn snapshot(&self) -> DeskState {
        self.state.lock().map(|state| state.clone()).unwrap_or_default()
    }

    pub(super) fn switch(&self, workspace: usize) {
        let _ = self
            .commands
            .send(Command_::Switch(u32::try_from(workspace).unwrap_or(0)));
    }

    /// Presses GNOME's own workspace shortcut: Ctrl+Alt+Right when `right`, else Ctrl+Alt+Left.
    pub(super) fn press_key(&self, right: bool) {
        let _ = self.commands.send(Command_::Key(right));
    }

    /// Puts a panel window on a desktop workspace.
    pub(super) fn move_window(&self, app_id: &str, workspace: usize) {
        let _ = self.commands.send(Command_::MoveClass(
            app_id.to_string(),
            u32::try_from(workspace).unwrap_or(0),
        ));
    }

    /// Positions and sizes a panel window (left, top, width, height).
    pub(super) fn place_window(&self, app_id: &str, rect: [i32; 4]) {
        let _ = self.commands.send(Command_::PlaceClass(app_id.to_string(), rect));
    }

    /// Shows or hides GNOME's own overview, with a thumbnail of every workspace.
    pub(super) fn toggle_overview(&self) {
        let _ = self.commands.send(Command_::Overview);
    }

    /// Keeps the bar on every desktop, above the other windows, and as many desktops as workspaces.
    /// Idempotent, so it is repeated for the first seconds while the window is created.
    pub(super) fn place_bar(&mut self, workspaces: usize, bar: [i32; 4]) {
        let now = Instant::now();
        let early = self.started.elapsed() < Duration::from_secs(40);
        let settling = self.settle_until.is_some_and(|until| now < until);
        let due = self.last_setup.is_none_or(|last| {
            now.duration_since(last)
                >= if settling {
                    Duration::from_millis(500)
                } else if early {
                    Duration::from_secs(2)
                } else {
                    Duration::from_secs(30)
                }
        });
        if !due {
            return;
        }
        self.last_setup = Some(now);
        let count = u32::try_from(workspaces.max(1)).unwrap_or(1);
        let _ = self.commands.send(Command_::Ensure(count));
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
        Command_::MoveClass(app_id, workspace) => call("MoveClass", &[app_id.clone(), workspace.to_string()]),
        Command_::PlaceClass(app_id, [x, y, w, h]) => call(
            "PlaceClass",
            &[
                app_id.clone(),
                x.to_string(),
                y.to_string(),
                w.to_string(),
                h.to_string(),
            ],
        ),
        Command_::Overview => call("ToggleOverview", &[]),
        Command_::Key(right) => call(
            "PressWorkspaceKey",
            &[(if *right { "right" } else { "left" }).to_string()],
        ),
    };
}
