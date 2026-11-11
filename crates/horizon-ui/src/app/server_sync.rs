//! The link of this Horizon to its server, and the machine list that follows it.
//!
//! A start that names a server in its settings starts one supervisor on a worker
//! thread and reads the machine list through the mirror of that server. A start
//! that names none reads the file store of this computer, exactly as before the
//! server existed, and starts no process.
//!
//! The sidebar line is one short text of the state of the link, and the machine
//! rows of a change that waits for the server are marked pending. Both are read
//! again only when the state or a record changes, so an idle frame reads no
//! machine and builds no string.

use std::sync::Arc;

use horizon_browser::remote::RemoteBrowserConfig;
use horizon_core::machines::{MachineRegistry, MachineStore, Machines};
use horizon_core::named_secrets::{self, NamedSecretVault};
use horizon_core::remote_browser_credential::KeyringCredentialStore;
use horizon_core::secret::{CredentialVault, SecretId, SecretTarget, SecretVault};
use horizon_core::{MachineError, MachineKeys, ServerConfig};
use horizon_sync::{LinkState, Supervisor, SupervisorConfig, SyncHandle, SyncedMachineStore};

use super::HorizonApp;

/// The state line of the machine section, cached by its state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct ServerLine {
    state: &'static str,
    pending: usize,
}

impl ServerLine {
    /// The line of one state of the link.
    pub(in crate::app) fn of(state: &LinkState) -> Self {
        Self {
            state: state.code(),
            pending: state.pending(),
        }
    }

    /// The line that the sidebar shows. `None` would name no server, which is the
    /// case where the section shows no line at all.
    ///
    /// `connecting` is short: only the first attempt of a link reads it, and an
    /// attempt after one that failed reads `away` until a hello answers.
    pub(in crate::app) fn text(&self) -> String {
        match self.state {
            "away" if self.pending == 0 => "Server away".to_owned(),
            "away" => format!("Server away · {} pending", self.pending),
            "in_step" => "Server: in step".to_owned(),
            _ => "Server: connecting".to_owned(),
        }
    }
}

/// The link of this Horizon to its server.
pub(in crate::app) struct ServerSync {
    supervisor: Supervisor,
    /// The credential values of this computer, when it has an OS store: the
    /// addresses of their references follow the remote browser settings.
    credentials: Option<Arc<CredentialVault>>,
    /// The generation of the records that the app read last.
    generation: u64,
    /// A read of the list is on the coordination worker.
    reloading: bool,
}

impl ServerSync {
    fn new(supervisor: Supervisor, credentials: Option<Arc<CredentialVault>>) -> Self {
        let generation = supervisor.handle().generation();
        Self {
            supervisor,
            credentials,
            generation,
            reloading: false,
        }
    }

    /// Takes the addresses of the credential references that the remote browser
    /// settings bind to the OS store of this computer, so a value that the server
    /// holds is written where the settings of this computer read it.
    pub(in crate::app) fn refresh_credentials(&self, remote: &RemoteBrowserConfig) {
        if let Some(credentials) = &self.credentials {
            credentials.register(remote);
        }
    }

    /// The state and the queue of the link.
    pub(in crate::app) fn handle(&self) -> &SyncHandle {
        self.supervisor.handle()
    }

    /// The state line of the sidebar now.
    pub(in crate::app) fn line(&self) -> ServerLine {
        ServerLine::of(&self.supervisor.handle().state())
    }

    /// The stable code of the state, for the answer of `machines`.
    pub(in crate::app) fn state_code(&self) -> &'static str {
        self.supervisor.handle().state().code()
    }

    /// The machine store of this link: the mirror of the server and the queue.
    pub(in crate::app) fn store(&self) -> Arc<dyn MachineStore> {
        Arc::new(SyncedMachineStore::new(self.supervisor.handle().clone()))
    }
}

/// What one read of the machine list of this start answered.
pub(in crate::app) struct MachineLoad {
    pub(in crate::app) machines: Machines,
    /// The link to the server, when the settings name one and it started.
    pub(in crate::app) server: Option<ServerSync>,
    /// Why the link did not start. The list is then the file store of this
    /// computer, so a damaged mirror costs the link and not the machines.
    pub(in crate::app) server_failure: Option<String>,
    /// The named secrets of this computer: the names of its settings and the
    /// values that its credential store holds. A start that names no server keeps
    /// them too, because an agent panel reads them without any server.
    pub(in crate::app) named_secrets: Option<Arc<NamedSecretVault>>,
}

