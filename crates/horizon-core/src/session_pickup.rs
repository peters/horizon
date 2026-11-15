//! The sessions that the agent programs of this computer already keep.
//!
//! A person who starts Horizon for the first time usually worked with an agent
//! program in a plain terminal for weeks. Those programs keep their sessions on
//! this computer, and Horizon can resume one of them. This module answers the
//! rows of **Pick up where you left off** on the first start page and of the
//! window **Recent agent sessions** of the titlebar Menu.
//!
//! The module is the one reader of those stores: it builds on
//! [`crate::runtime_state::AgentSessionCatalog`] and its provider readers instead
//! of reading a store a second time. It only reads. It writes no file of a store,
//! of the settings or of a session of Horizon.
//!
//! Every value of a row comes from a store that another program wrote, so this
//! module bounds every text and every read. `docs/review/trust-boundaries/agent-session-pickup.md`
//! states what it guarantees.

mod format;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::config::PresetConfig;
use crate::folder_id::folder_identity;
use crate::panel::PanelKind;
use crate::runtime_state::{AgentSessionKey, live_claude_session_ids_in, load_sessions_of_home};
use crate::setup::{find_program, preset_program};

pub use format::{MAX_FOLDER_CHARS, MAX_TITLE_CHARS, folder_name_and_parent, one_line, relative_time};

/// The agent programs whose session stores this module reads: Claude Code and
/// Codex. Every other kind of agent panel has no reader here: Gemini and
/// `KiloCode` keep no session store that Horizon reads, `OpenCode` and Pi keep one
/// that another code path reads for the session picker of a panel, and the pickup
/// leaves them out of this round.
pub const READ_PROGRAMS: [PanelKind; 2] = [PanelKind::Claude, PanelKind::Codex];

/// Whether the pickup reads the session store of `kind`.
#[must_use]
pub const fn reads_sessions(kind: PanelKind) -> bool {
    matches!(kind, PanelKind::Claude | PanelKind::Codex)
}

/// The longest age of a session that a row offers, in days.
pub const MAX_SESSION_AGE_DAYS: i64 = 14;
/// The most rows that the full list holds.
pub const MAX_SESSION_ROWS: usize = 24;
/// The most rows that the first start page holds: the newest session of each of
/// at most this many folders.
pub const MAX_FIRST_PAGE_ROWS: usize = 6;
/// The most panels that `Open all` opens at one press.
pub const MAX_OPEN_AT_ONCE: usize = 4;
/// The most characters of a session id that a row accepts. Every program of this
/// round names a session with a UUID, whose text is 36 characters.
pub const MAX_SESSION_ID_CHARS: usize = 64;

const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// What a row says about the session, beside its title and its time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickupState {
    /// The program is installed, the folder is there and no other process has
    /// the session.
    Ready,
    /// The folder of the session is gone, so the row cannot open.
    FolderGone,
    /// The program of the session is not installed, so the row cannot open.
    ProgramMissing,
    /// Another program has the session open now. The row opens after a second
    /// confirmation of the person.
    OpenInAnotherProgram,
}

impl PickupState {
    /// Whether a press of `Open` starts this row's panel.
    #[must_use]
    pub const fn opens(self) -> bool {
        matches!(self, Self::Ready | Self::OpenInAnotherProgram)
    }

    /// Whether a press of `Open` needs a second confirmation first.
    #[must_use]
    pub const fn asks_first(self) -> bool {
        matches!(self, Self::OpenInAnotherProgram)
    }

    /// The words that the row shows beside the time, or `None` for a row that
    /// only says what it offers.
    #[must_use]
    pub const fn says(self) -> Option<&'static str> {
        match self {
            Self::Ready => None,
            Self::FolderGone => Some("folder is gone"),
            Self::ProgramMissing => Some("program is not installed"),
            Self::OpenInAnotherProgram => Some("open in another program"),
        }
    }
}

