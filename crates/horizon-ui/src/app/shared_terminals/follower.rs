//! The follower side of the pump: the other computers and the panels that they
//! share, and the streams of the follow panels of this window. Every text, grid
//! and byte here comes from another computer: the feed of a follow panel bounds
//! the stream, and the texts of a list are cut and cleaned where they enter.

use std::time::{Duration, Instant};

use horizon_sync::{LiveComputer, LiveEndReason, LiveEvent, LivePanel, MAX_LIVE_PANELS};

use super::list::computer_name;
use super::pump::{FollowPanel, Run};
use super::{LiveLink, shown};

/// The sequence with which the bytes of a screen enter the alternate screen on
/// which its owner took it: after the reset of a fresh screen, or at the first
/// byte of a screen that is not fresh.
const ALT_SCREEN_ENTER: &[u8] = b"\x1b[?1049h";
/// The wait for a screen that this computer asked for before it asks again.
/// The server passes one request a second and drops the rest.
const ASK_AGAIN: Duration = Duration::from_millis(1200);
/// The wait between two grids that this computer gives the program of a panel
/// that it follows. The server passes two requests a second and drops the rest.
const ASK_GRID: Duration = Duration::from_millis(500);
/// The most computers of a list that this window keeps. A server lists 32.
const MAX_COMPUTERS: usize = 64;
/// Why every stream ends when the live link of this computer does.
pub(super) const NO_LINK: &str = "this computer has no link to its server";

/// The stream of one follow panel of this window.
pub(super) struct Follow {
    panel: FollowPanel,
    /// The stream ended. A list that names the panel starts it again.
    ended: bool,
    /// The next output is the first of a screen.
    screen_start: bool,
    alt: Alt,
    /// When this computer asks the owner for a new screen.
    ask_at: Option<Instant>,
    /// The grid of the newest screen of the stream.
    grid: (u16, u16),
    /// The grid that this computer holds: it asked for it and no screen changed
    /// it away, so a change of the body of the panel asks again.
    asked: Option<(u16, u16)>,
    /// When this computer may give the program of the panel another grid.
    grid_at: Option<Instant>,
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

/// The chip of a follow panel: its computer, and that the person only looks.
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

/// A computer of a list as it enters the state of this window: every text cut
/// and cleaned, and no more panels than one computer shares.
fn cleaned(mut computer: LiveComputer) -> LiveComputer {
    computer.panels.truncate(MAX_LIVE_PANELS);
    (computer.device, computer.label) = (shown(&computer.device), shown(&computer.label));
    for panel in &mut computer.panels {
        (panel.id, panel.title, panel.kind) = (shown(&panel.id), shown(&panel.title), shown(&panel.kind));
        (panel.state, panel.workspace) = (shown(&panel.state), shown(&panel.workspace));
    }
    computer
}

/// The computer `device` and its panel `id` in a list.
pub(super) fn listed<'a>(
    computers: &'a [LiveComputer],
    device: &str,
    id: &str,
) -> Option<(&'a LiveComputer, &'a LivePanel)> {
    let computer = computers.iter().find(|computer| computer.device == device)?;
    let panel = computer.panels.iter().find(|panel| panel.id == id)?;
    Some((computer, panel))
}

impl Run {
    /// Starts the follow of each follow panel that the window opened. A panel
    /// that the person closed is unfollowed first, so a new panel of the same
    /// terminal follows after it.
    pub(super) fn start_follows(&mut self) {
        self.drop_closed();
        for wanted in &self.wanted.follows {
            let known = self.follows.iter().any(|follow| follow.panel.local == wanted.local);
            if known || wanted.feed.closed() {
                continue;
            }
            self.link.follow(&wanted.device, &wanted.id);
            if let Some((computer, panel)) = listed(&self.found.computers, &wanted.device, &wanted.id) {
                let chip = follow_chip(computer, panel.typing);
                wanted.feed.set_typing(panel.typing);
                self.found.chips.insert(wanted.local.clone(), chip);
                self.changed = true;
            }
            self.follows.push(Follow {
                panel: wanted.clone(),
                ended: false,
                screen_start: false,
                alt: Alt::No,
                ask_at: None,
                grid: wanted.feed.grid(),
                asked: None,
                grid_at: None,
            });
        }
    }

    /// Unfollows each panel that the person closed.
    fn drop_closed(&mut self) {
        let (link, chips, changed) = (self.link.as_ref(), &mut self.found.chips, &mut self.changed);
        self.follows.retain(|Follow { panel, .. }| {
            let closed = panel.feed.closed();
            if closed {
                link.unfollow(&panel.device, &panel.id);
                *changed |= chips.remove(&panel.local).is_some();
            }
            !closed
        });
    }