/// What one computer keeps of the secrets that a server holds: its machine keys,
/// the credential values of its remote browser providers, and the named secrets
/// that its agent panels read.
pub(in crate::app) struct SecretHandles {
    pub(in crate::app) target: Arc<dyn SecretTarget>,
    pub(in crate::app) credentials: Option<Arc<CredentialVault>>,
    pub(in crate::app) named: Option<Arc<NamedSecretVault>>,
}

/// The secrets of this Horizon home that follow a server: its machine keys, and
/// its credential values when this computer has an OS credential store. Both are
/// written where the code of this computer already reads them.
///
/// This opens the OS credential store, so it runs on the worker thread that starts
/// the link and never on the UI thread. A computer without an OS store keeps its
/// machine keys alone; a credential value of the server then stays unread here,
/// with one line of the log.
pub(in crate::app) fn secret_vault(keys: MachineKeys, remote: &RemoteBrowserConfig, names: &[String]) -> SecretHandles {
    let credentials = match KeyringCredentialStore::open() {
        Ok(store) => {
            let vault = Arc::new(CredentialVault::new(Box::new(store)));
            vault.register(remote);
            Some(vault)
        }
        Err(error) => {
            tracing::info!(
                %error,
                "this computer has no OS credential store, so only its machine keys follow the server"
            );
            None
        }
    };
    let named_store = named_secret_vault(names);
    let vault = SecretVault::new(keys, credentials.clone()).with_named(Arc::clone(&named_store));
    SecretHandles {
        target: Arc::new(vault),
        credentials,
        named: Some(named_store),
    }
}

/// The named secrets of this computer, with the values that its credential store
/// holds published for the next agent panel.
///
/// This opens a credential store and reads one value per name, so it runs on the
/// worker thread that starts the link and never on the UI thread. A computer
/// without a store keeps the names of its settings and publishes no value, so its
/// panels start without the variables and the Settings window says that this
/// computer holds no value.
fn named_secret_vault(names: &[String]) -> Arc<NamedSecretVault> {
    let store = named_secrets::open_store();
    let vault = Arc::new(NamedSecretVault::new(store));
    vault.set_names(names.iter().cloned());
    match vault.refresh() {
        Ok(present) => {
            tracing::info!(count = present.len(), "the named secrets of this computer are ready");
        }
        Err(error) => {
            tracing::warn!(
                code = error.code(),
                reason = %error,
                "the named secrets of this computer were not read, so the next agent panels get none"
            );
        }
    }
    vault
}

/// Reads the machine list of this start and starts its link. It runs on a worker
/// thread, because it reads files and starts one process.
pub(in crate::app) fn load(
    registry: MachineRegistry,
    server: Option<ServerConfig>,
    secrets: SecretHandles,
    wake: impl Fn() + Send + Sync + 'static,
) -> Result<MachineLoad, MachineError> {
    let Some(server) = server.filter(ServerConfig::names_server) else {
        return Machines::load(registry).map(|machines| MachineLoad {
            machines,
            server: None,
            server_failure: None,
            named_secrets: secrets.named,
        });
    };
    let config = match SupervisorConfig::for_server(
        server.ssh_target().unwrap_or_default(),
        server.command(),
        registry.root(),
        secrets.target,
    ) {
        Ok(config) => config,
        Err(error) => return file_list(registry, error.to_string(), secrets.named),
    };
    match Supervisor::start(config, wake) {
        Ok(supervisor) => {
            let sync = ServerSync::new(supervisor, secrets.credentials);
            match Machines::from_store(&registry, sync.store()) {
                Ok(machines) => Ok(MachineLoad {
                    machines,
                    server: Some(sync),
                    server_failure: None,
                    named_secrets: secrets.named,
                }),
                // The list of the link cannot be read, so the file of this
                // computer is used and the link stops with it.
                Err(error) => file_list(registry, error.to_string(), secrets.named),
            }
        }
        Err(error) => file_list(registry, error.to_string(), secrets.named),
    }
}

