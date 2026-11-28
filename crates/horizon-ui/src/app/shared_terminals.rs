//! Shared terminals: the terminal panels of this computer that the other
//! computers of the person see and type in, and the panels of those computers
//! that this window follows.
//!
//! The live link of `horizon-sync` carries both. It starts and ends with the
//! sync link of this computer. It starts again when the sync link comes back
//! after it was away, because the live link has no keepalive of its own: a list
//! then never outlives its link by more than the sync link does. A new name of
//! this computer starts it again too. A link that is down or that the server
//! does not support shares nothing and fails nothing.
//!
//! The bytes move on the thread of the pump (`pump.rs`), which starts and stops
//! with the link, so a window that gets no frame still shares and follows. A
//! frame does what needs the board: it builds the list of this computer
//! (`list.rs`), tells the pump the panels, the follow panels and the two
//! switches, opens a follow panel, and draws what the pump found. Without a
//! frame the list goes stale, and nothing else does.

mod follower;
mod link;
mod list;
mod owner;
mod pump;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Instant;

use egui::Context;
use horizon_core::{Config, FollowSpec, ServerConfig};
use horizon_sync::{LiveSupervisor, SupervisorConfig};

use super::HorizonApp;
use super::session_mirror::SessionMirror;
use crate::block_widget::{CardChip, chip::ChipState};
pub(in crate::app) use link::LiveLink;
pub(in crate::app) use list::{attention_of, computer_name, kind_of};
use pump::{FollowPanel, Found, Pump, Wanted};

/// The most characters of a text of another computer that this window shows.
const SHOWN_CHARS: usize = 80;

/// The state of the join in this window.
#[derive(Default)]
pub(in crate::app) struct SharedTerminals {
    /// The live link of this computer, and the settings that start it again.
    supervisor: Option<LiveSupervisor>,
    config: Option<SupervisorConfig>,
    /// The pump of the link: the handle of `supervisor`, or the stand-in of a test.
    pump: Option<Pump>,
    /// The name setting that the link started with.
    name: String,
    /// Whether the sync link was away on the last frame.
    sync_away: bool,
    /// What this window wants of the pump, as it told the pump last.
    wanted: Wanted,
    /// What the pump found last: the other computers, the chips and the reason
    /// of a link that is down.
    pub(in crate::app) found: Arc<Found>,
    list: list::ListState,
    /// The choice of this window in the group `COMPUTERS`: the computers whose rows
    /// show every session.
    pub(in crate::app) mirror: SessionMirror,
}

impl SharedTerminals {
    /// The chip in the title bar of the panel with the local id `panel`, when
    /// it is watched or follows.
    pub(in crate::app) fn chip(&self, panel: &str) -> Option<CardChip<'_>> {
        let name = self.found.chips.get(panel)?;
        Some(CardChip {
            name,
            state: ChipState::Present,
        })
    }

    /// Terminal output arrived on this frame, so the list is built again.
    pub(in crate::app) fn note_output(&mut self) {
        self.list.dirty = true;
    }

    /// Starts the pump of `link`, which wakes on `shared`, with what this
    /// window wants now. The new pump ends the pump before it.
    fn start_pump(&mut self, shared: Arc<pump::Shared>, link: Box<dyn LiveLink>, ctx: &Context) {
        shared.want(&self.wanted);
        let ctx = ctx.clone();
        match Pump::start(shared, link, move || ctx.request_repaint(), self.pump.take()) {
            Ok(pump) => self.pump = Some(pump),
            Err(error) => tracing::warn!(%error, "the pump of the shared terminals did not start"),
        }
    }

    /// Tells the pump what this window wants now.
    fn send_wanted(&mut self) {
        self.wanted.version += 1;
        if let Some(pump) = &self.pump {
            pump.shared().want(&self.wanted);
        }
    }

    /// Ends the live link, and forgets what only an open link holds: the other
    /// computers, the chips and the streams of the follow panels, which stay
    /// and follow again on a new link. The pump goes with `pump`, or the pump
    /// of the next link ends it.
    fn end_link(&mut self, pump: bool) {
        retire(self.pump.take_if(|_| pump), self.supervisor.take());
        self.found = Arc::default();
        for follow in &self.wanted.follows {
            follow.feed.ended(follower::NO_LINK);
        }
    }
}

/// A text of another computer as one bounded line.
fn shown(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(SHOWN_CHARS).collect()
}

/// The server entry as the settings store holds it: the baseline of the Settings
/// window, or the applied settings while no window edits them. A draft of the name
/// of this computer is not a name that another computer reads, so a key in the name
/// field changes nothing on the live link.
fn stored_server<'a>(stored: Option<&'a Config>, applied: &'a Config) -> &'a ServerConfig {
    &stored.unwrap_or(applied).server
}

/// Whether this computer saved a name that its live link does not have yet.
fn link_renamed(shared: &SharedTerminals, server: &ServerConfig) -> bool {
    shared.name != server.computer_name
}

/// Stops a pump and its live link on a thread of its own, because each stop
/// joins a thread: the pump ends within one pass, and then the link ends.
fn retire(pump: Option<Pump>, supervisor: Option<LiveSupervisor>) {
    if pump.is_none() && supervisor.is_none() {
        return;
    }
    let stop = std::thread::Builder::new().name("live-link-stop".to_owned());
    let _ = stop.spawn(move || {
        drop(pump);
        drop(supervisor);
    });
}

