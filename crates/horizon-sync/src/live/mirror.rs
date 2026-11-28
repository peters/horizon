//! The frames of the session mirror: the list of the sessions of one publishing
//! computer, the follow of one of them, and the entries of a followed session.
//!
//! The frames travel on the live link, beside the frames of the shared terminals,
//! and the server relays them without writing one to its store or to its log. Every
//! text comes from a session store that another program wrote, so the server cuts
//! each one at its bound before it keeps or relays it, as it does for a panel.
//!
//! A computer joins the mirror with `mirror_open`. The server sends the frames of
//! the mirror only to a link that sent that request, so a computer of an older
//! build, which reads no mirror frame and sends none, keeps its live link and its
//! shared terminals. A computer sends `mirror_open` only to a server that answered
//! [`MIRROR_PROTOCOL_VERSION`](crate::protocol::MIRROR_PROTOCOL_VERSION).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use horizon_core::session_mirror::{
    MAX_ENTRIES, MAX_LIST_SESSIONS, MAX_SESSION_ID_CHARS, MAX_SUMMARY_CHARS, MAX_TEXT_CHARS, MirrorEntry,
    MirrorProgram, MirrorSession,
};

/// The most sessions that one publisher lists.
pub const MAX_MIRROR_SESSIONS: usize = MAX_LIST_SESSIONS;
/// The most entries of one session in one frame.
pub const MAX_MIRROR_ENTRIES: usize = MAX_ENTRIES;
/// The most sessions of other computers that one computer follows.
pub const MAX_MIRROR_FOLLOWS: usize = 8;
/// The most bytes of the entries in one frame. The ceiling of the link is
/// [`crate::MAX_FRAME_BYTES`], and the entries leave room for the rest of the
/// document.
pub const MIRROR_FRAME_BYTES: usize = 192 * 1024;
/// The most characters of the title of a session, of its folder and of its parent.
pub const TITLE_CHARS: usize = 160;
pub const FOLDER_CHARS: usize = 48;
/// The most characters of the name of a tool.
pub const TOOL_CHARS: usize = 120;

/// The list of the sessions of one publishing computer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorList {
    /// The sessions, newest first.
    pub sessions: Vec<MirrorSession>,
    /// The programs whose store this publisher cannot read.
    pub unreadable: Vec<MirrorProgram>,
}

/// One publishing computer and its list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorComputer {
    pub device: String,
    pub label: String,
    /// The sessions that this computer publishes.
    pub sessions: Vec<MirrorSession>,
    /// The programs that this computer names with no session.
    pub unreadable: Vec<MirrorProgram>,
    /// Whether this is the computer that reads the list. The handle of a computer
    /// sets it, and it does not travel.
    #[serde(skip)]
    pub own: bool,
}

/// The one list of a link, cut at its bounds.
#[must_use]
pub fn clean_list(list: MirrorList) -> MirrorList {
    let mut kept: Vec<MirrorSession> = Vec::new();
    for session in list.sessions.into_iter().take(MAX_MIRROR_SESSIONS) {
        let Some(clean) = clean_session(&session) else {
            continue;
        };
        if kept
            .iter()
            .any(|held| held.program == clean.program && held.session == clean.session)
        {
            continue;
        }
        kept.push(clean);
    }
    let mut programs = BTreeSet::new();
    let unreadable = list
        .unreadable
        .into_iter()
        .filter(|program| programs.insert(*program))
        .collect();
    MirrorList {
        sessions: kept,
        unreadable,
    }
}

/// One session, with each text cut at its bound. `None` for a session whose id this
/// build cannot show.
fn clean_session(session: &MirrorSession) -> Option<MirrorSession> {
    let id = clean(&session.session, MAX_SESSION_ID_CHARS);
    if id.is_empty() {
        return None;
    }
    Some(MirrorSession {
        program: session.program,
        session: id,
        title: clean(&session.title, TITLE_CHARS),
        folder: clean(&session.folder, FOLDER_CHARS),
        parent: clean(&session.parent, FOLDER_CHARS),
        updated_at: session.updated_at,
        live: session.live,
    })
}

/// The entries of one session, cut at their bounds and to one frame.
#[must_use]
pub fn clean_entries(entries: Vec<MirrorEntry>) -> Vec<MirrorEntry> {
    let mut kept = Vec::new();
    let mut bytes = 0usize;
    for entry in entries.into_iter().take(MAX_MIRROR_ENTRIES) {
        let Some(entry) = clean_entry(entry) else {
            continue;
        };
        let size = serde_json::to_vec(&entry).map_or(MIRROR_FRAME_BYTES, |json| json.len() + 1);
        if bytes + size > MIRROR_FRAME_BYTES {
            break;
        }
        bytes += size;
        kept.push(entry);
    }
    kept
}

/// One entry, with each text cut at its bound. `None` for an entry that holds no
/// text.
fn clean_entry(entry: MirrorEntry) -> Option<MirrorEntry> {
    match entry {
        MirrorEntry::Person { text } => Some(MirrorEntry::Person {
            text: clean(&text, MAX_TEXT_CHARS),
        }),
        MirrorEntry::Agent { text } => Some(MirrorEntry::Agent {
            text: clean(&text, MAX_TEXT_CHARS),
        }),
        MirrorEntry::Tool { name, request } => Some(MirrorEntry::Tool {
            name: clean(&name, TOOL_CHARS),
            request: clean(&request, MAX_SUMMARY_CHARS),
        }),
        MirrorEntry::Notice { text } => Some(MirrorEntry::Notice {
            text: clean(&text, MAX_SUMMARY_CHARS),
        }),
    }
    .filter(|entry| !is_blank(entry))
}

/// Whether one entry holds no text at all.
fn is_blank(entry: &MirrorEntry) -> bool {
    match entry {
        MirrorEntry::Person { text } | MirrorEntry::Agent { text } | MirrorEntry::Notice { text } => text.is_empty(),
        MirrorEntry::Tool { name, request } => name.is_empty() && request.is_empty(),
    }
}

/// `text` without its control characters, cut at `max` characters.
#[must_use]
pub fn clean(text: &str, max: usize) -> String {
    super::protocol::clean(text, max)
}
