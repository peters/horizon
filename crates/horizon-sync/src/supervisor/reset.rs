//! What an answer that replaces the state of this computer keeps first: the records,
//! the key bindings and the settings that this computer holds, in one dated file of
//! its home.
//!
//! Two mistakes with a file of the server reach this module. A person who restores or
//! moves an older copy of the file of the server gives the server a log that ends at an
//! index below the index that this computer saw. A person who points this computer at
//! another store gives it rows that the mirror of this computer did not hold. Both
//! answers replace what this computer holds, and both are silent without this module:
//! the machine list of the computer loses a machine that only the newer state had, and
//! the settings of the computer take the older values, with no file that holds them.
//!
//! The rules here:
//!
//! 1. **Nothing goes without a file.** The machine list and the key bindings of this
//!    computer are written to one file of the machine directory, and the settings of
//!    this computer to the backup that a first link writes. Both files carry the time
//!    of the answer in their name.
//! 2. **A log that is only older is not a new server.** When the server's log is
//!    behind the index that this computer saw, the computer is the newer side of that
//!    log: every record that the answer does not hold goes back to the queue, so the
//!    server has it again. Another store is a new state on purpose, so nothing of this
//!    computer is sent to it.
//! 3. **The person reads one line.** It names the case and the two files.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use horizon_core::config_store::ConfigStore;
use horizon_core::settings_sync;
use horizon_core::{AtomicFile, Machine};
use serde::{Deserialize, Serialize};

use super::SyncHandle;
use super::pull::{now_millis, view_bindings};
use super::state::SyncState;
use super::view::view_records;
use crate::error::SyncError;
use crate::intent::Op;
use crate::store::Changes;

/// The first part of the name of the file that keeps the machine list of this
/// computer from before an answer that replaced it. The time of the answer follows
/// it, so two of them never replace each other.
pub(super) const KEPT_PREFIX: &str = "server-before-reset";

/// The format of that file. A reader refuses a file of another format.
const KEPT_FORMAT: u32 = 1;

/// Why the server answered with a state that replaces the state of this computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cause {
    /// The log of the server ends at an index below the index that this computer
    /// saw: a person restored or moved an older copy of the file of the server.
    OlderStore,
    /// The log at the index of this computer is not the log that this store holds.
    AnotherLog,
    /// The server answered with another store than the one that this computer saw.
    AnotherStore,
}

impl Cause {
    /// The cause of an answer at `answer` for the computer that saw `seen`.
    #[must_use]
    pub(super) fn of(seen: u64, answer: u64) -> Self {
        if answer < seen {
            Self::OlderStore
        } else {
            Self::AnotherLog
        }
    }

    /// Whether the records that the answer does not hold go back to the queue. Another
    /// store is the state that the person chose, so nothing of this computer goes there.
    fn sends_missing_records(self) -> bool {
        !matches!(self, Self::AnotherStore)
    }

    fn name(self) -> &'static str {
        match self {
            Self::OlderStore => "older-store",
            Self::AnotherLog => "another-log",
            Self::AnotherStore => "another-store",
        }
    }

    /// The sentence for the person, with the two files that hold what this computer
    /// had. `kept` holds the machine list, and `settings` the settings.
    fn sentence(self, seen: u64, answer: u64, kept: Option<&Path>, settings: Option<&Path>) -> String {
        let mut line = match self {
            Self::OlderStore => format!(
                "The server holds an older state than this computer saw: its log ends at index {answer} and this \
                 computer saw index {seen}."
            ),
            Self::AnotherLog => {
                format!("The server holds another log than the one that this computer saw at index {seen}.")
            }
            Self::AnotherStore => {
                "This computer reached another store at its server, so the machine list and the settings of this \
                 computer are now the ones of that store."
                    .to_owned()
            }
        };
        if self.sends_missing_records() {
            line.push_str(" The machines that the server does not hold are sent to it again.");
        }
        if let Some(file) = kept {
            let _ = write!(
                line,
                " The machine list of this computer is kept in {}.",
                file.display()
            );
        }
        if let Some(file) = settings {
            let _ = write!(line, " The settings of this computer are kept in {}.", file.display());
        }
        line
    }
}

/// The machine list and the key bindings of this computer from before an answer that
/// replaced its mirror, as the one file of that name holds them.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct KeptFile {
    format: u32,
    /// The case, in one word: `older-store`, `another-log` or `another-store`.
    cause: String,
    /// The index that this computer saw, and the index of the answer.
    seen: u64,
    answer: u64,
    /// The records of this computer, with the intents that still waited for the
    /// server applied.
    machines: Vec<Machine>,
    /// The names that this computer wrote itself.
    owned: BTreeSet<String>,
    /// The key that each machine used.
    bindings: BTreeMap<String, String>,
}

