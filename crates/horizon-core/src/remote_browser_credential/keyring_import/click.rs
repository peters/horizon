//! The state of one click in the Settings tab: the line that the last import
//! answered, the networks that an older Horizon offers, and the import that runs now.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;

use super::import::{Tailnets, create_tailnets, import_from_keyring};
use super::outcome::{KeyringImportError, KeyringImportOutcome, KeyringImportReport, TailnetReport};
use super::plan::KeyringImportPlan;
use super::readers::{Bound, Platform};
use crate::bounded_program::ProgramEnvironment;
use crate::named_secrets::{NamedSecretChange, NamedSecretVault};
use crate::remote_browser_credential::{CredentialValueChange, SharedStore};

/// Which button of the tab started the job that answered, so each answer reaches
/// its own line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    /// The button "Import from the keyring".
    Import,
    /// The button "Create these secrets" of one network.
    Secrets,
}

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

    fn of_tailnets(report: &TailnetReport) -> Self {
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

/// The import of the keyring of the Settings tab: the answer of the last click, the
/// networks of an older Horizon that the last click offered, and the import that is
/// running now.
///
/// The struct owns a worker thread per click, so the UI thread never reads a
/// keyring, never starts a program and never waits for one.
pub struct KeyringImport {
    platform: Platform,
    environment: ProgramEnvironment,
    bound: Bound,
    /// The `cloud` folder of the home of this build, where the older Horizon saved
    /// the names of its networks.
    old: PathBuf,
    running: Option<Receiver<(Phase, Result<KeyringImportOutcome, KeyringImportError>)>>,
    line: Option<ImportLine>,
    /// The answer of the last click on the button of a network.
    secrets_line: Option<ImportLine>,
    /// The networks that the keyring of this system holds a client for, as names only.
    offer: Option<super::tailnet::TailnetOffer>,
    /// The references whose row this process wrote, for the link to a server.
    changes: Vec<CredentialValueChange>,
    /// The named secrets that this process created, for the link to a server and for
    /// the list of the Secrets tab.
    created: Vec<NamedSecretChange>,
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
        let old = super::tailnet::old_store(crate::HorizonHome::resolve().root());
        Self::with_programs(platform, ProgramEnvironment::of_this_process(), platform.bound()).with_old(old)
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
            old: PathBuf::new(),
            running: None,
            line: None,
            secrets_line: None,
            offer: None,
            changes: Vec::new(),
            created: Vec::new(),
        }
    }

    /// The same import, with the `cloud` folder that holds the saved networks of the
    /// older Horizon. A test names its own folder, so it never reads a home of a
    /// person.
    #[must_use]
    pub fn with_old(mut self, old: PathBuf) -> Self {
        self.old = old;
        self
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

    /// The line under the button of a network, or `None` before the first click on it.
    #[must_use]
    pub fn secrets_line(&self) -> Option<&ImportLine> {
        self.secrets_line.as_ref()
    }

    /// The networks of an older Horizon that the keyring holds a client for, as names
    /// only, or `None` before the first click.
    #[must_use]
    pub fn offer(&self) -> Option<&super::tailnet::TailnetOffer> {
        self.offer.as_ref()
    }

    /// Takes the offer that a reading answered, which is what the tab draws. A worker
    /// calls it with its answer; a test draws the lines of an offer that a worker
    /// would answer.
    pub fn adopt_offer(&mut self, offer: super::tailnet::TailnetOffer) {
        self.offer = Some(offer);
    }

    /// The references that an import wrote since the last call, oldest first. A link
    /// to a server queues each of them as one secret, so the other computers hold the
    /// same value.
    pub fn take_changes(&mut self) -> Vec<CredentialValueChange> {
        std::mem::take(&mut self.changes)
    }

    /// The named secrets that a click created since the last call, oldest first. The
    /// window takes their names into the settings and the link to a server queues each
    /// of them, exactly as it queues a value that a person typed.
    pub fn take_created(&mut self) -> Vec<NamedSecretChange> {
        std::mem::take(&mut self.created)
    }

    /// Read the keyring once and write the rows that these settings name, on a
    /// worker thread. `store` is the one connection of this process to the secrets
    /// file, which the Settings workbench holds, and `vault` is `None` when this
    /// computer keeps no named secret.
    ///
    /// The answer reaches [`Self::line`] after a later [`Self::poll`].
    pub fn start(&mut self, plan: KeyringImportPlan, store: SharedStore, vault: Option<Arc<NamedSecretVault>>) {
        let old = self.old.clone();
        let tailnets = vault.map(|vault| Tailnets::new(old, vault));
        self.spawn(Phase::Import, move |platform, environment, bound| {
            import_from_keyring(platform, &plan, &environment, bound, &store, tailnets.as_ref())
        });
    }

    /// Create the missing named secrets of the network `net`, which is every offered
    /// network when it is `None`, on a worker thread that reads the keyring again.
    ///
    /// The answer reaches [`Self::secrets_line`] after a later [`Self::poll`].
    pub fn start_secrets(&mut self, net: Option<String>, vault: Arc<NamedSecretVault>) {
        let tailnets = Tailnets::new(self.old.clone(), vault);
        self.spawn(Phase::Secrets, move |platform, environment, bound| {
            create_tailnets(platform, &environment, bound, &tailnets, net.as_deref())
        });
    }

    /// Run one job of the import on its own worker thread. One job at a time, so the
    /// line of a click is the answer of the click that the person made.
    fn spawn(
        &mut self,
        phase: Phase,
        work: impl FnOnce(Platform, ProgramEnvironment, Bound) -> Result<KeyringImportOutcome, KeyringImportError>
        + Send
        + 'static,
    ) {
        if self.running.is_some() {
            self.line = Some(ImportLine::of_error(&KeyringImportError::Busy));
            return;
        }
        let (sender, answers) = channel();
        let platform = self.platform;
        let environment = self.environment.clone();
        let bound = self.bound;
        let spawned = thread::Builder::new().name("keyring-import".into()).spawn(move || {
            let outcome = work(platform, environment, bound);
            sender.send((phase, outcome)).ok();
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
            Err(TryRecvError::Disconnected) => (Phase::Import, Err(KeyringImportError::NotStarted)),
        };
        self.running = None;
        let (phase, answer) = answer;
        match answer {
            Ok(outcome) => {
                self.changes.extend(outcome.changes);
                if let Some(offer) = outcome.offer {
                    self.offer = Some(offer);
                }
                match phase {
                    Phase::Import => self.line = Some(ImportLine::of_report(&outcome.report)),
                    Phase::Secrets => {
                        self.secrets_line = Some(ImportLine::of_tailnets(&outcome.created_report));
                        self.created.extend(outcome.created);
                    }
                }
            }
            Err(error) => {
                tracing::warn!(code = error.code(), %error, "the import of the keyring wrote no value");
                let line = ImportLine::of_error(&error);
                match phase {
                    Phase::Import => self.line = Some(line),
                    Phase::Secrets => self.secrets_line = Some(line),
                }
            }
        }
    }
}
