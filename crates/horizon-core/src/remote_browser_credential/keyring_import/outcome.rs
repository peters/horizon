//! What one import of the keyring says: the counts of the line under its button,
//! and the typed failure that keeps every value out of a text.
//!
//! Neither type here can print a value. [`KeyringImportReport`] holds counts, and
//! [`KeyringImportError`] holds a rule and, at most, a program name, an exit code,
//! a reference or the path of the secrets file. The output of a program of the
//! import is untrusted text that carries values, so it never reaches either type:
//! a reading that cannot be parsed is [`KeyringImportError::Unreadable`] and
//! nothing else.

use std::fmt;

use thiserror::Error;

use super::super::{CredentialValueChange, RemoteCredentialError};

/// The counts of one import, as the Settings tab writes them: "Imported 2 values.
/// 1 was already here. 3 entries of the keyring are not used by this version."
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KeyringImportReport {
    /// The rows of the secrets file that this import wrote.
    pub imported: usize,
    /// The references that the keyring holds and that this computer already had,
    /// so no row was written over.
    pub already_here: usize,
    /// The entries of the keyring that name no reference of these settings.
    pub unused: usize,
}

impl KeyringImportReport {
    /// The same counts after one more written row.
    pub(super) fn written(&mut self) {
        self.imported += 1;
    }

    /// The same counts after one more reference that this computer already had.
    pub(super) fn kept(&mut self) {
        self.already_here += 1;
    }

    /// The same counts after one more entry that names nothing.
    pub(super) fn skipped(&mut self) {
        self.unused += 1;
    }
}

impl fmt::Display for KeyringImportReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Imported {} {}.",
            self.imported,
            plural(self.imported, "value", "values")
        )?;
        if self.already_here > 0 {
            let verb = if self.already_here == 1 { "was" } else { "were" };
            write!(formatter, " {} {verb} already here.", self.already_here)?;
        }
        if self.unused > 0 {
            let verb = if self.unused == 1 { "is" } else { "are" };
            write!(
                formatter,
                " {} {} of the keyring {verb} not used by this version.",
                self.unused,
                plural(self.unused, "entry", "entries")
            )?;
        }
        Ok(())
    }
}

fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 { one } else { many }
}

/// What one import answered: the counts for the line, and the references whose row
/// this process wrote, for the link to a server.
pub struct KeyringImportOutcome {
    pub report: KeyringImportReport,
    pub changes: Vec<CredentialValueChange>,
}

impl KeyringImportOutcome {
    /// The counts of one import that wrote `changes`.
    #[must_use]
    pub fn new(report: KeyringImportReport, changes: Vec<CredentialValueChange>) -> Self {
        Self { report, changes }
    }
}

impl fmt::Debug for KeyringImportOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KeyringImportOutcome")
            .field("report", &self.report)
            .field("changes", &self.changes)
            .finish()
    }
}

/// Why the import of the keyring imported nothing, in the words of the rule that
/// refused it.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum KeyringImportError {
    /// No program of this computer reads the keyring, and the settings offer the
    /// package that holds one.
    #[error("Install the package `libsecret-tools` to read the values that an older version stored in the keyring.")]
    ToolMissing,
    /// The keyring is there and it is locked.
    #[error("The keyring is locked. Unlock it and try the import again.")]
    Locked,
    /// This computer has no keyring service.
    #[error("This computer has no keyring service, so no value can be read.")]
    Absent,
    /// The program could not be started, read or reaped.
    #[error("The program `{program}` could not be run, so no value was imported.")]
    NotRunnable { program: &'static str },
    /// The program answered nothing before the bound passed.
    #[error("The keyring program did not answer within {seconds} seconds, so no value was imported.")]
    TimedOut { seconds: u64 },
    /// The program printed text that no rule of this version can read.
    #[error("The keyring program printed something that this version cannot read, so no value was imported.")]
    Unreadable,
    /// The program ended with a failure.
    #[error("The program `{program}` ended with exit code {code}, so no value was imported.")]
    ProgramFailed { program: &'static str, code: i32 },
    /// The program ended without an exit code, so a signal ended it.
    #[error("The program `{program}` ended without an exit code, so no value was imported.")]
    ProgramSignalled { program: &'static str },
    /// A reference of the settings breaks the rule that the Secrets file applies.
    #[error("The reference `{reference}` breaks the reference rule, so no value was imported.")]
    RefusedReference { reference: String },
    /// The value of an entry breaks the rule that the secrets file applies.
    #[error("The keyring holds a value that this version cannot keep, so no value was imported.")]
    RefusedValue,
    /// The secrets file of this computer refused the write.
    #[error(transparent)]
    Store(#[from] RemoteCredentialError),
    /// The secrets file of this computer is not open yet.
    #[error("The secrets file of this computer is not open yet. Try the import again in a moment.")]
    FileNotOpen,
    /// One import at a time.
    #[error("An import of the keyring is already running.")]
    Busy,
    /// The settings name no reference whose value the keyring can hold.
    #[error("The settings of this computer name no reference whose value the keyring keeps.")]
    NothingToDo,
    /// The worker thread of the import did not start.
    #[error("The import of the keyring did not start.")]
    NotStarted,
}

impl KeyringImportError {
    /// The stable code of this failure, for a log line and a test.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::ToolMissing => "keyring_tool_missing",
            Self::Locked => "keyring_locked",
            Self::Absent => "keyring_absent",
            Self::NotRunnable { .. } => "keyring_not_runnable",
            Self::TimedOut { .. } => "keyring_timeout",
            Self::Unreadable => "keyring_unreadable",
            Self::ProgramFailed { .. } | Self::ProgramSignalled { .. } => "keyring_program_failed",
            Self::RefusedReference { .. } => "keyring_reference_refused",
            Self::RefusedValue => "keyring_value_refused",
            Self::Store(_) => "keyring_store",
            Self::FileNotOpen => "keyring_file_not_open",
            Self::Busy => "keyring_busy",
            Self::NothingToDo => "keyring_nothing_to_do",
            Self::NotStarted => "keyring_not_started",
        }
    }
}
