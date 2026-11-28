//! The sessions that this computer mirrors to the other computers of a Horizon
//! server: the list of the sessions of its agent programs, and the entries of one
//! session.
//!
//! The reader is a pure read: it opens no socket, writes no file and starts no
//! process. [`crate::runtime_state`] is the one reader of the session stores of
//! Claude Code and Codex, and of the live registry of Claude Code, so this module
//! builds on it instead of reading a store a second way. The stores of Pi are read
//! by the reader of the session picker of a panel.
//!
//! Every value here comes from a store that another program wrote, so every text is
//! cut and cleaned by [`one_line`](crate::session_pickup::one_line), and every read
//! has a bound. `docs/review/trust-boundaries/session-mirror.md` states what this
//! module guarantees.
//!
//! One entry is one of four things, which are the rows that the dock draws: the
//! text of the person, the text of the agent, the name of a tool with the first
//! line of its request, and one line of Horizon that says that the rest of a store
//! could not be read. No result of a tool, no thinking text, no system line and no
//! attachment content becomes an entry.

mod claude;
mod codex;
mod file;
mod pi;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::runtime_state::{AgentSessionRecord, live_claude_session_ids_in, load_sessions_of_home};
use crate::session_pickup::{MAX_TITLE_CHARS, folder_name_and_parent, one_line};

/// The most sessions of one list.
pub const MAX_LIST_SESSIONS: usize = 50;
/// The most entries of one session that the publisher sends.
pub const MAX_ENTRIES: usize = 60;
/// The most characters of one text of the person or of the agent.
pub const MAX_TEXT_CHARS: usize = 4_000;
/// The most characters of the first line of a tool request.
pub const MAX_SUMMARY_CHARS: usize = 200;
/// The bytes that the reader takes from the end of one session file.
pub const MAX_TAIL_BYTES: u64 = 1024 * 1024;
/// The most characters of the id of a session.
pub const MAX_SESSION_ID_CHARS: usize = 64;
/// The age of a session that the list holds, in days, when no process runs it.
pub const MAX_AGE_DAYS: i64 = 14;

const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// One agent program whose sessions a mirror can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MirrorProgram {
    /// Claude Code.
    Claude,
    /// Codex.
    Codex,
    /// Pi.
    Pi,
    /// Antigravity, whose store this build cannot read.
    Antigravity,
}

/// Every program that a list names, in the order of its rows.
pub const PROGRAMS: [MirrorProgram; 4] = [
    MirrorProgram::Claude,
    MirrorProgram::Codex,
    MirrorProgram::Pi,
    MirrorProgram::Antigravity,
];

impl MirrorProgram {
    /// The word of the program in a frame.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Pi => "pi",
            Self::Antigravity => "antigravity",
        }
    }

    /// The two letters that stand where the row of a terminal shows its mark.
    #[must_use]
    pub const fn mark(self) -> &'static str {
        match self {
            Self::Claude => "CC",
            Self::Codex => "CX",
            Self::Pi => "PI",
            Self::Antigravity => "AG",
        }
    }

    /// The name of the program that a row and a header show.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Pi => "Pi",
            Self::Antigravity => "Antigravity",
        }
    }

    /// The program of the word `key`, or `None` for a word this build does not
    /// know. A frame of a newer computer names a program that an older one leaves
    /// out instead of refusing the whole list.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        PROGRAMS.into_iter().find(|program| program.key() == key)
    }

    /// Whether this build reads the session store of the program. Antigravity keeps
    /// one SQLite file for each conversation under `~/.gemini`, whose steps are
    /// protobuf messages, and this build holds no reader of them.
    #[must_use]
    pub const fn reads_sessions(self) -> bool {
        !matches!(self, Self::Antigravity)
    }
}

/// One session in the list of one publishing computer.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct MirrorSession {
    /// The program of the session.
    pub program: MirrorProgram,
    /// The id of the session inside its store.
    pub session: String,
    /// The title of the session, in one line.
    pub title: String,
    /// The name of the folder of the session.
    pub folder: String,
    /// The path that holds that folder, with the home folder as `~`.
    pub parent: String,
    /// The time of the last change of the store of the session, in milliseconds of
    /// the clock of the computer that publishes it.
    pub updated_at: i64,
    /// Whether a process of the program runs the session now.
    pub live: bool,
}

/// The identity of one session of one computer.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionKey {
    pub program: MirrorProgram,
    pub session: String,
}