impl HorizonApp {
    /// The part of the join that needs the board, one time in each frame: it
    /// starts and stops the link, tells the pump what this window wants, and
    /// takes what the pump found. It waits for nothing.
    pub(in crate::app) fn poll_shared_terminals(&mut self, ctx: &Context) {
        self.follow_sync_link(ctx);
        if self.shared_terminals.pump.is_none() {
            return;
        }
        let listed = self.build_shared_list(ctx, Instant::now());
        let server = &self.template_config.server;
        let shared = &mut self.shared_terminals;
        let wanted = &mut shared.wanted;
        let shown_mirror = self.orchestrator_dock.front_desk().mirror().cloned();
        let before = (wanted.sharing, wanted.typing, wanted.follows.len());
        wanted.mirror.follow = shown_mirror;
        (wanted.sharing, wanted.typing) = (server.share_terminals, server.remote_typing);
        wanted.follows.retain(|follow| !follow.feed.closed());
        let mut changed = before != (wanted.sharing, wanted.typing, wanted.follows.len());
        if let Some(panels) = listed.filter(|panels| *panels != wanted.panels) {
            (wanted.panels, changed) = (panels, true);
        }
        let streams = !shared.wanted.follows.is_empty();
        if changed {
            shared.send_wanted();
        }
        if let Some(pump) = &shared.pump {
            // The keys of this frame wait in the feed of a follow panel.
            if streams && !changed {
                pump.shared().notify();
            }
            if let Some(found) = pump.shared().found() {
                shared.found = found;
                shared.mirror.retain(&shared.found.mirror.computers);
            }
        }
    }

    /// Opens the panel that follows the panel `id` of the computer `device` in
    /// the active workspace, or goes to the one that is open.
    pub(in crate::app) fn open_shared_terminal(&mut self, device: &str, id: &str, ctx: &Context) {
        let shared = &self.shared_terminals;
        let follows = shared.wanted.follows.iter().filter(|follow| !follow.feed.closed());
        let open = follows
            .into_iter()
            .find(|follow| follow.device == device && follow.id == id);
        if let Some(panel) = open.and_then(|follow| self.board.panel_id_by_local_id(&follow.local)) {
            self.reveal_selected_panel(ctx, panel);
            return;
        }
        let Some((computer, listed)) = follower::listed(&shared.found.computers, device, id) else {
            return;
        };
        let typing = listed.typing;
        let spec = FollowSpec {
            device: device.to_owned(),
            label: computer_name(computer).to_owned(),
            panel: id.to_owned(),
            title: listed.title.clone(),
            kind: kind_of(&listed.kind),
            cols: listed.cols,
            rows: listed.rows,
            typing,
        };
        let workspace = match self.board.active_workspace {
            Some(workspace) => workspace,
            None => self.ensure_workspace_visible(ctx),
        };
        let cell = crate::terminal_widget::grid_cell_size(ctx);
        let created = self.board.create_follow_panel(workspace, spec, [cell.x, cell.y]).ok();
        let opened = created.and_then(|panel| {
            let follow = self.board.panel(panel)?;
            Some((panel, follow.local_id.clone(), follow.follow_feed()?))
        });
        let Some((panel, local, feed)) = opened else {
            self.set_sidebar_notice("This terminal of another computer did not open.".to_owned());
            return;
        };
        let (device, id) = (device.to_owned(), id.to_owned());
        let follow = FollowPanel {
            local,
            device,
            id,
            feed,
        };
        self.shared_terminals.wanted.follows.push(follow);
        self.shared_terminals.send_wanted();
        self.reveal_selected_panel(ctx, panel);
    }

    /// Ends the live link, as the end of the sync link of this computer does.
    pub(in crate::app) fn stop_shared_terminals(&mut self) {
        self.shared_terminals.end_link(true);
        self.shared_terminals.config = None;
    }

    /// Starts the live link with the sync link of this computer, ends it with
    /// it, and starts it again when the sync link came back or the saved name
    /// changed.
    fn follow_sync_link(&mut self, ctx: &Context) {
        let Some(sync) = self.server_sync.as_mut() else {
            if self.shared_terminals.supervisor.is_some() {
                self.stop_shared_terminals();
            }
            return;
        };
        let baseline = self.settings.as_ref().map(|editor| &editor.original);
        let server = stored_server(baseline, &self.template_config);
        let shared = &mut self.shared_terminals;
        let away = sync.snapshot().state().code() == "away";
        let returned = std::mem::replace(&mut shared.sync_away, away) && !away;
        let renamed = link_renamed(shared, server);
        match sync.live.take() {
            Some(config) => shared.config = Some(config),
            None if shared.supervisor.is_some() && (returned || renamed) => {}
            None => return,
        }
        let Some(config) = shared.config.clone() else {
            return;
        };
        // The new link has nothing of the old one, so the pump of the new link
        // gets the list and the follows again.
        shared.end_link(false);
        shared.name.clone_from(&server.computer_name);
        let pump = Arc::new(pump::Shared::default());
        let wake = Arc::clone(&pump);
        match LiveSupervisor::start(config, &server.computer_label(), move || wake.notify()) {
            Ok(supervisor) => {
                let handle = Box::new(supervisor.handle());
                shared.supervisor = Some(supervisor);
                shared.start_pump(pump, handle, ctx);
            }
            Err(error) => {
                tracing::warn!(%error, "the live link of the shared terminals did not start");
                retire(shared.pump.take(), None);
            }
        }
    }
}
