//! The follower side of shared terminals: the other computers and the panels
//! that they share, and the follow panels of this window. Every text, grid and
//! byte here comes from another computer: the feed of a follow panel bounds the
//! stream, the server bounds the list, and this module shows a text only cut.

use std::time::{Duration, Instant};

use alacritty_terminal::term::TermMode;
use egui::Context;
use horizon_core::{Attention, FollowFeed, FollowSpec, Panel, PanelId, PanelKind};
use horizon_sync::{LiveComputer, LiveEndReason, LiveEvent, LivePanel};

use super::super::HorizonApp;
use super::owner::STATES;
use super::{LiveLink, shown};

/// The first bytes of a screen that its owner took on the alternate screen.
const ALT_SCREEN_START: &[u8] = b"\x1bc\x1b[?1049h";
/// The wait for a screen that this computer asked for before it asks again.
/// The server passes one request a second and drops the rest.
const ASK_AGAIN: Duration = Duration::from_millis(1200);

#[derive(Default)]
pub(in crate::app) struct Follower {
    /// The other computers that hold a live link, with the panels they share.
    pub(in crate::app) computers: Vec<LiveComputer>,
    follows: Vec<Follow>,
}

/// One follow panel of this window.
struct Follow {
    panel: PanelId,
    /// The computer and the panel that it follows, as the list names them.
    device: String,
    id: String,
    feed: FollowFeed,
    /// Whether the person may type, as the panel was opened.
    typing: bool,
    /// The stream ended. A list that names the panel starts it again.
    ended: bool,
    /// The next output is the first of a screen.
    screen_start: bool,
    alt: Alt,
    /// When this computer asks the owner for a new screen.
    ask_at: Option<Instant>,
}

/// Whether the newest screen started on the alternate screen of its owner. The
/// primary screen under it does not travel, so the follower asks for a new
/// screen when its terminal leaves the alternate screen that it joined on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Alt {
    No,
    /// The screen started there, and the terminal has not drawn it yet.
    Joined,
    Entered,
}

/// The name of a computer for the person: its label, or its device name.
pub(in crate::app) fn computer_name(computer: &LiveComputer) -> &str {
    let label = Some(computer.label.as_str()).filter(|label| !label.is_empty());
    label.unwrap_or(&computer.device)
}

/// The kind that a list names, which decides the keyboard protocol of the
/// follow panel. A name of no terminal kind reads as a shell.
pub(in crate::app) fn kind_of(code: &str) -> PanelKind {
    let kind = serde_json::from_value(serde_json::Value::String(code.to_owned())).unwrap_or_default();
    match kind {
        PanelKind::Editor | PanelKind::Browser | PanelKind::Device | PanelKind::Block => PanelKind::Shell,
        kind => kind,
    }
}

/// The state that a list names, for the mark of a row.
pub(in crate::app) fn attention_of(code: &str) -> Attention {
    let named = STATES.iter().find(|(_, name)| *name == code);
    named.map_or(Attention::Idle, |(state, _)| *state)
}

/// The chip of a follow panel: the computer that runs the terminal, and that
/// the person only looks while they may not type.
fn follow_chip(computer: &LiveComputer, typing: bool) -> String {
    let view = if typing { "" } else { " \u{00b7} view only" };
    format!("on {}{view}", shown(computer_name(computer)))
}

/// Why a stream ended, for the line under the grid of its follow panel.
fn ended_text(reason: LiveEndReason) -> &'static str {
    match reason {
        LiveEndReason::Closed => "the other computer closed this terminal or shares it no more",
        LiveEndReason::Offline => "the other computer is offline",
        LiveEndReason::NotFound => "the other computer does not share this terminal",
        LiveEndReason::Limit => "too many computers follow this terminal, or this computer follows too many",
    }
}

/// The computer `device` and its panel `id` in a list.
fn listed<'a>(computers: &'a [LiveComputer], device: &str, id: &str) -> Option<(&'a LiveComputer, &'a LivePanel)> {
    let computer = computers.iter().find(|computer| computer.device == device)?;
    let panel = computer.panels.iter().find(|panel| panel.id == id)?;
    Some((computer, panel))
}

impl HorizonApp {
    /// Opens the panel that follows the panel `id` of the computer `device` in
    /// the active workspace, or goes to the one that is open.
    pub(in crate::app) fn open_shared_terminal(&mut self, device: &str, id: &str, ctx: &Context) {
        let follower = &self.shared_terminals.follower;
        let open = follower
            .follows
            .iter()
            .find(|follow| follow.device == device && follow.id == id);
        if let Some(panel) = open.map(|follow| follow.panel) {
            self.reveal_selected_panel(ctx, panel);
            return;
        }
        let Some((computer, shared)) = listed(&follower.computers, device, id) else {
            return;
        };
        let (chip, typing) = (follow_chip(computer, shared.typing), shared.typing);
        let spec = FollowSpec {
            device: device.to_owned(),
            label: computer_name(computer).to_owned(),
            panel: id.to_owned(),
            title: shared.title.clone(),
            kind: kind_of(&shared.kind),
            cols: shared.cols,
            rows: shared.rows,
            typing,
        };
        let workspace = match self.board.active_workspace {
            Some(workspace) => workspace,
            None => self.ensure_workspace_visible(ctx),
        };
        let created = self.board.create_follow_panel(workspace, spec);
        let feed = created
            .as_ref()
            .ok()
            .and_then(|panel| self.board.panel(*panel)?.follow_feed());
        let (Ok(panel), Some(feed)) = (created, feed) else {
            self.set_sidebar_notice("This terminal of another computer did not open.".to_owned());
            return;
        };
        let shared = &mut self.shared_terminals;
        if let Some(link) = &shared.link {
            link.follow(device, id);
        }
        shared.chips.insert(panel, chip);
        shared.follower.follows.push(Follow {
            panel,
            device: device.to_owned(),
            id: id.to_owned(),
            feed,
            typing,
            ended: shared.link.is_none(),
            screen_start: false,
            alt: Alt::No,
            ask_at: None,
        });
        self.reveal_selected_panel(ctx, panel);
    }