/// The machine list of this computer alone, and the reason that the link to the
/// server did not start.
fn file_list(
    registry: MachineRegistry,
    reason: String,
    named_secrets: Option<Arc<NamedSecretVault>>,
) -> Result<MachineLoad, MachineError> {
    tracing::error!(%reason, "the link to the Horizon server did not start");
    Machines::load(registry).map(|machines| MachineLoad {
        machines,
        server: None,
        server_failure: Some(reason),
        named_secrets,
    })
}

impl HorizonApp {
    /// The store that the machine list of this Horizon lives in: the mirror of
    /// the server, or the file of this computer.
    pub(in crate::app) fn machine_store(&self, registry: &MachineRegistry) -> Arc<dyn MachineStore> {
        match &self.server_sync {
            Some(sync) => sync.store(),
            None => Arc::new(registry.clone()),
        }
    }

    /// Queues every credential value that this computer stored or removed for the
    /// server, so the other computers of the person read the same value. The queue
    /// holds the reference and never the value: the link reads it when it pushes.
    pub(super) fn queue_credential_secrets(&mut self) {
        let changes = self.remote_browser_credentials.take_value_changes();
        if changes.is_empty() {
            return;
        }
        let Some(sync) = self.server_sync.as_ref() else {
            // Without a server the value is the value of this computer alone.
            return;
        };
        for change in changes {
            let Ok(id) = horizon_core::SecretId::credential(change.reference.as_str()) else {
                continue;
            };
            let queued = if change.stored {
                sync.handle().queue_secret(&id)
            } else {
                sync.handle().queue_secret_remove(&id)
            };
            if let Err(error) = queued {
                tracing::warn!(%error, id = %id, "a credential value of this computer did not enter the queue");
            }
        }
    }

    /// Takes the named secrets of the startup read: the worker of the window reads
    /// the store again, and the settings of this run keep their names.
    pub(super) fn adopt_named_secrets(&mut self, loaded: Option<Arc<NamedSecretVault>>) {
        let names = match loaded.as_ref() {
            Some(vault) => vault.names(),
            None => self.template_config.secrets.secret_names(),
        };
        self.named_secrets = named_secrets::NamedSecretWorkbench::new(loaded.clone(), &names);
        self.named_secret_vault = loaded;
    }

    /// Runs the worker of the named secrets for this frame. A store that is still
    /// being read keeps frames coming, and a list that changed is read again.
    pub(super) fn poll_named_secrets(&mut self, ctx: &egui::Context) {
        self.named_secrets.poll();
        if self.named_secrets.is_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let names = self.named_secret_names();
        self.named_secrets.refresh_if_stale(&names);
        self.queue_named_secret_changes();
    }

    /// The names of the named secrets of this computer: the names that the link
    /// learned, or the names of the settings before the link answered.
    pub(super) fn named_secret_names(&self) -> Vec<horizon_core::named_secrets::SecretName> {
        match self.named_secret_vault.as_ref() {
            Some(vault) => vault.names(),
            None => self.template_config.secrets.secret_names(),
        }
    }

    /// Queues a named secret that the person stored or deleted on this computer, so
    /// the other computers of the person give it to their agent panels too. The
    /// queue holds the id `named/<NAME>` and never the value.
    pub(super) fn queue_named_secret_changes(&mut self) {
        let changes = self.named_secrets.take_changes();
        if changes.is_empty() {
            return;
        }
        let Some(sync) = self.server_sync.as_ref() else {
            // Without a server the value is the value of this computer alone.
            return;
        };
        for change in changes {
            let Ok(id) = SecretId::named(change.name.as_str()) else {
                continue;
            };
            let queued = if change.stored {
                sync.handle().queue_secret(&id)
            } else {
                sync.handle().queue_secret_remove(&id)
            };
            if let Err(error) = queued {
                tracing::warn!(%error, id = %id, "a named secret of this computer did not enter the queue");
            }
        }
    }