/// One session of one agent program, as a row shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickupRow {
    /// The agent program of the session.
    pub kind: PanelKind,
    /// The id of the session, which is the id of the panel that opens it.
    pub session_id: String,
    /// The title, with one line and at most [`MAX_TITLE_CHARS`] characters.
    pub title: String,
    /// The time of the last activity, as a word such as `2 hours ago`.
    pub last_active: String,
    /// What the row says about the session.
    pub state: PickupState,
}

impl PickupRow {
    /// The identity of the session in the board.
    #[must_use]
    pub fn key(&self) -> AgentSessionKey {
        AgentSessionKey::new(self.kind, self.session_id.clone())
    }

    /// The options of the panel that opens this session of `folder`.
    ///
    /// The panel takes the working directory of the session and the resume of
    /// exactly this session, which is the path that a restart of a panel takes
    /// for its own session: the launch resolves the binding from the resume, so
    /// the panel holds this session id from its first frame.
    ///
    /// The caller names the preset of the program of this row. A preset of
    /// another kind still opens a panel of this row's kind, because the row is
    /// the authority on what it resumes.
    #[must_use]
    pub fn panel_options(
        &self,
        folder: &Path,
        preset: &PresetConfig,
        browser_config: &crate::browser::BrowserConfig,
    ) -> crate::panel::PanelOptions {
        let mut options = preset.to_panel_options(browser_config);
        options.kind = self.kind;
        options.resume = crate::panel::PanelResume::Session {
            session_id: self.session_id.clone(),
        };
        options.cwd = Some(folder.to_path_buf());
        // The title of the conversation is what the person recognizes, and a
        // later title of the program may still replace it.
        options.name = Some(self.title.clone());
        options.name_is_custom = Some(false);
        options
    }
}

/// The sessions of one folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickupFolder {
    /// The folder as the store named it, resolved.
    pub path: PathBuf,
    /// The name of the folder, at most [`MAX_FOLDER_CHARS`] characters.
    pub name: String,
    /// The path that holds the folder, with the home folder as `~`.
    pub parent: String,
    /// Whether the folder exists. A folder that is gone cannot open.
    pub exists: bool,
    /// The sessions of this folder, newest first.
    pub rows: Vec<PickupRow>,
}

impl PickupFolder {
    /// The number of panels that `Open all` starts, or zero for a folder whose
    /// rows open one by one.
    ///
    /// A folder with one row has no `Open all`, and a folder with more rows than
    /// [`MAX_OPEN_AT_ONCE`] opens that many and leaves the rest in the list.
    #[must_use]
    pub fn open_all_count(&self) -> usize {
        if self.rows.len() < 2 {
            return 0;
        }
        self.rows
            .iter()
            .filter(|row| row.state.opens())
            .count()
            .min(MAX_OPEN_AT_ONCE)
    }
}

/// The rows of the pickup, grouped by folder, newest folder first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickupList {
    folders: Vec<PickupFolder>,
}

impl PickupList {
    /// The list of `folders`, in the order that the caller gives.
    ///
    /// [`load`] is the one reader of the stores. This constructor is for a caller
    /// that already holds the folders of its own read.
    #[must_use]
    pub fn new(folders: Vec<PickupFolder>) -> Self {
        Self { folders }
    }

    /// The folders, the one with the newest activity first.
    #[must_use]
    pub fn folders(&self) -> &[PickupFolder] {
        &self.folders
    }

