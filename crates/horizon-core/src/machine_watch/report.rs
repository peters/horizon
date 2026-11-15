//! The parser of one report of a machine: its lines to a typed report, with the
//! bounds of [`super`].
//!
//! Every byte here comes from another computer. The parser therefore reads one
//! fixed format: a header that names the version of the format, one block for each
//! pane, and a terminator line. A report that breaks one rule of the format or one
//! bound is dropped whole with a typed reason, and the watch goes on with the
//! report before it. No value of a report is interpreted as a path, a command or a
//! name of anything of this computer.

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
    /// The word of the note in a line of a report.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::NoTmux => "no-tmux",
            Self::NoServer => "no-server",
        }
    }

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

/// One pane of a machine, as its report names it.
///
/// Every field is one line of words: the text of the watch replaces each run of
/// spaces, tabs and newlines by one space, so the parser reads a field and never a
/// screen. `rows` are the last rows of the pane's screen, for one state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneReport {
    /// The pane id of `tmux`, such as `%3`.
    pub pane: String,
    /// The `tmux` session of the pane.
    pub session: String,
    /// The foreground program of the pane.
    pub command: String,
    /// The current folder of the pane.
    pub cwd: String,
    /// The title of the pane, which `tmux` keeps for every pane.
    pub title: String,
    /// The command lines of the processes of the pane's terminal, newest first. The
    /// text reports them only when an interpreter runs in the pane.
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

impl Report {
    /// The pane with this id, when the report holds it.
    #[must_use]
    pub fn pane(&self, pane: &str) -> Option<&PaneReport> {
        self.panes.iter().find(|held| held.pane == pane)
    }
}

/// Why one report of a machine is not a report of this build.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReportError {
    /// One line is longer than the bound of a line.
    #[error("one line of the report is longer than {limit} bytes")]
    LineTooLong {
        /// The bound of one line, in bytes.
        limit: usize,
    },
    /// The report holds more lines than the bound.
    #[error("the report holds more than {limit} lines")]
    TooManyLines {
        /// The bound of the lines of one report.
        limit: usize,
    },
    /// The report holds more bytes than the bound.
    #[error("the report holds more than {limit} bytes")]
    TooLong {
        /// The bound of the bytes of one report.
        limit: usize,
    },
    /// The report holds more panes than the bound.
    #[error("the report holds more than {limit} panes")]
    TooManyPanes {
        /// The bound of the panes of one machine.
        limit: usize,
    },
    /// One pane holds more rows than the bound.
    #[error("one pane of the report holds more than {limit} rows")]
    TooManyRows {
        /// The bound of the rows of one pane.
        limit: usize,
    },
    /// One pane holds more command lines than the bound.
    #[error("one pane of the report holds more than {limit} command lines")]
    TooManyCommandLines {
        /// The bound of the command lines of one pane.
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

/// The report of the lines of one machine, or why these lines are no report.
///
/// The lines are one report without their newlines: the header first and the
/// terminator last.
///
/// # Errors
/// [`ReportError`] names the bound or the rule of the format that the lines break.
pub fn parse_report(lines: &[Vec<u8>]) -> Result<Report, ReportError> {
    if lines.len() > MAX_REPORT_LINES {
        return Err(ReportError::TooManyLines {
            limit: MAX_REPORT_LINES,
        });
    }
    let bytes: usize = lines.iter().map(Vec::len).sum();
    if bytes > MAX_REPORT_BYTES {
        return Err(ReportError::TooLong {
            limit: MAX_REPORT_BYTES,
        });
    }
    let mut text = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        if line.len() > MAX_LINE_BYTES {
            return Err(ReportError::LineTooLong { limit: MAX_LINE_BYTES });
        }
        text.push(std::str::from_utf8(line).map_err(|_| ReportError::NotUtf8 { line: index + 1 })?);
    }
    let mut parsed = Parsed::default();
    let (Some(first), Some(last)) = (text.first(), text.last()) else {
        return Err(ReportError::Shape {
            line: 1,
            reason: "the report is empty",
        });
    };
    header(first)?;
    if *last != "Z" {
        return Err(ReportError::Shape {
            line: text.len(),
            reason: "the report does not end with its terminator",
        });
    }
    for (index, line) in text.iter().enumerate().take(text.len() - 1).skip(1) {
        parsed.line(line, index + 1)?;
    }
    Ok(parsed.done())
}

/// The report that the lines built so far name.
#[derive(Default)]
struct Parsed {
    note: Option<MachineNote>,
    panes: Vec<PaneReport>,
}

/// Reads the header of a report, which names the version of its format.
fn header(line: &str) -> Result<(), ReportError> {
    let fields: Vec<&str> = line.split('\t').collect();
    let ["HZ", version] = fields.as_slice() else {
        return Err(ReportError::Shape {
            line: 1,
            reason: "the report does not start with its header",
        });
    };
    if version.parse::<u32>().ok() != Some(WATCH_FORMAT_VERSION) {
        return Err(ReportError::Version {
            version: (*version).to_owned(),
            expected: WATCH_FORMAT_VERSION,
        });
    }
    Ok(())
}

impl Parsed {
    fn line(&mut self, line: &str, number: usize) -> Result<(), ReportError> {
        let shape = |reason: &'static str| ReportError::Shape { line: number, reason };
        let Some((kind, rest)) = line.split_once('\t') else {
            return Err(shape("the line names no kind and no field"));
        };
        match kind {
            "P" => {
                let fields: Vec<&str> = rest.split('\t').collect();
                let [pane, session, command, cwd, title] = fields.as_slice() else {
                    return Err(shape("a pane line holds five values"));
                };
                if !is_pane_id(pane) {
                    return Err(shape("a pane id is '%' and digits"));
                }
                if self.panes.len() >= MAX_PANES {
                    return Err(ReportError::TooManyPanes { limit: MAX_PANES });
                }
                self.panes.push(PaneReport {
                    pane: (*pane).to_owned(),
                    session: (*session).to_owned(),
                    command: (*command).to_owned(),
                    cwd: (*cwd).to_owned(),
                    title: (*title).to_owned(),
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
                    return Err(ReportError::TooManyCommandLines {
                        limit: MAX_COMMAND_LINES,
                    });
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
                    return Err(ReportError::TooManyRows { limit: MAX_ROWS });
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

    fn done(self) -> Report {
        Report {
            note: self.note,
            panes: self.panes,
        }
    }
}

/// The pane id and the value of a line of a pane, or `None` for a line that holds
/// no pane id of the format.
fn pane_and_value(rest: &str) -> Option<(&str, &str)> {
    let (pane, value) = rest.split_once('\t')?;
    is_pane_id(pane).then_some((pane, value))
}

/// Whether `pane` is one pane id of `tmux`: `%` and its digits.
#[must_use]
pub fn is_pane_id(pane: &str) -> bool {
    pane.strip_prefix('%').is_some_and(|digits| {
        !digits.is_empty() && digits.len() <= 9 && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}