    /// Takes the list of the computers. A follow panel whose stream ended
    /// follows again when the list names its panel, and its chip is read again.
    pub(super) fn shared_computers(&mut self, link: &dyn LiveLink, computers: Vec<LiveComputer>) {
        let shared = &mut self.shared_terminals;
        shared.follower.computers = computers.into_iter().filter(|computer| !computer.own).collect();
        for follow in &mut shared.follower.follows {
            let Some((computer, panel)) = listed(&shared.follower.computers, &follow.device, &follow.id) else {
                continue;
            };
            if std::mem::take(&mut follow.ended) {
                link.follow(&follow.device, &follow.id);
            }
            let chip = follow_chip(computer, panel.typing && follow.typing);
            shared.chips.insert(follow.panel, chip);
        }
    }

    /// Gives one event of a stream to the feed of the panel that follows it.
    pub(super) fn follow_event(&mut self, event: LiveEvent) {
        let (LiveEvent::Screen { owner, panel, .. }
        | LiveEvent::Output { owner, panel, .. }
        | LiveEvent::Ended { owner, panel, .. }) = &event
        else {
            return;
        };
        let follows = &mut self.shared_terminals.follower.follows;
        let Some(follow) = follows
            .iter_mut()
            .find(|follow| follow.device == *owner && follow.id == *panel)
        else {
            return;
        };
        // The frame that shows these bytes follows the first of them, as the
        // frame of an echo does.
        if let Some(terminal) = self.board.panel(follow.panel).and_then(Panel::terminal) {
            terminal.arm_output_wake();
        }
        match event {
            LiveEvent::Screen { cols, rows, .. } => {
                follow.feed.screen(cols, rows);
                (follow.ended, follow.screen_start, follow.alt, follow.ask_at) = (false, true, Alt::No, None);
            }
            LiveEvent::Output { data, .. } => {
                if std::mem::take(&mut follow.screen_start) && data.0.starts_with(ALT_SCREEN_START) {
                    follow.alt = Alt::Joined;
                }
                follow.feed.output(&data.0);
            }
            LiveEvent::Ended { reason, .. } => {
                follow.feed.ended(ended_text(reason));
                // A stream over a limit would end again with every list.
                (follow.ended, follow.ask_at) = (reason != LiveEndReason::Limit, None);
            }
            _ => {}
        }
    }

    /// Runs every follow panel for one frame: what the person typed goes out, a
    /// stream that lost output or left its first alternate screen asks for a
    /// new screen, and a panel that the person closed is unfollowed.
    pub(super) fn pump_follows(&mut self, link: &dyn LiveLink, ctx: &Context, now: Instant) {
        let (board, shared) = (&self.board, &mut self.shared_terminals);
        let chips = &mut shared.chips;
        shared.follower.follows.retain_mut(|follow| {
            if follow.feed.closed() {
                link.unfollow(&follow.device, &follow.id);
                chips.remove(&follow.panel);
                return false;
            }
            let typed = follow.feed.take_typed();
            if !typed.is_empty() {
                link.typed(&follow.device, &follow.id, &typed);
            }
            if follow.feed.take_lost() {
                follow.ask_at.get_or_insert(now);
            }
            if follow.alt != Alt::No
                && let Some(terminal) = board.panel(follow.panel).and_then(Panel::terminal)
            {
                if terminal.mode().contains(TermMode::ALT_SCREEN) {
                    follow.alt = Alt::Entered;
                } else if follow.alt == Alt::Entered {
                    follow.alt = Alt::No;
                    follow.ask_at.get_or_insert(now);
                }
            }
            if follow.ask_at.is_some_and(|at| at <= now) {
                link.refresh(&follow.device, &follow.id);
                follow.ask_at = Some(now + ASK_AGAIN);
                ctx.request_repaint_after(ASK_AGAIN);
            }
            true
        });
    }

    /// Ends every stream, as the end of the link does. The follow panels stay.
    pub(super) fn follows_lost(&mut self) {
        let follower = &mut self.shared_terminals.follower;
        follower.computers.clear();
        for follow in &mut follower.follows {
            follow.feed.ended("this computer has no link to its server");
            (follow.ended, follow.ask_at) = (true, None);
        }
    }
}