/// One row of a session of this computer, with the file that holds its entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirrorSessionRow {
    /// The row of the list.
    pub session: MirrorSession,
    /// The file of the entries of that session, when this build found one.
    pub source: Option<PathBuf>,
}

impl MirrorSessionRow {
    /// The identity of the session of this row.
    #[must_use]
    pub fn key(&self) -> SessionKey {
        SessionKey {
            program: self.session.program,
            session: self.session.session.clone(),
        }
    }
}

/// The list of the sessions of one computer, and the programs whose store this
/// build cannot read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MirrorCatalog {
    rows: Vec<MirrorSessionRow>,
    unreadable: Vec<MirrorProgram>,
}

impl MirrorCatalog {
    /// The rows of the list, newest first.
    #[must_use]
    pub fn rows(&self) -> &[MirrorSessionRow] {
        &self.rows
    }

    /// The programs that the list names with no session.
    #[must_use]
    pub fn unreadable(&self) -> &[MirrorProgram] {
        &self.unreadable
    }

    /// The row of one session.
    #[must_use]
    pub fn row(&self, program: MirrorProgram, session: &str) -> Option<&MirrorSessionRow> {
        self.rows
            .iter()
            .find(|row| row.session.program == program && row.session.session == session)
    }
}

/// One entry of a session, as the dock draws it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MirrorEntry {
    /// The text of the person.
    Person { text: String },
    /// The text of the agent.
    Agent { text: String },
    /// The name of a tool and the first line of what it was asked to do.
    Tool { name: String, request: String },
    /// One line of Horizon: the rest of the store could not be read.
    Notice { text: String },
}

/// The notice of a file whose earlier part the reader did not read.
pub const CUT_NOTICE: &str = "The earlier part of this session is not shown.";
/// The notice of a file with a line that this build could not read.
pub const UNREAD_NOTICE: &str = "Some lines of this session could not be read.";

/// The home that holds the session stores of this computer: the home folder of the
/// person, as the readers of the session pickup name it.
#[must_use]
pub fn stores_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The sessions of the agent programs of `home`, newest first, at most
/// [`MAX_LIST_SESSIONS`] of them.
///
/// `now_millis` is the clock of this computer, which the age bound counts from, and
/// `alive` answers whether one process id still runs, so a test reads no process of
/// this computer and a publisher reads the live registry as the pickup does.
#[must_use]
pub fn read_list(home: &Path, now_millis: i64, alive: &dyn Fn(u64) -> bool) -> MirrorCatalog {
    crate::ui_thread::blocking("the session stores of the mirror are read");
    let records = load_sessions_of_home(home).unwrap_or_else(|error| {
        tracing::warn!("the mirror cannot read the session stores of this computer: {error}");
        Vec::new()
    });
    let live = live_claude_session_ids_in(home, alive);
    let mut rows = Vec::new();
    for record in records {
        let Some(program) = program_of(record.kind) else {
            continue;
        };
        let is_live = program == MirrorProgram::Claude && live.contains(&record.session_id);
        if let Some(row) = row_of(&record, program, home, now_millis, is_live) {
            rows.push(row);
        }
    }
    rows.extend(pi::rows(home, now_millis));
    rows.sort_by_key(|row| std::cmp::Reverse(row.session.updated_at));
    rows.truncate(MAX_LIST_SESSIONS);
    MirrorCatalog {
        rows,
        unreadable: PROGRAMS
            .into_iter()
            .filter(|program| !program.reads_sessions())
            .collect(),
    }
}

/// The program of one kind of agent panel.
fn program_of(kind: crate::PanelKind) -> Option<MirrorProgram> {
    match kind {
        crate::PanelKind::Claude => Some(MirrorProgram::Claude),
        crate::PanelKind::Codex => Some(MirrorProgram::Codex),
        crate::PanelKind::Pi => Some(MirrorProgram::Pi),
        _ => None,
    }
}

/// The row of one record of a store, or `None` for a session that the list leaves
/// out: a session of a subagent, a session older than [`MAX_AGE_DAYS`] that no
/// process runs, and a session with no id of a form that a list shows.
fn row_of(
    record: &AgentSessionRecord,
    program: MirrorProgram,
    home: &Path,
    now_millis: i64,
    live: bool,
) -> Option<MirrorSessionRow> {
    if !record.interactive && !live {
        return None;
    }
    let oldest = now_millis.saturating_sub(MAX_AGE_DAYS.saturating_mul(DAY_MILLIS));
    if !live && record.updated_at < oldest {
        return None;
    }
    let session = one_line(&record.session_id, MAX_SESSION_ID_CHARS);
    if session.is_empty() {
        return None;
    }
    Some(MirrorSessionRow {
        source: source_of(home, program, &session),
        session: session_row(record, program, home, live, session),
    })
}

