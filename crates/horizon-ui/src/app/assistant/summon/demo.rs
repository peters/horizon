//! Scripted input for demos (desk mode only, opt in with `HORIZON_DESK_SCRIPT`).
//!
//! The file named by the variable is read as it grows. Each line is one command:
//! `say <ms> <text>` reveals the text word by word over that many milliseconds
//! while the mic shows recording, as if it were being dictated; `enter` submits
//! the prompt; `expand a|b|c` and `collapse` change the bar; `go <n>` switches
//! desktop workspace; `scope all|<workspace name>` narrows the assistant.

use std::time::{Duration, Instant};

use super::HorizonApp;
use super::command_bar;

pub(in crate::app::assistant) struct Demo {
    path: std::path::PathBuf,
    consumed: usize,
    saying: Option<Saying>,
    /// Voice level (0 to 1) every 50 ms of the audio being "dictated".
    envelope: Vec<f32>,
    /// The tile a click just landed on, for the ripple and cursor.
    click: Option<(usize, Instant)>,
    /// A workspace shortcut just pressed (true for right), for the keycap overlay.
    key: Option<(bool, Instant)>,
}

struct Saying {
    words: Vec<String>,
    started: Instant,
    duration: Duration,
}

impl Demo {
    pub(in crate::app::assistant) fn from_env() -> Option<Self> {
        let path = std::env::var_os("HORIZON_DESK_SCRIPT")?;
        Some(Self {
            path: path.into(),
            consumed: 0,
            saying: None,
            envelope: Vec::new(),
            click: None,
            key: None,
        })
    }

    /// Whether the mic should look like it is listening.
    pub(in crate::app::assistant) fn is_listening(&self) -> bool {
        self.saying.is_some()
    }

    /// How loud the voice is right now, while dictating with a recorded voice.
    pub(in crate::app::assistant) fn level(&self) -> Option<f32> {
        let saying = self.saying.as_ref()?;
        if self.envelope.is_empty() {
            return None;
        }
        let index = crate::app::assistant::num::index(saying.started.elapsed().as_secs_f32() * 20.0);
        Some(self.envelope.get(index).copied().unwrap_or(0.0))
    }

    /// The tile clicked in the last second, and how far the click animation has run (0 to 1).
    pub(in crate::app::assistant) fn click_progress(&self) -> Option<(usize, f32)> {
        let (tile, at) = self.click?;
        let progress = at.elapsed().as_secs_f32() / 0.9;
        (progress < 1.0).then_some((tile, progress))
    }

    /// The shortcut pressed in the last second, and how far its animation has run (0 to 1).
    pub(in crate::app::assistant) fn key_progress(&self) -> Option<(bool, f32)> {
        let (right, at) = self.key?;
        let progress = at.elapsed().as_secs_f32() / 1.1;
        (progress < 1.0).then_some((right, progress))
    }

    /// Appends the moment a command ran, so a recording can be lined up with its sound.
    pub(in crate::app::assistant) fn log(&self, line: &str) {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis());
        let mut path = self.path.clone().into_os_string();
        path.push(".log");
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            use std::io::Write as _;
            let _ = writeln!(file, "{millis} {line}");
        }
    }

    /// New complete lines since the last call.
    fn new_lines(&mut self) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        let Some(unread) = text.get(self.consumed..) else {
            return Vec::new();
        };
        let complete = unread.rfind('\n').map_or(0, |end| end + 1);
        let lines = unread[..complete].lines().map(str::to_string).collect();
        self.consumed += complete;
        lines
    }
}

impl HorizonApp {
    /// Runs the demo commands that arrived and advances the dictation.
    pub(super) fn run_demo(&mut self, ctx: &egui::Context) {
        let Some(demo) = self.assistant.demo.as_mut() else {
            return;
        };
        let lines = demo.new_lines();
        for line in lines {
            self.run_demo_line(&line);
        }
        let Some(demo) = self.assistant.demo.as_mut() else {
            return;
        };
        if let Some(saying) = demo.saying.as_ref() {
            let elapsed = saying.started.elapsed().as_millis();
            let total = saying.duration.as_millis().max(100);
            // Words appear in proportion to the time spent, rounding up so the first shows at once.
            let shown = usize::try_from((saying.words.len() as u128 * elapsed).div_ceil(total)).unwrap_or(usize::MAX);
            let text = saying.words[..shown.min(saying.words.len())].join(" ");
            let finished = elapsed >= total;
            self.assistant.summon.text = text;
            if finished {
                demo.saying = None;
            }
            ctx.request_repaint();
        }
    }