    /// Keeps the names of the named secrets of the settings in step with the names
    /// that the link learned. It copies names and never a value, and the write of
    /// the settings runs on the settings worker.
    fn adopt_named_secret_names(&mut self) {
        let Some(vault) = self.named_secret_vault.as_ref() else {
            return;
        };
        let mut names: Vec<String> = vault.names().into_iter().map(|name| name.as_str().to_owned()).collect();
        names.sort();
        let mut stored = self.template_config.secrets.names.clone();
        stored.sort();
        if stored == names {
            return;
        }
        let mut config = self.template_config.clone();
        config.secrets.names = names;
        if let Err(error) = config.secrets.validate() {
            tracing::warn!(%error, "the names of the named secrets of the link cannot be kept");
            return;
        }
        self.apply_runtime_config(&config);
        if let Some(editor) = self.settings.as_mut() {
            editor.config.secrets.names.clone_from(&config.secrets.names);
        }
        self.settings_io.save_config(self.config_path.clone(), config);
    }

    /// Reads the machine list again when the link brought a change, on the
    /// coordination worker, so no frame reads a file of this home.
    pub(super) fn poll_server_sync(&mut self) {
        // A secret of this computer that lost the version rule is kept, and the
        // person reads where it went in the sidebar.
        let notices = self
            .server_sync
            .as_ref()
            .map(|sync| sync.handle().take_secret_notices())
            .unwrap_or_default();
        if let Some(notice) = notices.into_iter().next_back() {
            self.set_sidebar_notice(notice);
        }
        let Some(sync) = self.server_sync.as_ref() else {
            return;
        };
        let generation = sync.handle().generation();
        if generation == sync.generation || sync.reloading {
            return;
        }
        // A value that arrived from the server also brought its name, and the
        // settings of this computer now list it.
        self.adopt_named_secret_names();
        let registry = self.machine_registry();
        let Some(sync) = self.server_sync.as_mut() else {
            return;
        };
        sync.reloading = true;
        let asked_at = generation;
        let store = sync.store();
        self.browser_create_host.io.then(
            move || Machines::from_store(&registry, store),
            move |app, answer| {
                if let Some(sync) = app.server_sync.as_mut() {
                    sync.reloading = false;
                }
                match answer {
                    Some(Ok(machines)) => {
                        app.machine_list = machines;
                        if let Some(sync) = app.server_sync.as_mut() {
                            sync.generation = asked_at;
                        }
                    }
                    Some(Err(error)) => app.set_sidebar_notice(format!("The machine list of the server: {error}")),
                    None => {}
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use horizon_sync::LinkState;

    use super::ServerLine;

    #[test]
    fn the_line_of_a_server_that_does_not_answer_is_away_after_the_first_attempt() {
        // The state of a link that a start leaves, and of every attempt that a
        // hello did not answer.
        let away = LinkState::Away { pending: 0 };
        assert_eq!(ServerLine::of(&away).text(), "Server away");
        assert_eq!(
            ServerLine::of(&LinkState::Away { pending: 2 }).text(),
            "Server away · 2 pending"
        );
        // Only the first attempt of a link reads `connecting`.
        assert_eq!(ServerLine::of(&away.clone().started(true)).text(), "Server: connecting");
        assert_eq!(
            ServerLine::of(&away.started(false)).text(),
            "Server away",
            "a second attempt of a hung server does not read connecting"
        );
    }

    #[test]
    fn every_state_of_the_link_has_one_short_line() {
        assert_eq!(ServerLine::of(&LinkState::Off).text(), "Server: connecting");
        assert_eq!(ServerLine::of(&LinkState::Connecting).text(), "Server: connecting");
        assert_eq!(ServerLine::of(&LinkState::Behind).text(), "Server: connecting");
        assert_eq!(
            ServerLine::of(&LinkState::InStep { index: 12 }).text(),
            "Server: in step"
        );
        assert_eq!(ServerLine::of(&LinkState::Away { pending: 0 }).text(), "Server away");
        assert_eq!(
            ServerLine::of(&LinkState::Away { pending: 1 }).text(),
            "Server away · 1 pending"
        );
        assert_eq!(
            ServerLine::of(&LinkState::Away { pending: 4 }).text(),
            "Server away · 4 pending"
        );
    }

    #[test]
    fn the_line_follows_the_queue_of_an_away_server() {
        let quiet = ServerLine::of(&LinkState::Away { pending: 0 });
        let waiting = ServerLine::of(&LinkState::Away { pending: 2 });
        assert_ne!(quiet, waiting, "a change that waits changes the line");
        assert_eq!(waiting, ServerLine::of(&LinkState::Away { pending: 2 }));
    }
}