/// What a reset keeps, so the records that the server does not hold can go back to the
/// queue after the answer is applied.
#[derive(Debug)]
pub(super) struct Kept {
    records: Vec<Machine>,
    keeps_missing: bool,
}

/// Keeps the machine list and the settings of this computer when `changes` replaces
/// them, and answers what the caller sends back to the server afterwards.
///
/// This runs before the answer is applied, so the files hold what this computer holds
/// before the answer takes it away. A file that cannot be written is one line and not
/// a lost link: the answer of the server still applies.
pub(super) fn keep_before_reset(handle: &SyncHandle, changes: &Changes, seen: u64) -> Option<Kept> {
    if !changes.reset {
        return None;
    }
    let cause = Cause::of(seen, changes.index);
    let (kept, line) = keep(handle, cause, seen, changes.index);
    handle.push_notice(line);
    Some(kept)
}

/// Keeps the machine list and the settings of this computer when the hello of a link
/// names another store, and answers the line for the person.
pub(super) fn keep_before_store_change(handle: &SyncHandle) -> String {
    let seen = handle.lock().client.index();
    let (_, line) = keep(handle, Cause::AnotherStore, seen, seen);
    line
}

/// Keeps what this computer holds in one file and one settings backup, and answers what
/// the caller sends back to the server and the line for the person.
fn keep(handle: &SyncHandle, cause: Cause, seen: u64, answer: u64) -> (Kept, String) {
    let (records, owned, bindings, directory, settings_path) = {
        let state = handle.lock();
        (
            view_records(&state),
            state.owned.clone(),
            view_bindings(&state),
            state.mirror_file.parent().map(Path::to_path_buf),
            state.settings_path.clone(),
        )
    };
    let at = now_millis();
    let kept_path = directory.map(|directory| directory.join(format!("{KEPT_PREFIX}.{at}.json")));
    let mut written = None;
    if let Some(path) = &kept_path {
        let file = KeptFile {
            format: KEPT_FORMAT,
            cause: cause.name().to_owned(),
            seen,
            answer,
            machines: records.clone(),
            owned,
            bindings,
        };
        match write_kept(path, &file) {
            Ok(()) => written = Some(path.clone()),
            Err(error) => tracing::warn!(
                path = %path.display(),
                %error,
                "the machine list of this computer was not kept"
            ),
        }
    }
    let settings = keep_settings(settings_path.as_deref(), at);
    let line = cause.sentence(seen, answer, written.as_deref(), settings.as_deref());
    (
        Kept {
            records,
            keeps_missing: cause.sends_missing_records(),
        },
        line,
    )
}

/// Writes one kept machine list to `path`, at the mode of a private file of the home.
fn write_kept(path: &Path, file: &KeptFile) -> std::io::Result<()> {
    let encoded = serde_json::to_vec_pretty(file)
        .map_err(|error| std::io::Error::other(format!("the kept machine list: {error}")))?;
    AtomicFile::new(path.to_path_buf()).write(&encoded)
}

/// Writes the settings of this computer beside its store, as the backup of a first link
/// is written, and answers its path.
///
/// Only the shared sections are kept: a section that stays with this computer is never
/// replaced by a server. A store that cannot be read keeps nothing, and the answer of
/// the server applies as it does with a backup.
fn keep_settings(path: Option<&Path>, at: u64) -> Option<PathBuf> {
    let store = ConfigStore::resolve(path?);
    let rows = match store.section_rows() {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, "the settings of this computer were not kept");
            return None;
        }
    };
    let kept: BTreeMap<String, String> = rows
        .into_iter()
        .filter(|(section, _)| settings_sync::is_shared(section))
        .collect();
    if kept.is_empty() {
        return None;
    }
    let backup = settings_sync::backup_path(&store, at);
    match settings_sync::write_backup(&backup, &kept) {
        Ok(()) => Some(backup),
        Err(error) => {
            tracing::warn!(
                path = %backup.display(),
                %error,
                "the settings of this computer were not kept"
            );
            None
        }
    }
}

/// Queues every record of `kept` that the answer did not hold, so the server holds it
/// again. The name becomes one that this computer wrote, as it was before the answer.
///
/// The answer took the record out of the mirror, so the record of this computer is the
/// newer one for that name and the queue is where it belongs until the server decides
/// it.
///
/// # Errors
/// Returns why the queue file cannot be written.
pub(super) fn send_missing_records(state: &mut SyncState, kept: &Kept) -> Result<(), SyncError> {
    if !kept.keeps_missing {
        return Ok(());
    }
    for record in &kept.records {
        if state.mirror.contains_key(&record.name) {
            // A name that both hold is no loss: the record of the server decides.
            continue;
        }
        state.client.enqueue(None, Op::MachinePut { record: record.clone() })?;
        state.owned.insert(record.name.clone());
    }
    Ok(())
}