    fn run_demo_line(&mut self, line: &str) {
        if let Some(demo) = self.assistant.demo.as_ref() {
            demo.log(line.trim());
        }
        let mut parts = line.trim().splitn(3, ' ');
        let (Some(command), argument, rest) = (parts.next(), parts.next(), parts.next()) else {
            return;
        };
        match command {
            "say" => {
                let millis: u64 = argument.and_then(|value| value.parse().ok()).unwrap_or(3000);
                if let Some(demo) = self.assistant.demo.as_mut() {
                    demo.saying = Some(Saying {
                        words: rest
                            .unwrap_or_default()
                            .split_whitespace()
                            .map(str::to_string)
                            .collect(),
                        started: Instant::now(),
                        duration: Duration::from_millis(millis),
                    });
                }
                self.assistant.summon.text.clear();
            }
            "voice" => {
                // The level of the recorded voice, one number per line, 20 per second.
                let levels = argument
                    .and_then(|path| std::fs::read_to_string(path).ok())
                    .map(|text| text.lines().filter_map(|line| line.trim().parse().ok()).collect())
                    .unwrap_or_default();
                if let Some(demo) = self.assistant.demo.as_mut() {
                    demo.envelope = levels;
                }
            }
            "enter" => {
                let entry = command_bar::parse(&self.assistant.summon.text);
                self.submit_summon(&entry);
            }
            "expand" => {
                self.assistant.summon.expanded = true;
                self.assistant.summon.style = match argument {
                    Some("b") => super::ExpandStyle::Split,
                    Some("c") => super::ExpandStyle::Stage,
                    _ => super::ExpandStyle::Sheet,
                };
            }
            "collapse" => self.assistant.summon.expanded = false,
            "overview" => {
                if let Some(desk) = self.assistant.desk.as_ref() {
                    desk.toggle_overview();
                }
            }
            "move" => self.demo_move(argument, rest),
            "key" => self.demo_key(argument),
            "go" => self.demo_go(argument),
            "scope" => self.demo_scope(argument, rest),
            // `mark` only needs to be logged, so a recording can name its scenes.
            _ => {}
        }
    }
    /// `move <desktop number> <panel title>`: as if the window were dragged there.
    fn demo_move(&self, argument: Option<&str>, title: Option<&str>) {
        let (Some(number), Some(title)) = (argument.and_then(|value| value.parse::<usize>().ok()), title) else {
            return;
        };
        let app_id = self
            .board
            .panels
            .iter()
            .find(|panel| panel.display_title().contains(title))
            .map(|panel| crate::app::desk::panel_app_id(&panel.local_id));
        if let (Some(desk), Some(app_id)) = (self.assistant.desk.as_ref(), app_id) {
            desk.move_window(&app_id, number.saturating_sub(1));
        }
    }

    /// `key right|left`: GNOME's own workspace shortcut, with a keycap overlay.
    fn demo_key(&mut self, direction: Option<&str>) {
        let right = direction != Some("left");
        if let Some(desk) = self.assistant.desk.as_ref() {
            desk.press_key(right);
        }
        if let Some(demo) = self.assistant.demo.as_mut() {
            demo.key = Some((right, Instant::now()));
        }
    }

    /// `go <number>`: a click on that workspace's tile.
    fn demo_go(&mut self, argument: Option<&str>) {
        let Some(index) = argument.and_then(|value| value.parse::<usize>().ok()) else {
            return;
        };
        if let Some(desk) = self.assistant.desk.as_ref() {
            desk.switch(index.saturating_sub(1));
        }
        if let Some(demo) = self.assistant.demo.as_mut() {
            demo.click = Some((index.saturating_sub(1), Instant::now()));
        }
    }

    /// `scope all` or `scope <workspace name>`.
    fn demo_scope(&mut self, argument: Option<&str>, rest: Option<&str>) {
        let wanted = format!(
            "{}{}",
            argument.unwrap_or_default(),
            rest.map_or(String::new(), |rest| format!(" {rest}"))
        );
        if wanted == "all" {
            self.assistant.scope.set_all();
        } else if let Some(workspace) = self.board.workspaces.iter().find(|workspace| workspace.name == wanted) {
            let local_id = workspace.local_id.clone();
            self.assistant.scope.set_only(&local_id);
        }
    }
}
