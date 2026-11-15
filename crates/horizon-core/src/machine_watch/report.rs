//! The parser of one report of a machine: its lines to a typed report, with the bounds
//! of [`super`].
//!
//! Every byte here comes from another computer, so the parser reads one fixed format:
//! a header that names the version, one block for each pane, and a terminator line. A
//! report that breaks one rule or one bound is dropped whole with a typed reason, and
//! the watch goes on with the report before it. No value of a report is interpreted as
//! a path, a command or a name of anything of this computer.

use super::state::AgentState;
use super::{
    MAX_COMMAND_LINES, MAX_LINE_BYTES, MAX_PANES, MAX_REPORT_BYTES, MAX_REPORT_LINES, MAX_ROWS, WATCH_FORMAT_VERSION,
};

/// The note of a machine that runs no `tmux` session for the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineNote {
    /// The machine has no `tmux` program.
    NoTmux,
    /// The machine has `tmux` and no `tmux` server.
    NoServer,
}

impl MachineNote {
    /// The note that `word` names, or `None` for another word.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "no-tmux" => Some(Self::NoTmux),
            "no-server" => Some(Self::NoServer),
            _ => None,
        }
    }
}

/// One pane of a machine, as its report names it. Every field is one line of words:
/// the text of the watch replaces each run of spaces, tabs and newlines by one space,
/// so the parser reads a field and never a screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneReport {
    /// The pane id of `tmux`, such as `%3`, and its session.
    pub pane: String,
    pub session: String,
    /// The foreground program of the pane, its current folder and its title, which
    /// `tmux` keeps for every pane.
    pub command: String,
    pub cwd: String,
    pub title: String,
    /// The state that the program of the pane reports in the pane option
    /// `@horizon_state`, or `None` for no report and for a word outside the four of the
    /// contract.
    pub claim: Option<AgentState>,
    /// The command lines of the processes of the pane's terminal, newest first, which
    /// the text reports only when an interpreter runs in the pane.
    pub command_lines: Vec<String>,
    /// The last rows of the pane's screen, oldest first.
    pub rows: Vec<String>,
}

/// The panes of one machine and the note that the machine gave instead.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The note of a machine with no `tmux` or no `tmux` server.
    pub note: Option<MachineNote>,
    /// The panes of the report, in the order that the machine named them.
    pub panes: Vec<PaneReport>,
}

/// Why one report of a machine is not a report of this build.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReportError {
    /// One bound of [`super`] is broken and the report is dropped whole: `what` is
    /// above `limit`, or holds more than it.
    #[error("{what} of the report is above its bound of {limit}")]
    Bound {
        /// The part of the report that is above its bound.
        what: &'static str,
        /// The bound that it passed.
        limit: usize,
    },
    /// The report names another version of the format.
    #[error("the report names format version {version}, and this Horizon reads version {expected}")]
    Version {
        /// The version that the report named.
        version: String,
        /// The version of this build.
        expected: u32,
    },
    /// One line is not one line of the format.
    #[error("line {line} of the report is not one line of the format: {reason}")]
    Shape {
        /// The number of the line, counted from 1.
        line: usize,
        /// What the line breaks.
        reason: &'static str,
    },
    /// One line is not UTF-8.
    #[error("line {line} of the report is not UTF-8")]
    NotUtf8 {
        /// The number of the line, counted from 1.
        line: usize,
    },
}

/// The error of a part of a report that is above its bound.
fn bound(what: &'static str, limit: usize) -> ReportError {
    ReportError::Bound { what, limit }
}

/// The error of a line that is not one line of the format.
fn shape(line: usize, reason: &'static str) -> ReportError {
    ReportError::Shape { line, reason }
}

/// The report of the lines of one machine, or why these lines are no report. The lines
/// are one report without their newlines: the header first and the terminator last.
///
/// # Errors
/// [`ReportError`] names the bound or the rule of the format that the lines break.
pub fn parse_report(lines: &[Vec<u8>]) -> Result<Report, ReportError> {
    if lines.len() > MAX_REPORT_LINES {
        return Err(bound("the count of lines", MAX_REPORT_LINES));
    }
    let bytes: usize = lines.iter().map(Vec::len).sum();
    if bytes > MAX_REPORT_BYTES {
        return Err(bound("the bytes", MAX_REPORT_BYTES));
    }
    let mut text = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        if line.len() > MAX_LINE_BYTES {
            return Err(bound("one line in bytes", MAX_LINE_BYTES));
        }
        text.push(std::str::from_utf8(line).map_err(|_| ReportError::NotUtf8 { line: index + 1 })?);
    }
    let mut report = Report::default();
    let (Some(first), Some(last)) = (text.first(), text.last()) else {
        return Err(shape(1, "the report is empty"));
    };
    header(first)?;
    if *last != "Z" {
        return Err(shape(text.len(), "the report does not end with its terminator"));
    }
    for (index, line) in text.iter().enumerate().take(text.len() - 1).skip(1) {
        report.line(line, index + 1)?;
    }
    Ok(report)
}

