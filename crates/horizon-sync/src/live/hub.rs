//! The hub of the live links of one server: which computer shares which panels,
//! who follows them, and where each frame of a computer goes.
//!
//! The hub is memory only and lives in the registry of the links, so every link
//! of a server shares it. No call does I/O or waits for a link: a frame goes into
//! the outbox of its link, and a link that must end ends on a thread of its own.
//! Everything of a computer is cut at its bound before the hub keeps it, and a
//! log line names devices and counts, never a title or a byte of a panel.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
    thread,
    time::{Duration, Instant},
};

use super::outbox::{Outbox, Push, Tag};
use super::protocol::{
    DEVICE_CHARS, ID_CHARS, LABEL_CHARS, LiveBytes, LiveComputer, LiveEndReason, LiveEvent, LivePanel, LiveRequest,
    LiveWatcher, MAX_GRID, MAX_LIVE_FOLLOWS, MAX_LIVE_OUTPUT_BYTES, MAX_LIVE_TYPED_BYTES, MAX_LIVE_WATCHERS, clean,
    clean_panels,
};
use crate::protocol::LinkErrorCode;
use crate::server::LinkTicket;

/// The most live links of one server, one for each computer.
pub(crate) const MAX_LIVE_LINKS: usize = 32;
/// The most bytes of all panels in one list of the computers, so that the list
/// stays one frame. Each computer has an equal part, and the frame cuts a longer
/// list of panels.
const LIST_BYTES: usize = 192 * 1024;
/// How long one follower waits before the owner of one panel is asked again
/// for a new screen for it.
pub(crate) const REFRESH_EVERY: Duration = Duration::from_secs(1);

struct Link {
    id: u64,
    label: String,
    panels: Vec<LivePanel>,
    outbox: Arc<Outbox>,
    ticket: Arc<LinkTicket>,
}

/// How the output of a panel reaches one follower.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Relay {
    /// Nothing is relayed until the next screen of the owner.
    Waiting,
    Relaying,
    /// The follower fell behind: nothing is relayed until its link is ready.
    Behind,
}

struct Follow {
    follower: String,
    owner: String,
    panel: String,
    relay: Relay,
    /// When this follower last asked the owner for a new screen.
    refreshed: Option<Instant>,
}

#[derive(Default)]
struct State {
    /// The live link of each device.
    links: BTreeMap<String, Link>,
    follows: Vec<Follow>,
    /// The links that must end, which end after the lock is free.
    ending: Vec<(Arc<LinkTicket>, LinkErrorCode, &'static str)>,
}

/// The live links of one server.
#[derive(Default)]
pub(crate) struct LiveHub {
    state: Mutex<State>,
}

impl LiveHub {
    /// Runs `work` on the state, and then ends the links that it named. The end
    /// of a link waits for its error frame, so it has a thread of its own.
    fn with<T>(&self, work: impl FnOnce(&mut State) -> T) -> T {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let answer = work(&mut state);
        let ending = std::mem::take(&mut state.ending);
        drop(state);
        for (ticket, code, message) in ending {
            let ended = Arc::clone(&ticket);
            if thread::Builder::new().spawn(move || ended.end(code, message)).is_err() {
                ticket.shut();
            }
        }
        answer
    }

    /// Opens the live link of `ticket`, and ends an older live link of the same
    /// device. Answers `false` when the server holds the most live links.
    pub(crate) fn open(&self, ticket: &Arc<LinkTicket>, label: &str, outbox: &Arc<Outbox>) -> bool {
        let device = ticket.device.as_str();
        self.with(|state| {
            if let Some(older) = state.drop_link(device) {
                let message = "Another live link of this computer opened.";
                state.ending.push((older.ticket, LinkErrorCode::LiveReplaced, message));
            } else if state.links.len() >= MAX_LIVE_LINKS {
                return false;
            }
            let link = Link {
                id: ticket.id,
                label: clean(label, LABEL_CHARS),
                panels: Vec::new(),
                outbox: Arc::clone(outbox),
                ticket: Arc::clone(ticket),
            };
            state.links.insert(device.to_owned(), link);
            tracing::info!(device, links = state.links.len(), "a live link opened");
            let (name, computers) = (device.to_owned(), state.computers());
            state.send(
                device,
                LiveEvent::Opened {
                    device: name,
                    computers,
                },
                Tag::Kept,
            );
            state.list_changed(Some(device));
            true
        })
    }