    /// Whether the pickup shows nothing, so the page looks as it does without it.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.folders.is_empty()
    }

    /// The number of rows of the full list.
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.folders.iter().map(|folder| folder.rows.len()).sum()
    }

    /// The rows of the first start page: the newest session of each of at most
    /// [`MAX_FIRST_PAGE_ROWS`] folders.
    #[must_use]
    pub fn first_page(&self) -> Vec<PickupFolder> {
        self.folders
            .iter()
            .take(MAX_FIRST_PAGE_ROWS)
            .filter_map(|folder| {
                let newest = folder.rows.first()?;
                Some(PickupFolder {
                    rows: vec![newest.clone()],
                    ..folder.clone()
                })
            })
            .collect()
    }

    /// The row of one session.
    #[must_use]
    pub fn row(&self, kind: PanelKind, session_id: &str) -> Option<&PickupRow> {
        self.folders
            .iter()
            .flat_map(|folder| &folder.rows)
            .find(|row| row.kind == kind && row.session_id == session_id)
    }

    /// The folder of one session.
    #[must_use]
    pub fn folder_of(&self, kind: PanelKind, session_id: &str) -> Option<&PickupFolder> {
        self.folders.iter().find(|folder| {
            folder
                .rows
                .iter()
                .any(|row| row.kind == kind && row.session_id == session_id)
        })
    }
}

/// One agent program whose sessions a pickup row can offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickupProgram {
    /// The kind of panel that the program starts.
    pub kind: PanelKind,
    /// The program as this computer runs it: the command of the preset, or the
    /// command of the kind when its preset names none.
    pub program: String,
}

impl PickupProgram {
    /// The programs of `presets` whose sessions Horizon can resume, in the order
    /// of the presets.
    ///
    /// A preset of a kind that has no exact session binding offers no row: a
    /// panel of it cannot resume one conversation. A second preset of one kind is
    /// the same program, so it offers nothing new.
    #[must_use]
    pub fn from_presets(presets: &[PresetConfig]) -> Vec<Self> {
        let mut programs: Vec<Self> = Vec::new();
        for preset in presets {
            if !reads_sessions(preset.kind) {
                continue;
            }
            if programs.iter().any(|program| program.kind == preset.kind) {
                continue;
            }
            programs.push(Self {
                kind: preset.kind,
                program: preset_program(preset),
            });
        }
        programs
    }
}

/// The pickup of the agent programs of this computer, read from their stores.
///
/// `open_panels` holds the sessions that an open panel of this Horizon already
/// has: such a session is not offered again. `now_millis` is the time of the
/// clock of the window, which the words of the rows count from.
#[must_use]
pub fn load<H: std::hash::BuildHasher>(
    home: &Path,
    programs: &[PickupProgram],
    open_panels: &HashSet<AgentSessionKey, H>,
    now_millis: i64,
) -> PickupList {
    load_with(home, programs, open_panels, now_millis, |program| {
        find_program(program, None).is_some()
    })
}

/// As [`load`], with the answer of whether a program is installed given by the
/// caller, so a test needs no program on this computer.
#[must_use]
pub fn load_with<H: std::hash::BuildHasher>(
    home: &Path,
    programs: &[PickupProgram],
    open_panels: &HashSet<AgentSessionKey, H>,
    now_millis: i64,
    mut installed: impl FnMut(&str) -> bool,
) -> PickupList {
    crate::ui_thread::blocking("the session stores of the agent programs are read");
    if programs.is_empty() {
        return PickupList::default();
    }
    let installed: HashMap<PanelKind, bool> = programs
        .iter()
        .map(|program| (program.kind, !program.program.is_empty() && installed(&program.program)))
        .collect();
    let records = match load_sessions_of_home(home) {
        Ok(records) => records,
        Err(error) => {
            tracing::warn!("the pickup cannot read the session stores of this computer: {error}");
            return PickupList::default();
        }
    };
    let running = live_claude_session_ids_in(home, alive_process);
    let list = group_rows(records, home, &installed, open_panels, &running, now_millis, programs);
    tracing::info!(
        rows = list.row_count(),
        folders = list.folders.len(),
        "the pickup of the first start page is ready"
    );
    list
}

