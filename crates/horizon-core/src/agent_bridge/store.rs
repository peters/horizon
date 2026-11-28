//! The stored conversation of one workspace: one JSON object per line, one file
//! for each workspace of a saved session.
//!
//! A restart of Horizon shows the entries of an earlier run, so the entries
//! leave the memory of the process. One file holds one conversation, and each
//! line of it is one entry with the version of this line format:
//!
//! ```text
//! {"version":1,"entry":{"kind":"person","text":"alpha"}}
//! ```
//!
//! Every function of this module reads or writes a file, so no frame calls one:
//! the worker thread `orchestrator-dock` of the dock runs them. Each function
//! starts with [`crate::ui_thread::blocking`], so a call from a frame is
//! reported by the guard of the UI thread.
//!
//! A file that Horizon reads back is untrusted: the process that wrote it can
//! stop in the middle of a line, a disk can hold a damaged block, and a person
//! can edit it. Thus a line that is not one entry of this version is dropped
//! whole and counted, and one bad line never costs the file. The functions
//! below never hold more than [`MAX_ENTRIES`] entries and one line of
//! [`MAX_LINE_BYTES`] bytes of a file that they read.
//!
//! The folder of the conversations of a session is `0700` and each file of it is
//! `0600`, as the folder of a session is. [`compact`] rewrites a file that holds
//! more than twice [`MAX_ENTRIES`] lines, and it drops the lines that do not
//! parse in the same pass.

use std::collections::VecDeque;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::conversation::{Entry, ToolState};
use super::{MAX_ENTRIES, MAX_LINE_BYTES};
use crate::error::Result;

/// The version of the line format of one stored entry. A line of another version
/// is dropped, so a later format cannot make an older Horizon read a wrong entry.
pub const STORE_VERSION: u32 = 1;

/// The result that a tool call of an earlier run reports when it was still
/// running at the end of that run.
pub const INTERRUPTED_RESULT: &str = "interrupted";

/// The mode of the folder of the conversations of one session.
const DIR_MODE: u32 = 0o700;
/// The mode of one conversation file.
const FILE_MODE: u32 = 0o600;
/// The suffix of one conversation file.
const FILE_SUFFIX: &str = ".jsonl";

/// One line of a conversation file.
#[derive(Deserialize, Serialize)]
struct StoredLine {
    version: u32,
    entry: Entry,
}

/// One line of a file that a reader took, or the reason that it took none.
enum NextLine<'a> {
    /// One line of at most [`MAX_LINE_BYTES`] bytes, without its newline.
    Line(&'a [u8]),
    /// A line above [`MAX_LINE_BYTES`] bytes. Its bytes are gone.
    TooLong,
}

/// The path of the conversation of the workspace `local_id` in `root`.
#[must_use]
pub fn conversation_path(root: &Path, local_id: &str) -> PathBuf {
    root.join(format!("{}{FILE_SUFFIX}", file_name(local_id)))
}

/// Appends `entries` to the file of `path`, one line each, and makes the folder
/// of the file when it is not there.
///
/// # Errors
///
/// Returns an error if the folder or the file cannot be made, or if the write
/// fails. The entries that a failed write did not reach the file are lost.
pub fn append(path: &Path, entries: &[Entry]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    crate::ui_thread::blocking("a stored conversation entry is written");
    let parent = path
        .parent()
        .ok_or_else(|| crate::error::Error::State(format!("the path {} has no folder", path.display())))?;
    make_private_dir(parent)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(FILE_MODE)
        .open(path)?;
    let mut lines = String::new();
    for entry in entries {
        lines.push_str(&encode(entry)?);
        lines.push('\n');
    }
    file.write_all(lines.as_bytes())?;
    file.flush()?;
    Ok(())
}

/// Reads the last `cap` entries of `path`, and says how many lines it dropped:
/// a line that does not parse, a line above [`MAX_LINE_BYTES`] bytes, and an
/// entry older than the `cap` newest ones.
///
/// A file that is not there holds no entry. A tool call that was running when
/// the file was written reads as [`ToolState::Failed`] with
/// [`INTERRUPTED_RESULT`], because the run that owned it ended.
///
/// # Errors
///
/// Returns an error if the file cannot be opened or read. A line that does not
/// parse is not an error: it is dropped and counted.
pub fn load(path: &Path, cap: usize) -> Result<(Vec<Entry>, u64)> {
    crate::ui_thread::blocking("a stored conversation is read");
    let Some(mut lines) = Lines::open(path)? else {
        return Ok((Vec::new(), 0));
    };
    let mut kept: VecDeque<Entry> = VecDeque::new();
    let mut dropped = 0_u64;
    while let Some(next) = lines.next()? {
        match next {
            NextLine::TooLong => dropped = dropped.saturating_add(1),
            NextLine::Line(line) => match decode(line) {
                Some(entry) => {
                    kept.push_back(interrupted(entry));
                    if kept.len() > cap {
                        kept.pop_front();
                        dropped = dropped.saturating_add(1);
                    }
                }
                None => dropped = dropped.saturating_add(1),
            },
        }
    }
    Ok((kept.into(), dropped))
}