/// The row of one session of one program.
fn session_row(
    record: &AgentSessionRecord,
    program: MirrorProgram,
    home: &Path,
    live: bool,
    session: String,
) -> MirrorSession {
    let (folder, parent) = record.cwd.as_deref().map_or_else(
        || (String::new(), String::new()),
        |cwd| folder_name_and_parent(Path::new(cwd), home),
    );
    let title = record
        .label
        .as_deref()
        .map(|label| one_line(label, MAX_TITLE_CHARS))
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| format!("{} session", program.name()));
    MirrorSession {
        program,
        session,
        title,
        folder,
        parent,
        updated_at: record.updated_at,
        live,
    }
}

/// The file that holds the entries of one session of this computer.
///
/// The catalog of the runtime state names a session by its id, so the file of a
/// session is found from its id inside the store of its program. The scan reads
/// names only, never a line of a store.
#[must_use]
pub fn source_of(home: &Path, program: MirrorProgram, session: &str) -> Option<PathBuf> {
    match program {
        MirrorProgram::Claude => claude::source(home, session),
        MirrorProgram::Codex => codex::source(home, session),
        MirrorProgram::Pi => pi::source(home, session),
        MirrorProgram::Antigravity => None,
    }
}

/// The last [`MAX_ENTRIES`] entries of the session of `program` whose file is
/// `source`, in their order.
///
/// A file that this build cannot open answers no entry. A file whose earlier part
/// the reader did not read answers one notice before its entries, and a file with a
/// line that this build could not read answers one notice after them.
#[must_use]
pub fn read_entries(program: MirrorProgram, source: &Path) -> Vec<MirrorEntry> {
    crate::ui_thread::blocking("one session of the mirror is read");
    let tail = match file::read_tail(source, MAX_TAIL_BYTES) {
        Ok(tail) => tail,
        Err(error) => {
            tracing::debug!(source = %source.display(), %error, "the mirror cannot read one session file");
            return Vec::new();
        }
    };
    let parsed = match program {
        MirrorProgram::Claude => claude::entries(&tail.text),
        MirrorProgram::Codex => codex::entries(&tail.text),
        MirrorProgram::Pi => pi::entries(&tail.text),
        MirrorProgram::Antigravity => file::Parsed::default(),
    };
    let mut entries = Vec::with_capacity(parsed.entries.len() + 2);
    if tail.starts_mid_line {
        entries.push(notice(CUT_NOTICE));
    }
    entries.extend(parsed.entries);
    if parsed.unread > 0 {
        entries.push(notice(UNREAD_NOTICE));
    }
    let from = entries.len().saturating_sub(MAX_ENTRIES);
    entries.split_off(from)
}

/// One notice entry.
fn notice(text: &str) -> MirrorEntry {
    MirrorEntry::Notice { text: text.to_owned() }
}

/// The keys of a tool input that become the request of a tool call, as the codec of
/// the bridge reads them. Nothing else of the input is read.
const SUMMARY_KEYS: [(&str, &str); 8] = [
    ("Bash", "command"),
    ("Read", "file_path"),
    ("Write", "file_path"),
    ("Edit", "file_path"),
    ("MultiEdit", "file_path"),
    ("NotebookEdit", "file_path"),
    ("shell", "command"),
    ("exec", "command"),
];

/// The first line of the request of one tool call whose input is an object, from
/// its one key of [`SUMMARY_KEYS`]. A tool without such a key is its own request.
fn summary(name: &str, input: Option<&Value>) -> String {
    let Some((_, key)) = SUMMARY_KEYS.iter().find(|(tool, _)| *tool == name) else {
        return one_line(name, MAX_SUMMARY_CHARS);
    };
    match input.and_then(|input| input.get(*key)).and_then(Value::as_str) {
        Some(value) => request_text(value),
        None => one_line(name, MAX_SUMMARY_CHARS),
    }
}

/// The first line of the request of one tool call, cut at [`MAX_SUMMARY_CHARS`].
fn request_text(text: &str) -> String {
    let first = text.split(['\n', '\r']).next().unwrap_or_default();
    one_line(first, MAX_SUMMARY_CHARS)
}

/// Whether one process id of this computer still runs, as the live registry of
/// Claude Code asks it.
#[must_use]
pub fn process_alive(pid: u64) -> bool {
    u32::try_from(pid).is_ok_and(crate::process::is_alive)
}
