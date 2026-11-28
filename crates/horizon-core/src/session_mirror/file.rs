//! The bounded read of the end of one session file, and the answer of a reader of
//! a store.
//!
//! A session file of a program that ran for weeks is larger than anything that a
//! mirror needs: the dock shows the last entries. The reader therefore takes the
//! last [`MAX_TAIL_BYTES`](super::MAX_TAIL_BYTES) bytes of the file and one time
//! that, and it reports a read that started inside a line, because the first line
//! of such a read is half a line.

use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use super::MirrorEntry;

/// The bytes that the reader took from the end of one file, and whether they start
/// inside a line.
pub(super) struct Tail {
    pub(super) text: String,
    pub(super) starts_mid_line: bool,
}

/// What one reader of a store made of the lines of one file.
#[derive(Default)]
pub(super) struct Parsed {
    pub(super) entries: Vec<MirrorEntry>,
    /// The lines that this build could not parse.
    pub(super) unread: usize,
}

impl Parsed {
    /// Keeps one entry. A notice that repeats the notice before it is one notice.
    pub(super) fn push(&mut self, entry: MirrorEntry) {
        if matches!(entry, MirrorEntry::Notice { .. }) && self.entries.last() == Some(&entry) {
            return;
        }
        self.entries.push(entry);
    }

    /// Keeps one text of the turn of the person, when it holds a text.
    pub(super) fn person(&mut self, text: &str) {
        if let Some(entry) = super::person_entry(text) {
            self.push(entry);
        }
    }

    /// Joins `text` to the text of the agent entry at `last` when that entry holds
    /// one message, and starts a new entry at the end otherwise. Answers the index
    /// of the entry that holds the message of `text`.
    pub(super) fn agent(&mut self, last: Option<usize>, text: &str) -> Option<usize> {
        let text = super::text_block(text, super::MAX_TEXT_CHARS);
        if text.is_empty() {
            return last;
        }
        if let Some(MirrorEntry::Agent { text: held }) = last.and_then(|index| self.entries.get_mut(index)) {
            // Two parts of one message are two paragraphs of it.
            let joined = format!("{held}\n\n{text}");
            *held = super::text_block(&joined, super::MAX_TEXT_CHARS);
            return last;
        }
        self.entries.push(MirrorEntry::Agent { text });
        Some(self.entries.len() - 1)
    }
}

/// The last `max` bytes of `path`, with the line that those bytes cut away.
///
/// # Errors
/// Returns why the file cannot be opened or read. A path that is not a regular file
/// is refused before it is read.
pub(super) fn read_tail(path: &Path, max: u64) -> std::io::Result<Tail> {
    if !std::fs::metadata(path)?.is_file() {
        return Ok(Tail {
            text: String::new(),
            starts_mid_line: false,
        });
    }
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(max);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(max).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    if start == 0 {
        return Ok(Tail {
            text,
            starts_mid_line: false,
        });
    }
    // The read starts inside a line, so that line is not a line of the store.
    let cut = text.find('\n').map_or(text.len(), |at| at + 1);
    Ok(Tail {
        text: text[cut..].to_owned(),
        starts_mid_line: true,
    })
}