/// Rewrites `path` with its last [`MAX_ENTRIES`] entries when it holds more than
/// twice as many lines, so a conversation file stays bounded over many runs. The
/// lines that do not parse leave the file in the same pass. A file that is not
/// there and a file within its bound change nothing.
///
/// # Errors
///
/// Returns an error if the file cannot be read, or if the new file cannot be
/// written and put in its place. A failed rewrite keeps the old file.
pub fn compact(path: &Path) -> Result<()> {
    crate::ui_thread::blocking("a stored conversation is compacted");
    let Some(mut lines) = Lines::open(path)? else {
        return Ok(());
    };
    let mut kept: VecDeque<Entry> = VecDeque::new();
    let mut count = 0_u64;
    while let Some(next) = lines.next()? {
        // Every line counts towards the bound, so a file of lines that do not
        // parse is rewritten too and cannot grow without an end.
        count = count.saturating_add(1);
        let NextLine::Line(line) = next else {
            continue;
        };
        let Some(entry) = decode(line) else {
            continue;
        };
        kept.push_back(interrupted(entry));
        if kept.len() > MAX_ENTRIES {
            kept.pop_front();
        }
    }
    drop(lines);
    if count <= (MAX_ENTRIES as u64).saturating_mul(2) {
        return Ok(());
    }
    let kept: Vec<Entry> = kept.into();
    write_in_place(path, &kept)
}

/// Removes the file of `path`. A file that is not there is no error.
///
/// # Errors
///
/// Returns an error if the file is there and cannot be removed.
pub fn remove(path: &Path) -> Result<()> {
    crate::ui_thread::blocking("a stored conversation is removed");
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// One line of `entry`, or an error when serde refuses it.
fn encode(entry: &Entry) -> Result<String> {
    serde_json::to_string(&StoredLine {
        version: STORE_VERSION,
        entry: entry.clone(),
    })
    .map_err(|error| crate::error::Error::State(format!("a conversation entry has no stored form: {error}")))
}

/// The entry of one line, or `None` for a line that this version does not read.
fn decode(line: &[u8]) -> Option<Entry> {
    if line.len() > MAX_LINE_BYTES {
        return None;
    }
    let text = std::str::from_utf8(line).ok()?;
    let stored: StoredLine = serde_json::from_str(text).ok()?;
    (stored.version == STORE_VERSION).then_some(stored.entry)
}

/// A tool call that was running at the end of the run that wrote the line: the
/// run that owned it ended, so the result of that call never arrived.
fn interrupted(entry: Entry) -> Entry {
    match entry {
        Entry::Tool {
            tool_id,
            name,
            summary,
            state: ToolState::Running,
            ..
        } => Entry::Tool {
            tool_id,
            name,
            summary,
            result: Some(INTERRUPTED_RESULT.to_owned()),
            state: ToolState::Failed,
        },
        entry => entry,
    }
}

/// Makes `dir` with the mode of the conversation folder, and the folders above
/// it as the person's own folder.
fn make_private_dir(dir: &Path) -> Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    DirBuilder::new().recursive(true).mode(DIR_MODE).create(dir)?;
    Ok(())
}

/// Writes `entries` to `path` in place of its text: the new text goes to a file
/// beside it, which takes its place in one rename.
fn write_in_place(path: &Path, entries: &[Entry]) -> Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let written = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(FILE_MODE)
            .open(&temporary)?;
        let mut lines = String::new();
        for entry in entries {
            lines.push_str(&encode(entry)?);
            lines.push('\n');
        }
        file.write_all(lines.as_bytes())?;
        file.flush()?;
        Ok(())
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

/// The name of the file of one workspace: the local id, with every character
/// that is not safe in a name replaced. An id that keeps no character names the
/// file `workspace`.
fn file_name(local_id: &str) -> String {
    let mut name: String = local_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() {
        name.push_str("workspace");
    }
    name
}

/// The lines of one file, read one at a time with a bound on the bytes of one
/// line.
struct Lines {
    reader: BufReader<File>,
    line: Vec<u8>,
}