    /// Obeys one request of the live link `id` of `device`.
    pub(crate) fn request(&self, device: &str, id: u64, request: LiveRequest) {
        self.with(|state| {
            if state.links.get(device).is_some_and(|link| link.id == id) {
                state.request(device, request);
            }
        });
    }

    /// The live link `id` of `device` is ready again after it fell behind: each
    /// owner whose output it lost is asked for a new screen.
    pub(crate) fn resume(&self, device: &str, id: u64) {
        self.with(|state| {
            let mut ready = Vec::new();
            let behind = |follow: &&mut Follow| follow.follower == device && follow.relay == Relay::Behind;
            let open = state.links.get(device).is_some_and(|link| link.id == id);
            for follow in state.follows.iter_mut().filter(behind).filter(|_| open) {
                follow.relay = Relay::Waiting;
                ready.push((follow.owner.clone(), follow.panel.clone()));
            }
            for (owner, panel) in ready {
                state.watchers(&owner, &panel, true);
            }
        });
    }

    /// Forgets the live link `id` of `device`, which ended.
    pub(crate) fn close(&self, device: &str, id: u64) {
        self.with(|state| {
            if state.links.get(device).is_some_and(|link| link.id == id) {
                state.drop_link(device);
                tracing::info!(device, links = state.links.len(), "a live link ended");
                state.list_changed(None);
            }
        });
    }
}

impl State {
    /// Queues one kept frame for `device`. A link that does not read its kept
    /// frames ends.
    fn send(&mut self, device: &str, event: LiveEvent, tag: Tag) {
        let Some(link) = self.links.get(device) else {
            return;
        };
        match link.outbox.push(event, tag) {
            Push::Queued => {}
            Push::Evicted => {
                let relaying = |follow: &&mut Follow| follow.follower == device && follow.relay == Relay::Relaying;
                for follow in self.follows.iter_mut().filter(relaying) {
                    follow.relay = Relay::Behind;
                }
            }
            Push::Full => {
                tracing::warn!(
                    device,
                    "a live link did not read the frames that waited for it, so it ends"
                );
                let message = "This computer did not read its live link.";
                self.ending
                    .push((Arc::clone(&link.ticket), LinkErrorCode::LiveLimit, message));
            }
        }
    }

    /// The computers that have a live link, each with the panels that fit in its
    /// part of the list.
    fn computers(&self) -> Vec<LiveComputer> {
        let part = LIST_BYTES / self.links.len().max(1);
        let computer = |(device, link): (&String, &Link)| {
            let mut bytes = 0;
            let fits = |panel: &&LivePanel| {
                bytes += serde_json::to_vec(panel).map_or(part, |json| json.len() + 1);
                bytes <= part
            };
            LiveComputer {
                device: device.clone(),
                label: link.label.clone(),
                panels: link.panels.iter().take_while(fits).cloned().collect(),
                own: false,
            }
        };
        self.links.iter().map(computer).collect()
    }

    /// Sends the list of the computers to every live link but `except`.
    fn list_changed(&mut self, except: Option<&str>) {
        let computers = self.computers();
        let devices: Vec<String> = self.links.keys().cloned().collect();
        for device in devices.iter().filter(|device| Some(device.as_str()) != except) {
            let computers = computers.clone();
            self.send(device, LiveEvent::Computers { computers }, Tag::List);
        }
    }

    fn label(&self, device: &str) -> String {
        self.links
            .get(device)
            .map(|link| link.label.clone())
            .unwrap_or_default()
    }

