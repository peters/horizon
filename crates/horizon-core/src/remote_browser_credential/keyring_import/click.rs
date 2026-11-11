//! The state of one click in the Settings tab: the line that the last import
//! answered, and the import that runs now.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;

use super::import::import_from_keyring;
use super::outcome::{KeyringImportError, KeyringImportOutcome, KeyringImportReport};
use super::plan::KeyringImportPlan;
use super::program::ProgramEnvironment;
use super::readers::{Bound, Platform};
use crate::remote_browser_credential::{CredentialValueChange, SharedStore};

/// The line under the button of the import: the answer of the last click, and
/// whether it is a refusal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportLine {
    pub text: String,
    /// Whether the line says why nothing was imported, so the tab writes it in the
    /// danger tone.
    pub error: bool,
}

impl ImportLine {
    fn of_report(report: &KeyringImportReport) -> Self {
        Self {
            text: report.to_string(),
            error: false,
        }
    }

    fn of_error(error: &KeyringImportError) -> Self {
        Self {
            text: error.to_string(),
            error: true,
        }
    }
}

/// The import of the keyring of the Settings tab: the answer of the last click, and
/// the import that is running now.
///
/// The struct owns a worker thread per click, so the UI thread never reads a
/// keyring, never starts a program and never waits for one.
pub struct KeyringImport {
    platform: Platform,
    environment: ProgramEnvironment,
    bound: Bound,
    running: Option<Receiver<Result<KeyringImportOutcome, KeyringImportError>>>,
    line: Option<ImportLine>,
    /// The references whose row this process wrote, for the link to a server.
    changes: Vec<CredentialValueChange>,
}

impl Default for KeyringImport {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyringImport {
    /// An import that reads the keyring of this system.
    #[must_use]
    pub fn new() -> Self {
        let platform = Platform::of_this_system();
        Self::with_programs(platform, ProgramEnvironment::of_this_process(), platform.bound())
    }

    /// An import that reads the keyring of `platform` with the programs of
    /// `environment`, within `bound`.
    ///
    /// A test names the platform and points it at a folder of stand-in programs with
    /// a short bound, so a test never reads the keyring of the person who runs it,
    /// and it reads the sentences of either system on either computer.
    #[must_use]
    pub fn with_programs(platform: Platform, environment: ProgramEnvironment, bound: Bound) -> Self {
        Self {
            platform,
            environment,
            bound,
            running: None,
            line: None,
            changes: Vec::new(),
        }
    }

    /// The keyring that this import reads, which names its programs and the sentence
    /// of a missing one.
    #[must_use]
    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// Whether an import is running now, so the tab asks for the next frame.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.running.is_some()
    }

    /// The line under the button, or `None` before the first click.
    #[must_use]
    pub fn line(&self) -> Option<&ImportLine> {
        self.line.as_ref()
    }

    /// The references that an import wrote since the last call, oldest first. A link
    /// to a server queues each of them as one secret, so the other computers hold the
    /// same value.
    pub fn take_changes(&mut self) -> Vec<CredentialValueChange> {
        std::mem::take(&mut self.changes)
    }

    /// Read the keyring once and write the rows that these settings name, on a
    /// worker thread. `store` is the one connection of this process to the secrets
    /// file, which the Settings workbench holds.
    ///
    /// The answer reaches [`Self::line`] after a later [`Self::poll`].
    pub fn start(&mut self, plan: KeyringImportPlan, store: SharedStore) {
        if self.running.is_some() {
            self.line = Some(ImportLine::of_error(&KeyringImportError::Busy));
            return;
        }
        let (sender, answers) = channel();
        let platform = self.platform;
        let environment = self.environment.clone();
        let bound = self.bound;
        let spawned = thread::Builder::new().name("keyring-import".into()).spawn(move || {
            let outcome = import_from_keyring(platform, &plan, &environment, bound, &store);
            sender.send(outcome).ok();
        });
        match spawned {
            Ok(_) => {
                self.line = None;
                self.running = Some(answers);
            }
            Err(error) => {
                tracing::error!(%error, "the worker of the keyring import did not start");
                self.line = Some(ImportLine::of_error(&KeyringImportError::NotStarted));
            }
        }
    }

    /// Take the answer of a finished import, if there is one. Cheap when idle.
    pub fn poll(&mut self) {
        let Some(answers) = self.running.as_ref() else {
            return;
        };
        let answer = match answers.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(KeyringImportError::NotStarted),
        };
        self.running = None;
        match answer {
            Ok(outcome) => {
                self.changes.extend(outcome.changes);
                self.line = Some(ImportLine::of_report(&outcome.report));
            }
            Err(error) => {
                tracing::warn!(code = error.code(), %error, "the import of the keyring wrote no value");
                self.line = Some(ImportLine::of_error(&error));
            }
        }
    }
}