/// Reads the header of a report, which names the version of its format.
fn header(line: &str) -> Result<(), ReportError> {
    let fields: Vec<&str> = line.split('\t').collect();
    let ["HZ", version] = fields.as_slice() else {
        return Err(shape(1, "the report does not start with its header"));
    };
    if version.parse::<u32>().ok() != Some(WATCH_FORMAT_VERSION) {
        return Err(ReportError::Version {
            version: (*version).to_owned(),
            expected: WATCH_FORMAT_VERSION,
        });
    }
    Ok(())
}

impl Report {
    /// Reads one line of the report into the fields of the report so far.
    fn line(&mut self, line: &str, number: usize) -> Result<(), ReportError> {
        let shape = |reason: &'static str| shape(number, reason);
        let Some((kind, rest)) = line.split_once('\t') else {
            return Err(shape("the line names no kind and no field"));
        };
        match kind {
            "P" => {
                let fields: Vec<&str> = rest.split('\t').collect();
                let [pane, session, command, cwd, title, claim] = fields.as_slice() else {
                    return Err(shape("a pane line holds six values"));
                };
                if !is_pane_id(pane) {
                    return Err(shape("a pane id is '%' and digits"));
                }
                if self.panes.len() >= MAX_PANES {
                    return Err(bound("the count of panes", MAX_PANES));
                }
                self.panes.push(PaneReport {
                    pane: (*pane).to_owned(),
                    session: (*session).to_owned(),
                    command: (*command).to_owned(),
                    cwd: (*cwd).to_owned(),
                    title: (*title).to_owned(),
                    claim: AgentState::claim_of(claim),
                    command_lines: Vec::new(),
                    rows: Vec::new(),
                });
                Ok(())
            }
            "A" => {
                let (pane, command) = pane_and_value(rest).ok_or_else(|| shape("a command line names its pane"))?;
                let held = self
                    .mutable(pane)
                    .ok_or_else(|| shape("a command line names a pane that is no pane of the report"))?;
                if held.command_lines.len() >= MAX_COMMAND_LINES {
                    return Err(bound("the command lines of one pane", MAX_COMMAND_LINES));
                }
                held.command_lines.push(command.to_owned());
                Ok(())
            }
            "R" => {
                let (pane, row) = pane_and_value(rest).ok_or_else(|| shape("a row names its pane"))?;
                let held = self
                    .mutable(pane)
                    .ok_or_else(|| shape("a row names a pane that is no pane of the report"))?;
                if held.rows.len() >= MAX_ROWS {
                    return Err(bound("the rows of one pane", MAX_ROWS));
                }
                held.rows.push(row.to_owned());
                Ok(())
            }
            "X" => {
                if self.note.is_some() {
                    return Err(shape("a report holds one note"));
                }
                self.note = Some(MachineNote::parse(rest).ok_or_else(|| shape("a note is 'no-tmux' or 'no-server'"))?);
                Ok(())
            }
            _ => Err(shape("the line names no kind of the format")),
        }
    }

    fn mutable(&mut self, pane: &str) -> Option<&mut PaneReport> {
        self.panes.iter_mut().find(|held| held.pane == pane)
    }
}

/// The pane id and the value of a line of a pane, or `None` for a line that holds
/// no pane id of the format.
fn pane_and_value(rest: &str) -> Option<(&str, &str)> {
    let (pane, value) = rest.split_once('\t')?;
    is_pane_id(pane).then_some((pane, value))
}

/// Whether `pane` is one pane id of `tmux`: `%` and its digits.
fn is_pane_id(pane: &str) -> bool {
    pane.strip_prefix('%').is_some_and(|digits| {
        !digits.is_empty() && digits.len() <= 9 && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}