    fn followers<'a>(&'a self, owner: &'a str, panel: &'a str) -> impl Iterator<Item = &'a Follow> {
        let follows = move |follow: &&Follow| follow.owner == owner && follow.panel == panel;
        self.follows.iter().filter(follows)
    }

    /// Tells `owner` who follows its panel now.
    fn watchers(&mut self, owner: &str, panel: &str, refresh: bool) {
        let watcher = |follow: &Follow| LiveWatcher {
            device: follow.follower.clone(),
            label: self.label(&follow.follower),
        };
        let watchers = self.followers(owner, panel).map(watcher).collect();
        let panel = panel.to_owned();
        self.send(
            owner,
            LiveEvent::Watchers {
                panel,
                watchers,
                refresh,
            },
            Tag::Kept,
        );
    }

    /// Ends the follows that `ended` names: each follower reads the reason, and
    /// the output that still waits for it leaves.
    fn end_follows(&mut self, ended: impl Fn(&Follow) -> bool, reason: LiveEndReason) {
        let over: Vec<Follow> = self.follows.extract_if(.., |follow| ended(follow)).collect();
        for Follow {
            follower, owner, panel, ..
        } in over
        {
            if let Some(link) = self.links.get(&follower) {
                link.outbox.drop_stream(&owner, &panel);
            }
            self.send(&follower, LiveEvent::Ended { owner, panel, reason }, Tag::Kept);
        }
    }

    /// Removes the live link of `device`: its panels leave the list, its
    /// followers read that it is offline, and the owners that it followed read
    /// who follows them now. The caller sends the list.
    fn drop_link(&mut self, device: &str) -> Option<Link> {
        let link = self.links.remove(device)?;
        link.outbox.close();
        self.end_follows(|follow| follow.owner == device, LiveEndReason::Offline);
        let left = |follow: &mut Follow| follow.follower == device;
        for follow in self.follows.extract_if(.., left).collect::<Vec<Follow>>() {
            self.watchers(&follow.owner, &follow.panel, false);
        }
        Some(link)
    }

    /// Replaces the list of `device`. The followers of a panel that left read
    /// that it closed, and every link reads the list when it changed.
    fn set_panels(&mut self, device: &str, panels: Vec<LivePanel>) {
        let Some(link) = self.links.get_mut(device).filter(|link| link.panels != panels) else {
            return;
        };
        let listed: Vec<String> = panels.iter().map(|panel| panel.id.clone()).collect();
        link.panels = panels;
        let left = |follow: &Follow| follow.owner == device && !listed.contains(&follow.panel);
        self.end_follows(left, LiveEndReason::Closed);
        self.list_changed(None);
    }

    fn request(&mut self, device: &str, request: LiveRequest) {
        let owner = device.to_owned();
        match request {
            LiveRequest::Panels { panels } => self.set_panels(device, clean_panels(panels)),
            LiveRequest::Ended { panel } => {
                let mut panels = self.links.get(device).map_or_else(Vec::new, |link| link.panels.clone());
                panels.retain(|listed| listed.id != panel);
                self.set_panels(device, panels);
            }
            LiveRequest::Follow { owner, panel } => {
                self.follow(device, clean(&owner, DEVICE_CHARS), clean(&panel, ID_CHARS));
            }
            LiveRequest::Unfollow { owner, panel } => {
                let left =
                    |follow: &Follow| follow.follower == device && follow.owner == owner && follow.panel == panel;
                if self.follows.extract_if(.., |follow| left(follow)).count() > 0 {
                    if let Some(link) = self.links.get(device) {
                        link.outbox.drop_stream(&owner, &panel);
                    }
                    self.watchers(&owner, &panel, false);
                }
            }
            LiveRequest::Refresh { owner, panel } => self.refresh(device, &owner, &panel),
            LiveRequest::Screen { panel, cols, rows } => {
                let (cols, rows) = (cols.clamp(1, MAX_GRID), rows.clamp(1, MAX_GRID));
                self.stream(&LiveEvent::Screen {
                    owner,
                    panel,
                    cols,
                    rows,
                });
            }
            LiveRequest::Output { panel, data } if (1..=MAX_LIVE_OUTPUT_BYTES).contains(&data.0.len()) => {
                self.stream(&LiveEvent::Output { owner, panel, data });
            }
            LiveRequest::Typed { owner, panel, data } if (1..=MAX_LIVE_TYPED_BYTES).contains(&data.0.len()) => {
                self.typed(device, &owner, panel, data);
            }
            // A second `open` changes nothing, and a frame over its bound is
            // dropped, not obeyed.
            LiveRequest::Open { .. } | LiveRequest::Output { .. } | LiveRequest::Typed { .. } => {}
        }
    }

    /// Starts the follow of one panel, or answers why it does not start.
    fn follow(&mut self, device: &str, owner: String, panel: String) {
        let shares = |link: &Link| link.panels.iter().any(|listed| listed.id == panel);
        let listed = owner != device && self.links.get(&owner).is_some_and(shares);
        let mine = |follow: &&Follow| follow.follower == device;
        let refused = if !listed {
            Some(LiveEndReason::NotFound)
        } else if self.followers(&owner, &panel).any(|follow| mine(&follow)) {
            // A follow that is asked again asks the owner for a new screen.
            return self.refresh(device, &owner, &panel);
        } else if self.follows.iter().filter(mine).count() >= MAX_LIVE_FOLLOWS
            || self.followers(&owner, &panel).count() >= MAX_LIVE_WATCHERS
        {
            Some(LiveEndReason::Limit)
        } else {
            self.follows.push(Follow {
                follower: device.to_owned(),
                owner: owner.clone(),
                panel: panel.clone(),
                relay: Relay::Waiting,
                refreshed: None,
            });
            None
        };
        match refused {
            Some(reason) => self.send(device, LiveEvent::Ended { owner, panel, reason }, Tag::Kept),
            None => self.watchers(&owner, &panel, true),
        }
    }

    /// Asks the owner for a new screen for one follower of its panel, at most
    /// once in [`REFRESH_EVERY`]. A request that comes sooner, and one of a
    /// computer that does not follow the panel, is dropped.
    fn refresh(&mut self, device: &str, owner: &str, panel: &str) {
        let now = Instant::now();
        let due = |follow: &&mut Follow| {
            let asked = follow
                .refreshed
                .is_some_and(|at| now.duration_since(at) < REFRESH_EVERY);
            follow.follower == device && follow.owner == owner && follow.panel == panel && !asked
        };
        if let Some(follow) = self.follows.iter_mut().find(due) {
            follow.refreshed = Some(now);
            self.watchers(owner, panel, true);
        }
    }

    /// Relays a screen or output of an owner to the followers of that panel. A
    /// follower takes output only after a screen, and a follower whose link
    /// would pass its ceiling falls behind.
    fn stream(&mut self, event: &LiveEvent) {
        let (LiveEvent::Screen { owner, panel, .. } | LiveEvent::Output { owner, panel, .. }) = event else {
            return;
        };
        let starts = matches!(event, LiveEvent::Screen { .. });
        for follow in &mut self.follows {
            let takes = follow.relay == Relay::Relaying || (starts && follow.relay == Relay::Waiting);
            if follow.owner != *owner || follow.panel != *panel || !takes {
                continue;
            }
            let Some(link) = self.links.get(&follow.follower) else {
                continue;
            };
            let tag = Tag::Stream(owner.clone(), panel.clone());
            follow.relay = match link.outbox.push(event.clone(), tag) {
                Push::Full => Relay::Behind,
                Push::Queued | Push::Evicted => Relay::Relaying,
            };
        }
    }

    /// Passes what a follower typed to the owner, when the owner lets it type.
    fn typed(&mut self, device: &str, owner: &str, panel: String, data: LiveBytes) {
        let typing = |link: &Link| link.panels.iter().any(|listed| listed.id == panel && listed.typing);
        let follows = self.followers(owner, &panel).any(|follow| follow.follower == device);
        if follows && self.links.get(owner).is_some_and(typing) {
            let (from, label) = (device.to_owned(), self.label(device));
            self.send(
                owner,
                LiveEvent::Typed {
                    panel,
                    from,
                    label,
                    data,
                },
                Tag::Kept,
            );
        }
    }
}