    /// Takes the list of the computers. A follow panel whose stream ended
    /// follows again when the list names its panel, and its chip is read again.
    pub(super) fn computers(&mut self, computers: Vec<LiveComputer>) {
        let others = computers.into_iter().filter(|computer| !computer.own);
        self.found.computers = others.take(MAX_COMPUTERS).map(cleaned).collect();
        self.changed = true;
        for Follow { panel, ended, .. } in &mut self.follows {
            let Some((computer, shared)) = listed(&self.found.computers, &panel.device, &panel.id) else {
                continue;
            };
            if std::mem::take(ended) {
                self.link.follow(&panel.device, &panel.id);
            }
            // The open panel takes the typing of the newest list, in both
            // directions, and its chip says which one it is.
            let chip = follow_chip(computer, shared.typing);
            panel.feed.set_typing(shared.typing);
            self.found.chips.insert(panel.local.clone(), chip);
        }
    }

    /// Gives one event of a stream to the feed of the panel that follows it.
    /// The feed asks the window for the frame that shows the bytes.
    pub(super) fn stream(&mut self, event: LiveEvent) {
        let (LiveEvent::Screen { owner, panel, .. }
        | LiveEvent::Output { owner, panel, .. }
        | LiveEvent::Ended { owner, panel, .. }) = &event
        else {
            return;
        };
        let of = |follow: &&mut Follow| follow.panel.device == *owner && follow.panel.id == *panel;
        let Some(follow) = self.follows.iter_mut().find(of) else {
            return;
        };
        let feed = &follow.panel.feed;
        match event {
            LiveEvent::Screen { cols, rows, fresh, .. } => {
                feed.screen(fresh, cols, rows);
                (follow.ended, follow.screen_start, follow.alt, follow.ask_at) = (false, true, Alt::No, None);
                // A screen that does not bring the grid that this computer asked
                // for ends that hold, so the next key press asks again.
                follow.grid = (cols, rows);
            }
            LiveEvent::Output { data, .. } => {
                let head = &data.0[..data.0.len().min(ALT_SCREEN_ENTER.len() + 2)];
                if std::mem::take(&mut follow.screen_start)
                    && head
                        .windows(ALT_SCREEN_ENTER.len())
                        .any(|window| window == ALT_SCREEN_ENTER)
                {
                    follow.alt = Alt::Joined;
                }
                feed.output(&data.0);
            }
            LiveEvent::Ended { reason, .. } => {
                feed.ended(ended_text(reason));
                // A stream over a limit would end again with every list.
                (follow.ended, follow.ask_at) = (reason != LiveEndReason::Limit, None);
            }
            _ => {}
        }
    }

    /// Runs every follow panel for one pass: what the person typed goes out, a
    /// stream that lost output or left its first alternate screen asks for a
    /// new screen, a panel that the person closed is unfollowed, and the grid of
    /// the body of the panel goes to the program that the person typed at.
    pub(super) fn pump_follows(&mut self, now: Instant) {
        self.drop_closed();
        for follow in &mut self.follows {
            let FollowPanel { device, id, feed, .. } = &follow.panel;
            let typed = feed.take_typed();
            let pressed = !typed.is_empty();
            if pressed {
                self.link.typed(device, id, &typed);
            }
            if feed.take_lost() {
                follow.ask_at.get_or_insert(now);
            }
            if follow.alt != Alt::No && feed.on_alternate_screen() {
                follow.alt = Alt::Entered;
            } else if follow.alt == Alt::Entered {
                follow.alt = Alt::No;
                follow.ask_at.get_or_insert(now);
            }
            if follow.ask_at.is_some_and(|at| at <= now) {
                self.link.refresh(device, id);
                follow.ask_at = Some(now + ASK_AGAIN);
            }
            ask_grid(self.link.as_ref(), follow, pressed, now);
        }
    }

    /// Ends every stream, as the end of the link does. The follow panels stay.
    pub(super) fn follows_lost(&mut self) {
        self.found.computers.clear();
        for follow in &mut self.follows {
            follow.panel.feed.ended(NO_LINK);
            (follow.ended, follow.ask_at, follow.asked, follow.grid_at) = (true, None, None, None);
        }
    }
}

/// Gives the program of a panel of another computer the grid of the body of the
/// panel that follows it, when the person typed in it or this computer holds the
/// grid and the body changed. The same grid is asked once, and two requests a
/// second come out of one panel at most.
fn ask_grid(link: &dyn LiveLink, follow: &mut Follow, typed: bool, now: Instant) {
    let (fit, holds) = (follow.panel.feed.fit(), follow.asked == Some(follow.grid));
    let due = follow.grid_at.is_none_or(|at| at <= now);
    if !due || fit == follow.grid || !(typed || holds) {
        return;
    }
    let FollowPanel { device, id, .. } = &follow.panel;
    link.resize(device, id, fit.0, fit.1);
    (follow.asked, follow.grid_at) = (Some(fit), Some(now + ASK_GRID));
}
