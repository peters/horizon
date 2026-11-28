//! The part of the live link that carries the session mirror: the list that this
//! computer publishes, the sessions that it follows, and the entries that wait for
//! the link.
//!
//! A link takes part in the mirror only after it sent `mirror_open`, and it sends
//! that request only to a server that answered
//! [`MIRROR_PROTOCOL_VERSION`](crate::protocol::MIRROR_PROTOCOL_VERSION). The whole
//! state of the mirror is here, so the rest of the link is unchanged for a computer
//! that uses no mirror and for a server that relays none.
//!
//! The wanted state is sent again on each new link, as the panels and the follows of
//! a shared terminal are: the entries of a session wait for the link only while it
//! is open, and a new link asks for them again.

use std::collections::{BTreeSet, VecDeque};

use horizon_core::session_mirror::MirrorEntry;

use crate::live::mirror::{MirrorList, clean_entries, clean_list};
use crate::live::{LiveEvent, LiveRequest};

/// One session that a computer follows: the publisher, its program and the id of
/// the session.
type Key = (String, String, String);

/// The most frames of entries that wait for one link.
const MAX_PENDING: usize = 32;

/// The state of the mirror of one live link.
#[derive(Default)]
pub(super) struct MirrorState {
    /// Whether the application asked for the frames of the mirror.
    wanted: bool,
    /// Whether the open link sent `mirror_open`.
    opened: bool,
    /// The list that this computer publishes, and the one that the link has. A
    /// link sends no list before the application gave one: a viewer publishes
    /// nothing.
    list: MirrorList,
    list_set: bool,
    sent_list: Option<MirrorList>,
    /// The sessions that this computer follows, and the ones that the link has.
    follows: BTreeSet<Key>,
    sent_follows: BTreeSet<Key>,
    /// The entries of a followed session of this computer that wait for the link.
    entries: VecDeque<(String, String, Vec<MirrorEntry>, bool)>,
}

impl MirrorState {
    /// Takes part in the session mirror from the next frame.
    pub(super) fn want(&mut self) {
        self.wanted = true;
    }

    /// Replaces the list of the sessions that this computer publishes.
    pub(super) fn set_list(&mut self, list: MirrorList) {
        self.list = clean_list(list);
        self.list_set = true;
    }

    /// Starts the follow of one session of `owner`.
    pub(super) fn follow(&mut self, owner: &str, program: &str, session: &str) {
        self.follows.insert(key(owner, program, session));
    }

    /// Ends the follow of one session of `owner`.
    pub(super) fn unfollow(&mut self, owner: &str, program: &str, session: &str) {
        self.follows.remove(&key(owner, program, session));
    }

    /// Queues the entries of one of the followed sessions of this computer. `false`
    /// says that the link is away or that the frames that wait for it are full, so
    /// nothing was sent and the caller sends the tail again later.
    pub(super) fn entries(&mut self, program: &str, session: &str, entries: Vec<MirrorEntry>, tail: bool) -> bool {
        let entries = clean_entries(entries);
        if entries.is_empty() || !self.opened || self.entries.len() >= MAX_PENDING {
            return false;
        }
        self.entries
            .push_back((program.to_owned(), session.to_owned(), entries, tail));
        true
    }

    /// The frames of the mirror that the link sends now.
    pub(super) fn batch(&mut self, batch: &mut Vec<LiveRequest>) {
        if !self.wanted {
            return;
        }
        if !self.opened {
            self.opened = true;
            batch.push(LiveRequest::MirrorOpen);
        }
        if self.list_set && self.sent_list.as_ref() != Some(&self.list) {
            self.sent_list = Some(self.list.clone());
            batch.push(LiveRequest::MirrorSessions {
                sessions: self.list.sessions.clone(),
                unreadable: self.list.unreadable.clone(),
            });
        }
        let left: Vec<Key> = self.sent_follows.difference(&self.follows).cloned().collect();
        for (owner, program, session) in left {
            self.sent_follows
                .remove(&(owner.clone(), program.clone(), session.clone()));
            batch.push(LiveRequest::MirrorUnfollow {
                owner,
                program,
                session,
            });
        }
        let new: Vec<Key> = self.follows.difference(&self.sent_follows).cloned().collect();
        for (owner, program, session) in new {
            self.sent_follows
                .insert((owner.clone(), program.clone(), session.clone()));
            batch.push(LiveRequest::MirrorFollow {
                owner,
                program,
                session,
            });
        }
        while let Some((program, session, entries, tail)) = self.entries.pop_front() {
            batch.push(LiveRequest::MirrorEntries {
                program,
                session,
                entries,
                tail,
            });
        }
    }

    /// Takes one event of the server. It answers `false` for an event that only the
    /// mirror reads.
    pub(super) fn accept(&mut self, event: &mut LiveEvent) -> bool {
        match event {
            LiveEvent::MirrorWatched {
                program,
                session,
                watched,
            } => {
                if !*watched {
                    // No computer follows this session, so nothing of it waits for
                    // the link any more.
                    self.entries.retain(|(of, id, ..)| of != program || id != session);
                }
                true
            }
            LiveEvent::MirrorEnded {
                owner,
                program,
                session,
                ..
            } => {
                let left = key(owner, program, session);
                self.follows.remove(&left);
                self.sent_follows.remove(&left);
                true
            }
            _ => true,
        }
    }

    /// A server of an older version reads no frame of the mirror.
    pub(super) fn want_not(&mut self) {
        self.wanted = false;
    }

    /// A new link has nothing of the last one.
    pub(super) fn reset(&mut self) {
        self.opened = false;
        self.sent_list = None;
        self.sent_follows.clear();
        self.entries.clear();
    }
}

/// The key of one followed session.
fn key(owner: &str, program: &str, session: &str) -> Key {
    (owner.to_owned(), program.to_owned(), session.to_owned())
}
