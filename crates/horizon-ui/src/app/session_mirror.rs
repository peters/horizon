//! The sessions of the other computers, as the session mirror shows them: the
//! list that each publishing computer sends, the entries of the session that this
//! window follows, and the choice that a row of the sidebar makes.
//!
//! The frames of the mirror come on the live link of the shared terminals, so they
//! are read on the thread of the pump ([`super::shared_terminals`]) and a frame
//! reads a snapshot. Nothing here starts a process, opens a file or writes one: a
//! mirrored session is not stored, and a restart shows the list again when a
//! publisher sends it.
//!
//! `docs/architecture/session-mirror.md` gives the design.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use horizon_core::agent_bridge::{Entry, ToolState};
use horizon_sync::{LiveEvent, MirrorComputer, MirrorEntry, MirrorProgram};

/// The most entries of one followed session that this window keeps.
const MAX_KEPT_ENTRIES: usize = 200;

/// The identity of one session of one publishing computer.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(in crate::app) struct MirrorKey {
    pub(in crate::app) device: String,
    pub(in crate::app) program: MirrorProgram,
    pub(in crate::app) session: String,
}

impl MirrorKey {
    /// The identity of one row of the list of `computer`.
    fn of(device: &str, program: MirrorProgram, session: &str) -> Self {
        Self {
            device: device.to_owned(),
            program,
            session: session.to_owned(),
        }
    }

    /// The text that the place of the list of this session uses, so the scroll of
    /// one session never shows in another.
    pub(in crate::app) fn scroll_key(&self) -> String {
        format!("mirror:{}:{}:{}", self.device, self.program.key(), self.session)
    }
}

/// What this window knows of the mirror, as the pump gives it to a frame.
#[derive(Clone, Default)]
pub(in crate::app) struct MirrorView {
    /// The publishing computers and their sessions.
    pub(in crate::app) computers: Vec<MirrorComputer>,
    entries: BTreeMap<MirrorKey, Vec<Entry>>,
}

impl MirrorView {
    /// The session of one key, as a row of a list shows it.
    #[must_use]
    pub(in crate::app) fn session(&self, key: &MirrorKey) -> Option<&horizon_sync::MirrorSession> {
        self.computers
            .iter()
            .find(|computer| computer.device == key.device)
            .and_then(|computer| {
                computer
                    .sessions
                    .iter()
                    .find(|session| session.program == key.program && session.session == key.session)
            })
    }

    /// The name of the computer of one key, as its label or as its device name.
    #[must_use]
    pub(in crate::app) fn computer(&self, device: &str) -> String {
        self.computers
            .iter()
            .find(|computer| computer.device == device)
            .map_or_else(
                || device.to_owned(),
                |computer| {
                    if computer.label.is_empty() {
                        computer.device.clone()
                    } else {
                        computer.label.clone()
                    }
                },
            )
    }

    /// The rows of the session of `key`, in the form of a conversation of the dock.
    #[must_use]
    pub(in crate::app) fn entries(&self, key: &MirrorKey) -> &[Entry] {
        self.entries.get(key).map_or(&[], Vec::as_slice)
    }

    /// Whether the mirror shows nothing at all.
    #[must_use]
    pub(in crate::app) fn is_empty(&self) -> bool {
        self.computers.is_empty()
    }
}

/// What the pump accumulates from the frames of the mirror.
#[derive(Default)]
pub(in crate::app) struct Mirror {
    view: MirrorView,
    changed: bool,
}

impl Mirror {
    /// Whether the newest list names the session of `key`.
    pub(in crate::app) fn lists(&self, key: &MirrorKey) -> bool {
        self.view.session(key).is_some()
    }

    /// Takes one event of the link. An event that only the mirror reads answers
    /// `false`, so the pump does not hand it to a part of the shared terminals.
    pub(in crate::app) fn apply(&mut self, event: &LiveEvent) -> bool {
        match event {
            LiveEvent::MirrorComputers { computers } => {
                if self.view.computers != *computers {
                    self.view.computers.clone_from(computers);
                    self.changed = true;
                }
                false
            }
            LiveEvent::MirrorEntries {
                owner,
                program,
                session,
                entries,
                tail,
            } => {
                let Some(program) = MirrorProgram::from_key(program) else {
                    return false;
                };
                let key = MirrorKey::of(owner, program, session);
                let held = self.view.entries.entry(key).or_default();
                if *tail {
                    held.clear();
                }
                held.extend(entries.iter().map(row_of));
                let over = held.len().saturating_sub(MAX_KEPT_ENTRIES);
                if over > 0 {
                    held.drain(..over);
                }
                self.changed = true;
                false
            }
            LiveEvent::MirrorEnded {
                owner,
                program,
                session,
                ..
            } => {
                // The follow ended, and a restart of the publisher sends the tail
                // again: this window keeps the rows that it read and waits for no
                // more of them.
                if let Some(program) = MirrorProgram::from_key(program) {
                    self.view.entries.remove(&MirrorKey::of(owner, program, session));
                    self.changed = true;
                }
                false
            }
            LiveEvent::MirrorWatched { .. } => false,
            _ => true,
        }
    }

    /// What the frame reads, when it changed.
    pub(in crate::app) fn snapshot(&mut self) -> Option<Arc<MirrorView>> {
        if !self.changed {
            return None;
        }
        self.changed = false;
        Some(Arc::new(self.view.clone()))
    }

    /// A link that ended holds no list of the mirror: the next link sends it again.
    pub(in crate::app) fn clear(&mut self) {
        (self.view, self.changed) = (MirrorView::default(), true);
    }
}

/// One row of a mirrored session, in the form of an entry of the dock.
fn row_of(entry: &MirrorEntry) -> Entry {
    match entry {
        MirrorEntry::Person { text } => Entry::Person {
            text: text.clone(),
            via_front_desk: false,
        },
        MirrorEntry::Agent { text } => Entry::Agent {
            message_id: String::new(),
            text: text.clone(),
        },
        MirrorEntry::Tool { name, request } => Entry::Tool {
            tool_id: String::new(),
            name: name.clone(),
            summary: request.clone(),
            result: None,
            state: ToolState::Running,
        },
        MirrorEntry::Notice { text } => Entry::Notice { text: text.clone() },
    }
}

/// What this window wants of the mirror, which the pump sends to the link.
#[derive(Clone, Default, PartialEq, Eq)]
pub(in crate::app) struct MirrorWanted {
    /// The session that the dock shows, which this window follows while it shows
    /// it and ends when it shows another thing.
    pub(in crate::app) follow: Option<MirrorKey>,
}

/// The choice of this window in the group `COMPUTERS`: a press on a row, and the
/// session that the dock shows.
#[derive(Default)]
pub(in crate::app) struct SessionMirror {
    /// The computers whose rows show every session rather than the first eight.
    expanded: HashSet<String>,
}

impl SessionMirror {
    /// Whether the rows of `device` show every session.
    pub(in crate::app) fn is_expanded(&self, device: &str) -> bool {
        self.expanded.contains(device)
    }

    /// Shows every session of `device`, which a press on `and <N> more` does.
    pub(in crate::app) fn expand(&mut self, device: &str) {
        self.expanded.insert(device.to_owned());
    }

    /// Forgets the choice of a computer that left the list.
    pub(in crate::app) fn retain(&mut self, computers: &[MirrorComputer]) {
        self.expanded
            .retain(|device| computers.iter().any(|computer| &computer.device == device));
    }
}

#[cfg(test)]
mod tests;
