//! Shared terminals: the terminal panels of this computer that the other
//! computers of the person see and type in, and the panels of those computers
//! that this window follows.
//!
//! The live link of `horizon-sync` carries both. It starts and ends with the
//! sync link of this computer. It starts again when the sync link comes back
//! after it was away, because the live link has no keepalive of its own: a list
//! then never outlives its link by more than the sync link does. A new name of
//! this computer starts it again too. Every call of the link, of a feed and of
//! an outlet returns at once, so the frame does the whole join. A link that is
//! down or that the server does not support shares nothing and fails nothing.

mod follower;
mod link;
mod owner;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::time::Instant;

use egui::Context;
use horizon_core::{Config, PanelId, ServerConfig};
use horizon_sync::{LiveEvent, LiveState, LiveSupervisor, SupervisorConfig};

use super::HorizonApp;
use crate::block_widget::{CardChip, chip::ChipState};
pub(in crate::app) use follower::{attention_of, computer_name, kind_of};
pub(in crate::app) use link::LiveLink;

/// The most characters of a text of another computer that this window shows.
const SHOWN_CHARS: usize = 80;

/// The state of the join in this window.
#[derive(Default)]
pub(in crate::app) struct SharedTerminals {
    /// The live link of this computer, and the settings that start it again.
    supervisor: Option<LiveSupervisor>,
    config: Option<SupervisorConfig>,
    /// What the join talks to: the handle of `supervisor`, or the stand-in of a test.
    link: Option<Box<dyn LiveLink>>,
    /// The name setting that the link started with.
    name: String,
    /// Whether the sync link was away on the last frame.
    sync_away: bool,
    /// Why the live link is down, for the sidebar. It stays until the link opens.
    pub(in crate::app) down: Option<String>,
    /// The chip text of each panel that is watched or that follows another.
    chips: HashMap<PanelId, String>,
    owner: owner::Owner,
    pub(in crate::app) follower: follower::Follower,
}

impl SharedTerminals {
    /// The chip in the title bar of `panel`, when it is watched or follows.
    pub(in crate::app) fn chip(&self, panel: PanelId) -> Option<CardChip<'_>> {
        let name = self.chips.get(&panel)?;
        Some(CardChip {
            name,
            state: ChipState::Present,
        })
    }

    /// Terminal output arrived on this frame, so the list is built again.
    pub(in crate::app) fn note_output(&mut self) {
        self.owner.dirty = true;
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

/// Stops a live link on a thread of its own, because the stop joins a thread.
fn retire(supervisor: LiveSupervisor) {
    let stop = std::thread::Builder::new().name("live-link-stop".to_owned());
    let _ = stop.spawn(move || supervisor.stop());
}

impl HorizonApp {
    /// Runs the join for one frame: the events of the link, then the list and
    /// the output of this computer, then the follow panels of this window.
    pub(in crate::app) fn poll_shared_terminals(&mut self, ctx: &Context) {
        self.follow_sync_link(ctx);
        let Some(link) = self.shared_terminals.link.take() else {
            return;
        };
        let now = Instant::now();
        for event in link.take_events() {
            match event {
                LiveEvent::State(LiveState::Open) => self.shared_terminals.down = None,
                LiveEvent::State(LiveState::Connecting) => {}
                LiveEvent::State(LiveState::Unsupported) => self.shared_link_lost(None),
                LiveEvent::State(LiveState::Down(reason)) => self.shared_link_lost(Some(shown(&reason))),
                LiveEvent::Computers { computers } => self.shared_computers(link.as_ref(), computers),
                LiveEvent::Watchers {
                    panel,
                    watchers,
                    refresh,
                } => self.shared_watchers(&panel, &watchers, refresh, now),
                LiveEvent::Typed { panel, data, .. } => self.shared_typed(&panel, &data.0),
                stream => self.follow_event(stream),
            }
        }
        self.share_panels(link.as_ref(), ctx, now);
        self.pump_follows(link.as_ref(), ctx, now);
        self.shared_terminals.link = Some(link);
    }

    /// Ends the live link, as the end of the sync link of this computer does.
    pub(in crate::app) fn stop_shared_terminals(&mut self) {
        self.shared_link_lost(None);
        let shared = &mut self.shared_terminals;
        if let Some(supervisor) = shared.supervisor.take() {
            retire(supervisor);
            shared.link = None;
        }
        shared.config = None;
    }

    /// Forgets what only an open link holds: who watches the panels of this
    /// computer, the other computers, and the streams that this window follows.
    fn shared_link_lost(&mut self, reason: Option<String>) {
        self.shared_terminals.down = reason;
        self.unwatch_all();
        self.follows_lost();
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
        let stored = stored_server(
            self.settings.as_ref().map(|editor| &editor.original),
            &self.template_config,
        );
        let shared = &mut self.shared_terminals;
        let away = sync.snapshot().state().code() == "away";
        let returned = std::mem::replace(&mut shared.sync_away, away) && !away;
        let renamed = link_renamed(shared, stored);
        match sync.live.take() {
            Some(config) => shared.config = Some(config),
            None if shared.supervisor.is_some() && (returned || renamed) => {}
            None => return,
        }
        let Some(config) = shared.config.clone() else {
            return;
        };
        // The new link has nothing of the old one, so the list and the follows go again.
        self.shared_link_lost(None);
        let server = stored_server(
            self.settings.as_ref().map(|editor| &editor.original),
            &self.template_config,
        );
        let shared = &mut self.shared_terminals;
        shared.owner = owner::Owner::default();
        shared.name.clone_from(&server.computer_name);
        if let Some(old) = shared.supervisor.take() {
            retire(old);
        }
        let wake = {
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        };
        shared.link = match LiveSupervisor::start(config, &server.computer_label(), wake) {
            Ok(supervisor) => {
                let handle: Box<dyn LiveLink> = Box::new(supervisor.handle());
                shared.supervisor = Some(supervisor);
                Some(handle)
            }
            Err(error) => {
                tracing::warn!(%error, "the live link of the shared terminals did not start");
                None
            }
        };
    }
}