impl Lines {
    /// The reader of `path`, or `None` when the file is not there.
    fn open(path: &Path) -> Result<Option<Self>> {
        match File::open(path) {
            Ok(file) => Ok(Some(Self {
                reader: BufReader::new(file),
                line: Vec::new(),
            })),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Takes the next line, or `None` at the end of the file. A line above
    /// [`MAX_LINE_BYTES`] bytes keeps no byte of it.
    fn next(&mut self) -> Result<Option<NextLine<'_>>> {
        self.line.clear();
        let mut too_long = false;
        loop {
            let available = match self.reader.fill_buf() {
                Ok(available) => available,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            };
            if available.is_empty() {
                return Ok((!self.line.is_empty() || too_long).then_some(if too_long {
                    NextLine::TooLong
                } else {
                    NextLine::Line(&self.line[..])
                }));
            }
            let (taken, ended) = match available.iter().position(|byte| *byte == b'\n') {
                Some(index) => (index, true),
                None => (available.len(), false),
            };
            if !too_long {
                if self.line.len() + taken > MAX_LINE_BYTES {
                    too_long = true;
                    self.line.clear();
                } else {
                    self.line.extend_from_slice(&available[..taken]);
                }
            }
            self.reader.consume(taken + usize::from(ended));
            if !ended {
                continue;
            }
            if too_long {
                return Ok(Some(NextLine::TooLong));
            }
            if self.line.last() == Some(&b'\r') {
                self.line.pop();
            }
            return Ok(Some(NextLine::Line(&self.line[..])));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_bridge::conversation::{ProposalItem, ProposalState};
    use std::os::unix::fs::PermissionsExt;

    /// Every kind of entry, with a tool call that ended, one that failed and the
    /// reference row of a card.
    fn stored_entries() -> Vec<Entry> {
        vec![
            Entry::Person {
                text: "alpha".to_owned(),
                via_front_desk: false,
            },
            Entry::Agent {
                message_id: "msg-1".to_owned(),
                text: "the answer\non two lines".to_owned(),
            },
            Entry::Tool {
                tool_id: "toolu-1".to_owned(),
                name: "Bash".to_owned(),
                summary: "echo alpha".to_owned(),
                result: Some("alpha".to_owned()),
                state: ToolState::Done,
            },
            Entry::Tool {
                tool_id: "toolu-2".to_owned(),
                name: "Bash".to_owned(),
                summary: "exit 3".to_owned(),
                result: Some("the command failed".to_owned()),
                state: ToolState::Failed,
            },
            Entry::Notice {
                text: "a notice".to_owned(),
            },
            Entry::Card {
                panel: "8f14e45f-ceea-4a67-9f0b-3a1f2b3c4d5e".to_owned(),
                title: "Plan".to_owned(),
            },
            Entry::Person {
                text: "start the second one".to_owned(),
                via_front_desk: true,
            },
            Entry::Proposal {
                id: "3f7c1d9e-1a2b-4c3d-8e4f-5a6b7c8d9e0f".to_owned(),
                items: vec![
                    ProposalItem {
                        title: "Fix login redirect".to_owned(),
                        task: "Fix issue 1987. Done means the test passes.".to_owned(),
                        folder: Some("/home/person/work".to_owned()),
                    },
                    ProposalItem {
                        title: "Faster cold start".to_owned(),
                        task: "question: make the cold start faster.".to_owned(),
                        folder: None,
                    },
                ],
                state: ProposalState::Declined,
            },
            Entry::WorkItem {
                work_item: "9a8b7c6d-5e4f-4a3b-2c1d-0e9f8a7b6c5d".to_owned(),
                name: "Fix login redirect".to_owned(),
            },
        ]
    }

    #[test]
    fn the_entries_of_the_front_desk_survive_the_file() {
        let folder = tempfile::tempdir().expect("a folder");
        let path = path(folder.path());
        let rows = vec![
            Entry::Proposal {
                id: "3f7c1d9e-1a2b-4c3d-8e4f-5a6b7c8d9e0f".to_owned(),
                items: vec![ProposalItem {
                    title: "Faster cold start".to_owned(),
                    task: "make it faster".to_owned(),
                    folder: None,
                }],
                state: ProposalState::Open,
            },
            Entry::WorkItem {
                work_item: "9a8b7c6d-5e4f-4a3b-2c1d-0e9f8a7b6c5d".to_owned(),
                name: "Faster cold start".to_owned(),
            },
            Entry::Person {
                text: "alpha".to_owned(),
                via_front_desk: true,
            },
        ];
        append(&path, &rows).expect("the entries are written");
        let (loaded, dropped) = load(&path, MAX_ENTRIES).expect("the entries are read");
        assert_eq!(loaded, rows, "the proposal, its rows and the caption survive a restart");
        assert_eq!(dropped, 0);
    }

    #[test]
    fn a_person_entry_of_an_older_file_loads_without_the_mark_of_the_front_desk() {
        let folder = tempfile::tempdir().expect("a folder");
        let path = path(folder.path());
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("the folder of the test");
        // A line of the format that an earlier build wrote holds no mark.
        std::fs::write(
            &path,
            "{\"version\":1,\"entry\":{\"kind\":\"person\",\"text\":\"alpha\"}}\n",
        )
        .expect("the file of the test");

        let (loaded, dropped) = load(&path, MAX_ENTRIES).expect("the entry is read");

        assert_eq!(
            loaded,
            vec![Entry::Person {
                text: "alpha".to_owned(),
                via_front_desk: false
            }],
            "an old message of the person is no message of the front desk"
        );
        assert_eq!(dropped, 0, "the line of the older build parses");
    }

    /// The file of the conversation of one workspace in a folder of a test.
    fn path(folder: &Path) -> PathBuf {
        conversation_path(&folder.join("conversations"), "8f14e45f-ceea-4a67-9f0b-3a1f2b3c4d5e")
    }

    #[test]
    fn a_reference_row_of_a_card_loads_as_a_closed_row_after_a_restart() {
        let folder = tempfile::tempdir().expect("a folder");
        let path = path(folder.path());
        let rows = vec![Entry::Card {
            panel: "a-card-that-is-gone".to_owned(),
            title: "Plan".to_owned(),
        }];
        append(&path, &rows).expect("the row is written");

        let (loaded, dropped) = load(&path, MAX_ENTRIES).expect("the row is read");

        assert_eq!(loaded, rows, "the row keeps the card and the heading alone");
        assert_eq!(dropped, 0);
    }

    #[test]
    fn every_kind_of_entry_survives_the_file() {
        let folder = tempfile::tempdir().expect("a folder");
        let path = path(folder.path());
        append(&path, &stored_entries()).expect("the entries are written");
        let (loaded, dropped) = load(&path, MAX_ENTRIES).expect("the entries are read");
        assert_eq!(loaded, stored_entries());
        assert_eq!(dropped, 0, "every line of the file parses");
        // The file is private, and so is its folder.
        let file_mode = std::fs::metadata(&path).expect("the file").permissions().mode() & 0o777;
        let folder_mode = std::fs::metadata(path.parent().expect("a folder"))
            .expect("the folder")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, FILE_MODE, "one conversation file is private");
        assert_eq!(folder_mode, DIR_MODE, "the folder of the conversations is private");
    }

    #[test]
    fn a_second_append_keeps_the_entries_of_the_first() {
        let folder = tempfile::tempdir().expect("a folder");
        let path = path(folder.path());
        append(&path, &stored_entries()[..2]).expect("the first entries are written");
        append(&path, &stored_entries()[2..]).expect("the next entries are written");
        let (loaded, dropped) = load(&path, MAX_ENTRIES).expect("the entries are read");
        assert_eq!(loaded, stored_entries());
        assert_eq!(dropped, 0);
    }

    #[test]
    fn a_bad_line_and_a_long_line_are_dropped_and_counted() {
        let folder = tempfile::tempdir().expect("a folder");
        let path = path(folder.path());
        append(&path, &stored_entries()).expect("the entries are written");
        let mut file = OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("the file of the test");
        file.write_all(b"not a line of this format\n").expect("a bad line");
        file.write_all(&vec![b'x'; MAX_LINE_BYTES + 1]).expect("a long line");
        file.write_all(b"\n").expect("the end of the long line");
        file.write_all(b"{\"version\":99,\"entry\":{\"kind\":\"person\",\"text\":\"later\"}}\n")
            .expect("a line of a later version");
        drop(file);

        let (loaded, dropped) = load(&path, MAX_ENTRIES).expect("the entries are read");
        assert_eq!(loaded, stored_entries(), "the lines that parse stay");
        assert_eq!(dropped, 3, "a bad line, a long line and a line of a later version");
    }

    #[test]
    fn a_load_keeps_the_newest_entries_and_counts_the_older_ones() {
        let folder = tempfile::tempdir().expect("a folder");
        let path = path(folder.path());
        let many: Vec<_> = (0..10)
            .map(|index| Entry::Notice {
                text: format!("entry {index}"),
            })
            .collect();
        append(&path, &many).expect("the entries are written");

        let (loaded, dropped) = load(&path, 4).expect("the entries are read");

        assert_eq!(loaded, many[6..].to_vec(), "the newest four entries of the file");
        assert_eq!(dropped, 6, "the entries above the cap are counted");
    }

    #[test]
    fn a_tool_call_that_was_running_loads_as_interrupted() {
        let folder = tempfile::tempdir().expect("a folder");
        let path = path(folder.path());
        append(
            &path,
            &[Entry::Tool {
                tool_id: "toolu-1".to_owned(),
                name: "Bash".to_owned(),
                summary: "sleep 600".to_owned(),
                result: None,
                state: ToolState::Running,
            }],
        )
        .expect("the entry is written");

        let (loaded, _) = load(&path, MAX_ENTRIES).expect("the entry is read");

        assert_eq!(
            loaded,
            vec![Entry::Tool {
                tool_id: "toolu-1".to_owned(),
                name: "Bash".to_owned(),
                summary: "sleep 600".to_owned(),
                result: Some(INTERRUPTED_RESULT.to_owned()),
                state: ToolState::Failed,
            }]
        );
    }

    #[test]
    fn a_file_that_is_not_there_holds_no_entry() {
        let folder = tempfile::tempdir().expect("a folder");
        let path = path(folder.path());
        assert_eq!(load(&path, MAX_ENTRIES).expect("no file, no entry"), (Vec::new(), 0));
        compact(&path).expect("no file to compact");
        remove(&path).expect("no file to remove");
    }

    #[test]
    fn a_compaction_keeps_the_newest_entries_of_a_file_above_its_bound() {
        let folder = tempfile::tempdir().expect("a folder");
        let path = path(folder.path());
        let many: Vec<_> = (0..(2 * MAX_ENTRIES + 7))
            .map(|index| Entry::Notice {
                text: format!("entry {index}"),
            })
            .collect();
        append(&path, &many).expect("the entries are written");
        let lines_before = std::fs::read_to_string(&path).expect("the file").lines().count();
        assert_eq!(lines_before, 2 * MAX_ENTRIES + 7, "the file is above the bound");

        compact(&path).expect("the file is rewritten");

        let (loaded, dropped) = load(&path, MAX_ENTRIES).expect("the entries are read");
        assert_eq!(
            loaded,
            many[many.len() - MAX_ENTRIES..].to_vec(),
            "the newest entries stay"
        );
        assert_eq!(dropped, 0, "the rewritten file holds no line above the cap");
        let lines_after = std::fs::read_to_string(&path).expect("the file").lines().count();
        assert_eq!(lines_after, MAX_ENTRIES, "the file holds one cap of entries");
    }

    #[test]
    fn a_compaction_rewrites_a_file_of_lines_that_do_not_parse() {
        let folder = tempfile::tempdir().expect("a folder");
        let path = path(folder.path());
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("the folder of the test");
        let mut file = File::create(&path).expect("the file of the test");
        for index in 0..=(2 * MAX_ENTRIES) {
            writeln!(file, "damaged line {index}").expect("a damaged line");
        }
        drop(file);

        compact(&path).expect("the file is rewritten");

        let text = std::fs::read_to_string(&path).expect("the file");
        assert_eq!(text, "", "a file of lines that do not parse keeps no line");
    }

    #[test]
    fn a_file_within_its_bound_keeps_every_line_of_a_compaction() {
        let folder = tempfile::tempdir().expect("a folder");
        let path = path(folder.path());
        append(&path, &stored_entries()).expect("the entries are written");
        let before = std::fs::read_to_string(&path).expect("the file");

        compact(&path).expect("the file is within its bound");

        assert_eq!(std::fs::read_to_string(&path).expect("the file"), before);
    }

    #[test]
    fn a_removed_file_takes_its_entries_with_it() {
        let folder = tempfile::tempdir().expect("a folder");
        let path = path(folder.path());
        append(&path, &stored_entries()).expect("the entries are written");
        remove(&path).expect("the file is removed");
        assert!(!path.exists());
        assert_eq!(load(&path, MAX_ENTRIES).expect("no file, no entry").0, Vec::new());
    }

    #[test]
    fn the_name_of_a_file_keeps_no_character_of_a_path() {
        assert_eq!(file_name("8f14e45f-ceea"), "8f14e45f-ceea");
        assert_eq!(file_name("a/b"), "a_b");
        assert_eq!(file_name("../x"), "___x", "no character of a path reaches a name");
        assert_eq!(file_name(""), "workspace");
    }
}