/// The rows of `records`, filtered, bounded and grouped by folder.
fn group_rows<H: std::hash::BuildHasher>(
    records: Vec<crate::runtime_state::AgentSessionRecord>,
    home: &Path,
    installed: &HashMap<PanelKind, bool>,
    open_panels: &HashSet<AgentSessionKey, H>,
    running: &HashSet<String>,
    now_millis: i64,
    programs: &[PickupProgram],
) -> PickupList {
    let oldest = now_millis.saturating_sub(MAX_SESSION_AGE_DAYS.saturating_mul(DAY_MILLIS));
    let mut usable: Vec<(PathBuf, crate::runtime_state::AgentSessionRecord)> = Vec::new();
    for record in records {
        if !programs.iter().any(|program| program.kind == record.kind) {
            continue;
        }
        if record.updated_at < oldest {
            continue;
        }
        if open_panels.contains(&AgentSessionKey::new(record.kind, record.session_id.clone())) {
            continue;
        }
        // The only value of a store that reaches a command line is the session
        // id, and only after this check of its form.
        if !is_session_id(record.kind, &record.session_id) {
            tracing::warn!(
                kind = ?record.kind,
                "a session of a store is not offered: its id is not of the form that the program uses"
            );
            continue;
        }
        let Some(cwd) = record.cwd.as_deref().map(str::trim).filter(|cwd| !cwd.is_empty()) else {
            continue;
        };
        usable.push((folder_identity(Path::new(cwd)), record));
    }
    usable.sort_by_key(|(_, record)| std::cmp::Reverse(record.updated_at));
    usable.truncate(MAX_SESSION_ROWS);

    let mut folders: Vec<PickupFolder> = Vec::new();
    let mut index: HashMap<PathBuf, usize> = HashMap::new();
    for (path, record) in usable {
        let exists = path.is_dir();
        let state = state_of(record.kind, &record.session_id, exists, installed, running);
        let (name, parent) = folder_name_and_parent(&path, home);
        let row = PickupRow {
            kind: record.kind,
            title: title_of(&record),
            session_id: record.session_id,
            last_active: relative_time(now_millis, record.updated_at),
            state,
        };
        if let Some(position) = index.get(&path) {
            folders[*position].rows.push(row);
        } else {
            index.insert(path.clone(), folders.len());
            folders.push(PickupFolder {
                path,
                name,
                parent,
                exists,
                rows: vec![row],
            });
        }
    }
    // The rows arrive newest first, so a folder stands at the place of its newest
    // row and its own rows keep that order.
    PickupList { folders }
}

/// What the row says about one session.
fn state_of(
    kind: PanelKind,
    session_id: &str,
    folder_exists: bool,
    installed: &HashMap<PanelKind, bool>,
    running: &HashSet<String>,
) -> PickupState {
    if !folder_exists {
        return PickupState::FolderGone;
    }
    if !installed.get(&kind).copied().unwrap_or(false) {
        return PickupState::ProgramMissing;
    }
    if kind == PanelKind::Claude && running.contains(session_id) {
        return PickupState::OpenInAnotherProgram;
    }
    PickupState::Ready
}

/// The title of a row: the title of the program, with the words and the bound of
/// [`one_line`].
fn title_of(record: &crate::runtime_state::AgentSessionRecord) -> String {
    let label = record.label.as_deref().map(str::trim).filter(|label| !label.is_empty());
    let title = label.map_or_else(String::new, |label| one_line(label, MAX_TITLE_CHARS));
    if title.is_empty() {
        format!("{} session", record.kind.display_name())
    } else {
        title
    }
}

/// Whether a process id of a live registry entry still runs.
fn alive_process(pid: u64) -> bool {
    u32::try_from(pid).is_ok_and(crate::process::is_alive)
}

/// Whether `session_id` has the form that the program of `kind` writes.
///
/// Both programs of this round name a session with a UUID in its canonical
/// spelling, so the text that reaches a command line is a fixed set of
/// hexadecimal digits and dashes. A value with a space, a separator or a shell
/// character is not a session id of either program.
fn is_session_id(kind: PanelKind, session_id: &str) -> bool {
    if session_id.len() > MAX_SESSION_ID_CHARS {
        return false;
    }
    match kind {
        PanelKind::Claude | PanelKind::Codex => uuid::Uuid::parse_str(session_id).is_ok(),
        _ => false,
    }
}
