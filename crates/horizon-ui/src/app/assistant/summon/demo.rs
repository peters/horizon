//! Scripted input for demos (desk mode only, opt in with `HORIZON_DESK_SCRIPT`).
//!
//! The file named by the variable is read as it grows. Each line is one command:
//! `say <ms> <text>` reveals the text word by word over that many milliseconds
//! while the mic shows recording, as if it were being dictated; `enter` submits
//! the prompt; `expand a|b|c` and `collapse` change the bar; `go <n>` switches
//! desktop workspace; `scope all|<workspace name>` narrows the assistant.

use std::time::{Duration, Instant};

use horizon_core::browser::manifest::agent_panels::AgentState;

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
    /// The latest level of a live voice (input or output), refreshed about twenty times a second.
    live_level: Option<(f32, Instant)>,
    /// What the assistant is saying right now.
    speaking: Option<(String, Instant)>,
    /// A button the script just pressed, for the click animation.
    press: Option<Instant>,
    /// A hub button the script just pressed.
    page_press: Option<(super::HubPage, Instant)>,
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
            live_level: None,
            speaking: None,
            press: None,
            page_press: None,
        })
    }

    /// Whether the mic should look like it is listening.
    pub(in crate::app::assistant) fn is_listening(&self) -> bool {
        self.saying.is_some()
    }

    /// How loud the voice is right now: a live voice if one is reporting, else a recorded one.
    pub(in crate::app::assistant) fn level(&self) -> Option<f32> {
        if let Some((level, at)) = self.live_level
            && at.elapsed() < Duration::from_millis(250)
        {
            return Some(level);
        }
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

    /// Whether a live voice is being heard or is speaking.
    pub(in crate::app::assistant) fn voice_active(&self) -> bool {
        self.live_level
            .is_some_and(|(_, at)| at.elapsed() < Duration::from_millis(250))
    }

    /// The hub button pressed in the last second, and how far its click animation has run.
    pub(in crate::app::assistant) fn page_press(&self) -> Option<(super::HubPage, f32)> {
        let (page, at) = self.page_press?;
        let progress = at.elapsed().as_secs_f32() / 0.9;
        (progress < 1.0).then_some((page, progress))
    }

    /// How far the click animation on a pressed button has run (0 to 1).
    pub(in crate::app::assistant) fn press_progress(&self) -> Option<f32> {
        let progress = self.press?.elapsed().as_secs_f32() / 0.9;
        (progress < 1.0).then_some(progress)
    }

    /// The line the assistant is speaking, for a few seconds after it was reported.
    pub(in crate::app::assistant) fn speaking_line(&self) -> Option<&str> {
        let (text, at) = self.speaking.as_ref()?;
        (at.elapsed() < Duration::from_secs(9)).then_some(text.as_str())
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
        if let Some(demo) = self.assistant.demo.as_ref()
            && !line.trim_start().starts_with("level ")
        {
            demo.log(line.trim());
        }
        let mut parts = line.trim().splitn(3, ' ');
        let (Some(command), argument, rest) = (parts.next(), parts.next(), parts.next()) else {
            return;
        };
        if self.demo_view(command, argument, rest) {
            return;
        }
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
            "level" => {
                let level = argument.and_then(|value| value.parse::<f32>().ok()).unwrap_or(0.0);
                if let Some(demo) = self.assistant.demo.as_mut() {
                    demo.live_level = Some((level.clamp(0.0, 1.0), Instant::now()));
                }
            }
            "speaking" => {
                let text = line
                    .trim()
                    .strip_prefix("speaking")
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                self.assistant.feed.said(&text);
                if let Some(demo) = self.assistant.demo.as_mut() {
                    demo.speaking = Some((text, Instant::now()));
                }
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
                self.assistant.summon.mini = None;
                self.assistant.summon.expanded = true;
                self.assistant.summon.style = match argument {
                    Some("b") => super::ExpandStyle::Split,
                    Some("c") => super::ExpandStyle::Stage,
                    _ => super::ExpandStyle::Sheet,
                };
            }
            "collapse" => self.assistant.summon.expanded = false,
            "answer" => self.demo_answer(argument, rest),
            "ws" => {
                self.assistant.summon.pending_workspace = argument
                    .and_then(|value| value.parse::<usize>().ok())
                    .map(|number| number.saturating_sub(1));
            }
            "hub" => {
                self.assistant.summon.hub.style = match argument {
                    Some("b") => super::HubStyle::Window,
                    Some("c") => super::HubStyle::Inline,
                    _ => super::HubStyle::Satellites,
                };
                self.assistant.summon.hub.close();
            }
            "page" => self.demo_page(argument),
            "type" => self.demo_type(argument, rest),
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
    /// `page nav|hosts|cloud|sessions|settings|close`: presses that hub button.
    fn demo_page(&mut self, argument: Option<&str>) {
        let page = match argument {
            Some("nav") => super::HubPage::Nav,
            Some("hosts") => super::HubPage::Hosts,
            Some("cloud") => super::HubPage::Cloud,
            Some("sessions") => super::HubPage::Sessions,
            Some("settings") => super::HubPage::Settings,
            _ => {
                self.assistant.summon.hub.close();
                return;
            }
        };
        if let Some(demo) = self.assistant.demo.as_mut() {
            demo.page_press = Some((page, Instant::now()));
        }
        // Pressing a page that is open already would close it; the script means "show this one".
        if self.assistant.summon.hub.page != Some(page) {
            self.press_hub_button(page);
        }
    }

    /// `type <panel title> <text>`: types a line into an agent, as the person would.
    fn demo_type(&mut self, title: Option<&str>, text: Option<&str>) {
        let (Some(title), Some(text)) = (title, text) else {
            return;
        };
        let target = self
            .board
            .panels
            .iter()
            .find(|panel| panel.display_title() == title)
            .map(|panel| panel.id);
        if let Some(id) = target {
            self.send_to_agent(id, text, true, Instant::now());
        }
    }

    /// The commands that arrange the concierge and its text agent: `layout`, `engine`, `typeto`, `pick`,
    /// `sheet`, `drawer` and `channel`. Returns whether the command was one of them.
    fn demo_agents(&mut self, command: &str, argument: Option<&str>, rest: Option<&str>, text: &str) -> bool {
        match command {
            "layout" => {
                self.assistant.summon.layout = match argument {
                    Some("b") => super::dock::Layout3::Feed,
                    Some("c") => super::dock::Layout3::Roster,
                    _ => super::dock::Layout3::Split,
                };
                self.assistant.summon.sheet = None;
            }
            "engine" => {
                let kind = match argument {
                    Some("codex") => horizon_core::PanelKind::Codex,
                    Some("grok") => horizon_core::PanelKind::Grok,
                    _ => horizon_core::PanelKind::Claude,
                };
                self.assistant.draft.agent = kind;
                self.apply_assistant_engine();
            }
            "typeto" => {
                // Types into the text agent, as the person would in its terminal.
                if let Some(id) = self.board.assistant_panel() {
                    self.send_to_agent(id, text, true, Instant::now());
                }
            }
            "pick" => {
                self.assistant.summon.pick = match argument {
                    Some("text") => super::dock::Pick::Text,
                    Some("worker") => rest
                        .and_then(|title| self.board.panels.iter().find(|panel| panel.display_title() == title))
                        .map_or(super::dock::Pick::Voice, |panel| super::dock::Pick::Worker(panel.id)),
                    _ => super::dock::Pick::Voice,
                };
            }
            "sheet" => {
                // `sheet <width> <height>`, or `sheet reset`: as if the window had been resized.
                let size = match (
                    argument.and_then(|w| w.parse::<f32>().ok()),
                    rest.and_then(|h| h.parse::<f32>().ok()),
                ) {
                    (Some(width), Some(height)) => [width, height],
                    _ => [0.0, 0.0],
                };
                self.assistant.summon.sheet_request = Some(size);
            }
            "drawer" => self.assistant.summon.drawer_open = argument != Some("off"),
            "channel" => {
                self.assistant.summon.channel = if argument == Some("text") {
                    super::dock::Channel::Text
                } else {
                    super::dock::Channel::Voice
                };
            }
            _ => return false,
        }
        true
    }

    /// The commands that change how the bar looks: `mini`, `unmini`, `feed`, `terminal`, `deck`.
    /// Returns whether the command was one of them.
    fn demo_view(&mut self, command: &str, argument: Option<&str>, rest: Option<&str>) -> bool {
        let text = [argument, rest].into_iter().flatten().collect::<Vec<_>>().join(" ");
        match command {
            "mini" => {
                let style = match argument {
                    Some("b") => super::MiniStyle::Strip,
                    Some("c") => super::MiniStyle::Orb,
                    Some("d") => super::MiniStyle::Deck,
                    _ => super::MiniStyle::Pill,
                };
                self.assistant.summon.mini_style = style;
                self.assistant.summon.mini = Some(style);
            }
            "unmini" => self.assistant.summon.mini = None,
            "dock" => {
                self.assistant.summon.dock_style = Some(match argument {
                    Some("m") => super::dock::DockStyle::Mission,
                    Some("l") => super::dock::DockStyle::Lens,
                    _ => super::dock::DockStyle::Concierge,
                });
            }
            "you" => self.assistant.feed.you(&text, true),
            "said" => self.assistant.feed.said(&text),
            "did" => self.assistant.feed.did(text),
            "thread" => {
                // `thread <space>|<title>|<minutes ago>`: a thread to show in the history.
                let parts: Vec<&str> = text.split('|').map(str::trim).collect();
                if let [space, title, minutes] = parts[..] {
                    let ago = minutes.parse::<i64>().unwrap_or(0) * 60_000;
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(0));
                    self.assistant.threads.upsert(horizon_core::assistant::Thread {
                        session_id: format!("demo-{title}"),
                        agent: horizon_core::PanelKind::Claude,
                        title: title.to_string(),
                        space: space.to_string(),
                        cwd: None,
                        updated_at: now - ago,
                    });
                }
            }
            "allowlow" => {
                if let Some(demo) = self.assistant.demo.as_mut() {
                    demo.press = Some(Instant::now());
                }
                self.allow_low_risk();
            }
            "next" => {
                // The "Next" button of the lens: the panel of the most careful question.
                if let Some(ask) = self.inbox().first() {
                    self.assistant.summon.pending_reveal = Some(ask.id);
                }
            }
            "chat" => self.assistant.summon.board_chat = argument != Some("off"),
            "feedclear" => self.assistant.feed.clear(),
            "follow" => self.assistant.summon.scope_follow = argument != Some("off"),
            "feed" => {
                self.assistant.summon.feed_style = match argument {
                    Some("b") => super::FeedStyle::Cards,
                    _ => super::FeedStyle::Chat,
                };
            }
            "terminal" => self.assistant.summon.raw = argument != Some("off"),
            "deck" => {
                // The person presses a card's button: Open (the cards view) or Terminal.
                self.assistant.summon.mini = None;
                self.assistant.summon.expanded = true;
                self.assistant.summon.style = super::ExpandStyle::Sheet;
                self.assistant.summon.feed_style = super::FeedStyle::Cards;
                self.assistant.summon.raw = argument == Some("terminal");
            }
            _ => return self.demo_agents(command, argument, rest, &text),
        }
        true
    }

    /// `answer yes|no`: the person presses the button on the card of the first agent that asked.
    fn demo_answer(&mut self, argument: Option<&str>, agent: Option<&str>) {
        let asking = self
            .board
            .panels
            .iter()
            .filter(|panel| self.board.agent_state(panel.id) == Some(AgentState::NeedsInput))
            .find(|panel| agent.is_none_or(|title| panel.display_title() == title))
            .map(|panel| panel.id);
        if let Some(id) = asking {
            if let Some(demo) = self.assistant.demo.as_mut() {
                demo.press = Some(Instant::now());
            }
            self.answer_agent(id, argument != Some("no"));
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
