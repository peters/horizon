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
use horizon_core::HorizonHome;
use horizon_core::machines::{MachineRegistry, MachineStore, Machines};
use horizon_core::named_secrets::{self, NamedSecretChange, NamedSecretVault};
use horizon_core::secret::{CredentialVault, SecretId, SecretTarget, SecretVault};
use horizon_core::secret_store;
use horizon_core::{MachineError, MachineKeys, ServerConfig};
use horizon_sync::{
    LinkRefusal, LinkSnapshot, LinkState, ServerRequest, Supervisor, SupervisorConfig, SyncHandle, SyncedMachineStore,
};

use super::HorizonApp;
use super::settings::server::{ServerAction, ServerView};

/// The state line of the machine section, cached by its state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct ServerLine {
    state: &'static str,
    pending: usize,
    /// Why the last attempt did not open the link, when one failed. A frame reads
    /// it without building a sentence; only the hover and the Server tab do.
    refusal: Option<LinkRefusal>,
}

impl ServerLine {
    /// The line of one state of the link, with the reason of its last attempt.
    pub(in crate::app) fn of(state: &LinkState, refusal: Option<LinkRefusal>) -> Self {
        Self {
            state: state.code(),
            pending: state.pending(),
            refusal,
        }
    }

    /// The sentence of the last attempt, for the hover of the line and the Server
    /// tab. A reason is a fixed sentence of `horizon-sync`, so no address, no name
    /// of a computer and no secret can reach the window through it.
    pub(in crate::app) fn refusal(&self) -> Option<&'static str> {
        self.refusal.map(LinkRefusal::sentence)
    }

    /// The line that the sidebar and the top of the Server tab show. A state that
    /// no person can bring back reads as the reason that the join is the only way
    /// on, and never as a link that is still trying.
    ///
    /// `connecting` is short: only the first attempt of a link reads it, and an
    /// attempt after one that failed reads `away` until a hello answers.
    pub(in crate::app) fn text(&self) -> String {
        match self.state {
            "away" if self.pending == 0 => "Server away".to_owned(),
            "away" => format!("Server away · {} pending", self.pending),
            "in_step" => "Server: in step".to_owned(),
            "removed" => "This computer was removed from the server.".to_owned(),
            "not_logged_in" => "This computer is not logged in to the server.".to_owned(),
            _ => "Server: connecting".to_owned(),
        }
    }
}

/// The link of this Horizon to its server.
pub(in crate::app) struct ServerSync {
    supervisor: Supervisor,
    /// The credential values of this computer: the addresses of their references
    /// follow the remote browser settings.
    credentials: Arc<CredentialVault>,
    /// Whether the link of this computer started with the token of a join, so the
    /// name of this computer is the name of that login and a join cannot take
    /// another one while the token stands.
    token: bool,
    /// The generation of the records that the app read last.
    generation: u64,
    /// A read of the list is on the coordination worker.
    reloading: bool,
    /// The settings of this link, until the live link of the shared terminals
    /// takes them. A link of a test has none, so it starts no live link.
    pub(in crate::app) live: Option<SupervisorConfig>,
}

impl ServerSync {
    /// The link of a test, over a supervisor whose child is a stand-in.
    #[cfg(test)]
    pub(in crate::app) fn for_test(supervisor: Supervisor, credentials: Arc<CredentialVault>) -> Self {
        Self::new(supervisor, credentials)
    }

    fn new(supervisor: Supervisor, credentials: Arc<CredentialVault>) -> Self {
        let generation = supervisor.handle().generation();
        Self {
            supervisor,
            credentials,
            // A link that a test starts holds no token of a login.
            token: false,
            generation,
            reloading: false,
            live: None,
        }
    }

    /// Whether this computer holds the token of a login now, so the name of this
    /// computer belongs to that login.
    ///
    /// The supervisor deletes the token of a link that the server removed, or of
    /// one whose token the server does not know, so those two states answer no.
    pub(in crate::app) fn holds_token(&self) -> bool {
        self.token && !matches!(self.snapshot().state(), LinkState::Removed | LinkState::NotLoggedIn)
    }

    /// Takes the addresses of the credential references that the remote browser
    /// settings bind to the secrets file of this computer, so a value that the server
    /// holds is written where the settings of this computer read it.
    pub(in crate::app) fn refresh_credentials(&self, remote: &RemoteBrowserConfig) {
        self.credentials.register(remote);
    }

    /// The state and the queue of the link.
    pub(in crate::app) fn handle(&self) -> &SyncHandle {
        self.supervisor.handle()
    }

    /// The snapshot of the link that a frame reads: the state, the names that
    /// wait for the server and the generation of the records. The worker of the
    /// link publishes it, so a frame takes no lock that a write of a file holds.
    pub(in crate::app) fn snapshot(&self) -> Arc<LinkSnapshot> {
        self.supervisor.handle().snapshot()
    }

    /// The state line of the sidebar now.
    pub(in crate::app) fn line(&self) -> ServerLine {
        let snapshot = self.snapshot();
        ServerLine::of(snapshot.state(), snapshot.refusal())
    }

    /// The stable code of the state, for the answer of `machines`.
    pub(in crate::app) fn state_code(&self) -> &'static str {
        self.snapshot().state().code()
    }

    /// The store id that this computer saw at its last hello, when it saw one, for
    /// the answer of `server_status`.
    pub(in crate::app) fn store_id(&self) -> Option<String> {
        self.snapshot().store().map(str::to_owned)
    }

    /// The machine store of this link: the mirror of the server and the queue.
    pub(in crate::app) fn store(&self) -> Arc<dyn MachineStore> {
        Arc::new(SyncedMachineStore::new(self.supervisor.handle().clone()))
    }
}

/// The line about the settings of this computer on the server, or `None` while
/// they are in step and nothing waits.
///
/// It is the same small line as the state of the link: one text of the state, so
/// the person reads the settings of the server where the other lines of the link
/// are, and never as a second kind of message. The line about a setting that
/// another computer changed first comes before the state, because it is the one
/// line of the three that a person has to read and close.
fn settings_line(view: &horizon_sync::SettingsSyncView, lost: Option<&str>) -> Option<String> {
    if let Some(line) = lost {
        return Some(line.to_owned());
    }
    if let Some(line) = view.refused() {
        return Some(line.to_owned());
    }
    if !view.pending().is_empty() {
        return Some(format!("Settings: {} pending", view.pending().len()));
    }
    (!view.in_step()).then(|| "Settings: reading the server".to_owned())
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
    /// values that its secrets file holds. A start that names no server keeps them
    /// too, because an agent panel reads them without any server.
    pub(in crate::app) named_secrets: Arc<NamedSecretVault>,
}

/// What one computer keeps of the secrets that a server holds: its machine keys,
/// the credential values of its remote browser providers, and the named secrets
/// that its agent panels read.
pub(in crate::app) struct SecretHandles {
    pub(in crate::app) target: Arc<dyn SecretTarget>,
    pub(in crate::app) credentials: Arc<CredentialVault>,
    pub(in crate::app) named: Arc<NamedSecretVault>,
    /// The secrets file of this computer, which also holds the token of a join.
    pub(in crate::app) store: Option<Arc<horizon_core::secret_store::SecretStore>>,
}

/// The secrets of this Horizon home that follow a server: its machine keys, the
/// credential values of its remote browser providers, and the named secrets of its
/// agent panels. All are written where the code of this computer already reads
/// them.
///
/// This opens the secrets file, so it runs on the worker thread that starts the
/// link and never on the UI thread. A file that cannot be opened is one typed
/// error that the caller logs and the window shows; the machine keys still follow
/// the server.
pub(in crate::app) fn secret_vault(
    home: &HorizonHome,
    keys: MachineKeys,
    remote: &RemoteBrowserConfig,
    names: &[String],
) -> SecretHandles {
    // One file has one connection in this process, so the token row and the values
    // of this computer share the store that the window already holds.
    let (store, credentials) = match secret_store::open_store(home) {
        Ok(store) => (
            Some(Arc::clone(&store)),
            Arc::new(CredentialVault::new(Box::new(store))),
        ),
        Err(error) => {
            tracing::error!(%error, "the secrets file of this computer cannot be opened, so it holds no value");
            (
                None,
                Arc::new(CredentialVault::new(Box::new(
                    horizon_core::secret_store::UnreadableStore::new(error),
                ))),
            )
        }
    };
    credentials.register(remote);
    let secrets = named_secret_vault(home, names);
    let vault = SecretVault::new(keys, Arc::clone(&credentials), Arc::clone(&secrets));
    SecretHandles {
        target: Arc::new(vault),
        credentials,
        named: secrets,
        store,
    }
}

/// The named secrets of this computer, with the values that its secrets file holds
/// published for the next agent panel.
///
/// This reads one value per name, so it runs on the worker thread that starts the
/// link and never on the UI thread. A file that cannot be read keeps the names of
/// its settings and publishes no value, so its panels start without the variables
/// and the Settings window says why.
fn named_secret_vault(home: &HorizonHome, names: &[String]) -> Arc<NamedSecretVault> {
    let vault = Arc::new(NamedSecretVault::new(secret_store::open(home)));
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
///
/// `program` is the child of the link, which is `ssh`. It is a parameter because
/// a test of a path that starts a link substitutes a program that ends at once:
/// a unit test starts no `ssh`.
pub(in crate::app) fn load(
    registry: MachineRegistry,
    server: Option<ServerConfig>,
    settings_path: &std::path::Path,
    secrets: SecretHandles,
    keys: &horizon_core::MachineKeys,
    program: String,
    wake: impl Fn() + Send + Sync + 'static,
) -> Result<MachineLoad, MachineError> {
    let Some(server) = server.filter(ServerConfig::names_server) else {
        return Machines::load(registry).map(|machines| MachineLoad {
            machines,
            server: None,
            server_failure: None,
            named_secrets: secrets.named.clone(),
        });
    };
    // A server that runs on a machine of the list is reached with the login of that
    // machine: the machine holds the machine key of Horizon and no key of the
    // person. The record and the key are files, so this worker reads them.
    let route = match server.machine() {
        Some(machine) => match horizon_sync::machine_login(registry.root(), keys, machine) {
            Ok(route) => Some(route),
            Err(error) => return file_list(registry, error.to_string(), secrets.named),
        },
        None => None,
    };
    // A link of the TCP listener needs an address and the token of a join. An
    // address without a token falls back to the `ssh` child, as the entry did
    // before a join existed.
    let token = secrets
        .store
        .as_deref()
        .and_then(|store| horizon_core::secret_store::read_token(store).ok().flatten());
    let target = server.ssh_target().unwrap_or_default();
    let command = server.command();
    // The name of a link that holds a token is the name of that login, which the
    // join wrote before it took the token. A start of such a link never proposes
    // another name, so the name of the join is the name of every later link.
    let mut config = if token.is_some() {
        match SupervisorConfig::for_login(target, command, registry.root(), secrets.target) {
            Ok(Some(config)) => config,
            Ok(None) => {
                return file_list(
                    registry,
                    "This computer holds the login of a server but not the name of it. Join it again with a link from a computer that is logged in."
                        .to_owned(),
                    secrets.named,
                );
            }
            Err(error) => return file_list(registry, error.to_string(), secrets.named),
        }
    } else {
        match SupervisorConfig::for_server(target, command, registry.root(), secrets.target) {
            Ok(config) => config,
            Err(error) => return file_list(registry, error.to_string(), secrets.named),
        }
    };
    config.token_store.clone_from(&secrets.store);
    config.route.clone_from(&route);
    config.program = program;
    // The sections of the settings of this computer follow the server through the
    // same link: a start reads the store first, so no frame waits for the server,
    // and a section that arrives afterwards is written to that store.
    config.settings_path = Some(settings_path.to_path_buf());
    config.address = server.address().map(str::to_owned).filter(|_| token.is_some());
    if server.address().is_some() && config.address.is_none() && server.ssh_target().is_none() && route.is_none() {
        return file_list(
            registry,
            "This computer is not logged in to the server. Join it with a link from a computer that is logged in."
                .to_owned(),
            secrets.named,
        );
    }
    let holds_token = token.is_some();
    drop(token);
    let live = config.clone();
    match Supervisor::start(config, wake) {
        Ok(supervisor) => {
            let mut sync = ServerSync::new(supervisor, secrets.credentials);
            sync.token = holds_token;
            sync.live = Some(live);
            match Machines::from_store(&registry, sync.store()) {
                Ok(machines) => Ok(MachineLoad {
                    machines,
                    server: Some(sync),
                    server_failure: None,
                    named_secrets: secrets.named.clone(),
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
    named_secrets: Arc<NamedSecretVault>,
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
    /// The state of the link that the Server tab draws, from the snapshot of its
    /// worker: a frame never takes the lock of the link state.
    pub(in crate::app) fn server_view(&self) -> ServerView {
        let Some(sync) = self.server_sync.as_ref() else {
            return ServerView {
                line: "This computer has no server.".to_owned(),
                ..ServerView::default()
            };
        };
        let snapshot = sync.snapshot();
        let state = snapshot.state();
        let logged_out = matches!(state, LinkState::Removed | LinkState::NotLoggedIn);
        let settings = snapshot.settings();
        ServerView {
            line: ServerLine::of(state, snapshot.refusal()).text(),
            refusal: snapshot.refusal().map(LinkRefusal::sentence).map(str::to_owned),
            settings: settings_line(settings, self.settings_lost.as_deref()),
            device: snapshot.device().to_owned(),
            // A computer that the server removed has no link that could ask for an
            // invitation or read the list, so the tab shows the join alone.
            linked: !logged_out,
            logged_out,
            // A login owns its name: the field of the name belongs to the join that
            // made the token, so it is read-only until the token is gone.
            name_locked: sync.holds_token(),
        }
    }

    /// Ends the line about a setting of this computer that another computer
    /// changed first: the link forgets it, so the Server tab loses it in the same
    /// frame as the sidebar.
    pub(in crate::app) fn dismiss_settings_lost(&mut self) {
        if let Some(sync) = self.server_sync.as_ref() {
            sync.handle().dismiss_settings_lost();
        }
        self.settings_lost = None;
    }

    /// Applies one press of the Server tab, after the panel drew it.
    ///
    /// A question travels on the link that the supervisor already holds, so the
    /// invitation, the list and the removal work over `ssh` as well as over the
    /// listener. The answer comes back on a channel that the tab polls.
    pub(in crate::app) fn apply_server_action(&mut self, action: ServerAction, ctx: egui::Context) {
        // The join comes first, and it needs no link: a computer that has no server
        // joins one with a link, which is the only way that it gets a link at all.
        if let ServerAction::Join { link, device } = &action {
            self.start_server_join(link.clone(), device.clone(), ctx);
            return;
        }
        let Some(sync) = self.server_sync.as_ref() else {
            self.server_notice("This computer has no link to the server now.".to_owned());
            return;
        };
        let handle = sync.handle().clone();
        let asked = match &action {
            ServerAction::Invite => handle.ask(ServerRequest::Invite),
            ServerAction::RefreshDevices => handle.ask(ServerRequest::Devices),
            ServerAction::Remove(device) => handle.ask(ServerRequest::Remove(device.clone())),
            ServerAction::Join { .. } => return,
        };
        let Some(editor) = self.settings.as_mut() else {
            return;
        };
        match action {
            ServerAction::Invite => editor.server.hold_invitation(asked),
            ServerAction::RefreshDevices => editor.server.hold_devices(asked),
            ServerAction::Remove(device) => editor.server.hold_removal(&device, asked),
            ServerAction::Join { .. } => {}
        }
    }

    /// Writes one line into the Server tab, or into the sidebar when the window is
    /// closed.
    fn server_notice(&mut self, line: String) {
        match self.settings.as_mut() {
            Some(editor) => editor.server.notice = Some(line),
            None => self.set_sidebar_notice(line),
        }
    }

    /// Joins the server of one link as one name. The join opens a socket, so it
    /// runs on the coordination worker and never on the UI thread; its answer
    /// writes the address of the server and starts the link of it.
    ///
    /// The worker writes the device name of this computer before it takes the
    /// token, so the name of the join is the name of the link that follows it, and
    /// the supervisor reads that name instead of proposing another one.
    fn start_server_join(&mut self, link: String, device: String, ctx: egui::Context) {
        let home = self.session_store.home().clone();
        let directory = self.machine_registry().root().to_path_buf();
        self.browser_create_host.io.then(
            move || -> Result<String, String> {
                horizon_sync::write_device_name(&directory, &device).map_err(|error| error.to_string())?;
                let store = horizon_core::secret_store::SecretStore::open(home.secrets_path())
                    .map_err(|error| error.to_string())?;
                horizon_sync::join(&link, &device, &store).map_err(|error| error.sentence())?;
                horizon_core::JoinLink::parse(&link)
                    .map(|link| link.authority())
                    .map_err(|error| error.to_string())
            },
            move |app, answered| match answered {
                Some(Ok(address)) => app.accepted_server_join(&address, &ctx),
                Some(Err(sentence)) => app.failed_server_join(sentence),
                None => app.failed_server_join("This computer did not finish the join.".to_owned()),
            },
        );
    }

    /// Writes the address of a successful join and starts the link of it.
    ///
    /// The address goes into the settings of the store, and not into a draft that
    /// the person did not save: the entry of the editor keeps every other field of
    /// the store, so a join never writes an unsaved edit of another tab.
    fn accepted_server_join(&mut self, address: &str, ctx: &egui::Context) {
        let mut stored = match self.settings.as_ref() {
            Some(editor) => editor.original.clone(),
            None => self.template_config.clone(),
        };
        stored.server.address = Some(address.to_owned());
        if let Err(error) = stored.server.validate() {
            self.failed_server_join(error.to_string());
            return;
        }
        self.template_config.server = stored.server.clone();
        // The entry `server` stays with this computer, so a join queues no section.
        self.settings_io
            .save_config(self.config_path.clone(), stored.clone(), Vec::new());
        if let Some(editor) = self.settings.as_mut() {
            editor.config.server = stored.server.clone();
            editor.original.server = stored.server.clone();
            editor.server.joining = false;
            editor.server.join_error = None;
            editor.server.link.clear();
            editor.server.notice = Some("This computer joined the server.".to_owned());
        }
        // The old link ends before the new one starts, so one computer never holds
        // two writers of one queue.
        self.restart_server_link(ctx);
    }

    /// Ends the link of this computer and starts the link of the entry that the
    /// settings hold now, as the frame after a join does. A confirmed server
    /// link reaches this too, so the person needs no start of Horizon.
    ///
    /// A start reads the entry of the run in [`HorizonApp::template_config`], so
    /// the caller writes that entry first.
    pub(in crate::app) fn restart_server_link(&mut self, ctx: &egui::Context) {
        // The old link ends before the new one starts, so one computer never holds
        // two writers of one queue.
        self.server_sync = None;
        self.stop_shared_terminals();
        self.settings_request.link_failure = None;
        self.settings_io.set_link(None);
        self.load_machines(ctx);
    }

    /// Shows why a join did not work, in the tab of the window.
    fn failed_server_join(&mut self, sentence: String) {
        let Some(editor) = self.settings.as_mut() else {
            self.set_sidebar_notice(sentence);
            return;
        };
        editor.server.joining = false;
        editor.server.join_error = Some(sentence);
    }

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
        let mut intents = Vec::new();
        for change in changes {
            let Ok(id) = horizon_core::SecretId::credential(change.reference.as_str()) else {
                continue;
            };
            intents.push((id, change.stored));
        }
        self.queue_secret_intents(intents);
    }

    /// Queues one intent of a secret on the coordination worker, in the order that
    /// the frame asked for them.
    ///
    /// The queue of the link is a file that the writer syncs, so the frame copies
    /// the ids and the worker writes them. A queue that cannot be written is one
    /// warning, and the value of this computer still stands.
    pub(super) fn queue_secret_intents(&mut self, intents: Vec<(SecretId, bool)>) {
        if intents.is_empty() {
            return;
        }
        let Some(sync) = self.server_sync.as_ref() else {
            // Without a server the value is the value of this computer alone.
            return;
        };
        let link = sync.handle().clone();
        for (id, stored) in intents {
            let link = link.clone();
            self.browser_create_host.io.write(move || {
                let queued = if stored {
                    link.queue_secret(&id)
                } else {
                    link.queue_secret_remove(&id)
                };
                if let Err(error) = queued {
                    tracing::warn!(%error, id = %id, "a secret of this computer did not enter the queue");
                }
            });
        }
    }

    /// Takes the named secrets of the startup read: the worker of the window reads
    /// the store again, and the settings of this run keep their names.
    pub(super) fn adopt_named_secrets(&mut self, loaded: Arc<NamedSecretVault>) {
        let names = loaded.names();
        self.named_secrets = named_secrets::NamedSecretWorkbench::new(Some(Arc::clone(&loaded)), &names);
        self.named_secret_vault = Some(loaded);
    }

    /// Runs the worker of the named secrets for this frame. A store that is still
    /// being read keeps frames coming, and a name that appeared or went is read
    /// again. An idle frame reads no store and builds no list.
    pub(super) fn poll_named_secrets(&mut self, ctx: &egui::Context) {
        self.named_secrets.poll();
        self.poll_speech_key_state();
        if self.named_secrets.is_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.named_secrets.refresh_if_stale();
        self.queue_named_secret_changes();
    }

    /// Reads the state of the named secret of the key of speech input, so the
    /// Settings tab "Speech" says whether this computer holds it. One read runs for
    /// one name, and never for every frame.
    fn poll_speech_key_state(&mut self) {
        let name = {
            let config = self
                .settings
                .as_ref()
                .map_or(&self.template_config, |editor| &editor.config);
            config.speech.key_secret()
        };
        let Some(name) = name else {
            return;
        };
        if self.speech.asked_key() == Some(&name) {
            return;
        }
        let mut names = self.template_config.secrets.secret_names();
        names.push(name.clone());
        names.sort();
        names.dedup();
        self.named_secrets.refresh(&names);
        self.speech.set_asked_key(name);
    }

    /// Queues a named secret that the person stored or deleted on this computer, so
    /// the other computers of the person give it to their agent panels too. The
    /// queue holds the id `named/<NAME>` and never the value.
    pub(super) fn queue_named_secret_changes(&mut self) {
        let changes = self.named_secrets.take_changes();
        self.queue_named_secret_intents(changes);
    }

    /// Queues the named secrets that an import of the keyring created, so the other
    /// computers of the person give them to their agent panels too.
    pub(super) fn queue_named_secret_intents(&mut self, changes: Vec<NamedSecretChange>) {
        if changes.is_empty() {
            return;
        }
        let mut intents = Vec::new();
        for change in changes {
            let Ok(id) = SecretId::named(change.name.as_str()) else {
                continue;
            };
            intents.push((id, change.stored));
        }
        self.queue_secret_intents(intents);
    }

    /// Takes the named secrets that a click on the keyring import created: their
    /// names enter the `secrets` section as a typed name does, the Secrets tab reads
    /// the state of every name again, and the link to a server queues each of them by
    /// id. The values stay in the secrets file.
    pub(super) fn adopt_created_secrets(&mut self, created: Vec<NamedSecretChange>) {
        if created.is_empty() {
            return;
        }
        self.adopt_named_secret_names();
        let names = self.template_config.secrets.secret_names();
        self.named_secrets.refresh(&names);
        self.queue_named_secret_intents(created);
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
        // The names are one shared section: a name that this computer learned from
        // the server or added for its own tools follows the other computers too.
        let shared = horizon_core::settings_sync::changed_sections(&self.template_config, &config);
        self.apply_runtime_config(&config);
        if let Some(editor) = self.settings.as_mut() {
            editor.config.secrets.names.clone_from(&config.secrets.names);
        }
        self.settings_io.save_config(self.config_path.clone(), config, shared);
    }

    /// Reads the machine list again when the link brought a change, on the
    /// coordination worker, so no frame reads a file of this home.
    pub(super) fn poll_server_sync(&mut self) {
        // A setting of this computer that lost to the value of another computer
        // stays under the state line of the server until the person closes it, so a
        // frame reads it here and never as one of the short notices of an action.
        if let Some(sync) = self.server_sync.as_ref() {
            self.settings_lost = sync.handle().settings_lost();
        }
        // A confirmed server entry answers as soon as the link knows whether the
        // server answered, so this runs every frame and reads only the snapshot.
        self.settle_link_answer();
        // A secret or a setting of this computer that lost the version rule is
        // kept, and the person reads where it went in the sidebar.
        let notices = self
            .server_sync
            .as_ref()
            .map(|sync| sync.handle().take_notices())
            .unwrap_or_default();
        if let Some(notice) = notices.into_iter().next_back() {
            self.set_sidebar_notice(notice);
        }
        let Some(sync) = self.server_sync.as_ref() else {
            return;
        };
        let generation = sync.snapshot().generation();
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
    use std::sync::Arc;

    use egui::Context;
    use horizon_sync::{LinkRefusal, LinkState, Supervisor, SupervisorConfig};

    use super::{NamedSecretVault, ServerLine, ServerSync};
    use crate::app::test_support::{run_app_frame, test_app};

    #[test]
    fn a_frame_reads_the_link_from_the_snapshot_of_its_worker() {
        // The frame path of the sidebar runs under the guard in strict mode: a
        // read of the link that waited for the lock of a write would panic here.
        horizon_core::ui_thread::use_strict_mode();
        let (_temp, mut app, _link_dir) = test_link_app();
        let ctx = Context::default();
        run_app_frame(&ctx, &mut app);
        run_app_frame(&ctx, &mut app);

        let sync = app.server_sync.as_ref().expect("the link of this window");
        assert!(
            matches!(sync.snapshot().state(), LinkState::Away { .. } | LinkState::Connecting),
            "the snapshot says what the link did: {:?}",
            sync.snapshot().state()
        );
        assert_eq!(
            sync.line().text(),
            ServerLine::of(sync.snapshot().state(), sync.snapshot().refusal()).text(),
            "the sidebar line is the snapshot of the link"
        );
    }

    #[test]
    fn the_secrets_of_a_frame_enter_the_queue_on_the_coordination_worker_in_order() {
        // The frame path of a secret runs under the guard in strict mode: a
        // queue write on the frame would panic here.
        horizon_core::ui_thread::use_strict_mode();
        let (temp, mut app, link_dir) = test_link_app();
        let first = horizon_core::SecretId::named("FIRST").expect("a name");
        let second = horizon_core::SecretId::named("SECOND").expect("a name");
        app.queue_secret_intents(vec![(first.clone(), true), (second.clone(), true)]);
        app.settle_browser_host_io();

        let queue: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(link_dir.join("server-queue.json")).expect("the queue"))
                .expect("the queue of the link");
        let ids: Vec<&str> = queue["pending"]
            .as_array()
            .expect("the intents of the queue")
            .iter()
            .map(|intent| intent["op"]["secret_put"]["id"].as_str().expect("the id of the intent"))
            .collect();
        assert_eq!(
            ids,
            vec![first.as_str(), second.as_str()],
            "the queue holds the secrets in the order that the frame asked for them"
        );
        drop(temp);
    }

    #[test]
    fn a_save_of_a_shared_section_without_a_server_queues_nothing_and_starts_no_child() {
        // The frame path of a save runs under the guard in strict mode, and a
        // computer without a server writes its store alone.
        horizon_core::ui_thread::use_strict_mode();
        let (temp, mut app) = test_app();
        assert!(app.server_sync.is_none(), "the settings of this window name no server");
        let mut config = app.template_config.clone();
        config.appearance.theme = horizon_core::AppearanceTheme::Light;
        let shared = horizon_core::settings_sync::changed_sections(&app.template_config, &config);
        assert_eq!(shared, vec!["appearance".to_owned()], "one shared section changed");
        app.settings_io.save_config(app.config_path.clone(), config, shared);
        app.wait_for_settings_io();

        let queue = app.machine_registry().root().join("server-queue.json");
        assert!(!queue.exists(), "a computer without a server has no queue of intents");
        assert!(
            !queue.with_file_name("server-settings.json").exists(),
            "and no mirror of settings"
        );
        assert!(app.server_sync.is_none(), "no link started");
        assert_eq!(
            app.template_config.appearance.theme,
            horizon_core::AppearanceTheme::Light,
            "the value of this computer is in its store"
        );
        drop(temp);
    }

    /// A window with a link whose child ends at once, and the directory of its
    /// queue and its mirror.
    fn test_link_app() -> (tempfile::TempDir, crate::app::HorizonApp, std::path::PathBuf) {
        let (temp, mut app) = test_app();
        let link_dir = temp.path().join("link");
        let credentials = Arc::new(horizon_core::secret::CredentialVault::new(Box::new(
            horizon_core::secret_store::SecretStore::in_memory().expect("a store in memory"),
        )));
        credentials.register(&horizon_browser::remote::RemoteBrowserConfig::default());
        let keys = horizon_core::MachineKeys::new(temp.path().join("keys"));
        let mut config =
            SupervisorConfig::for_server("server-vm", "/opt/horizon --server-link", &link_dir, test_secrets(keys))
                .expect("the settings of the link");
        config.program = "/bin/true".to_owned();
        let supervisor = Supervisor::start(config, || {}).expect("the supervisor");
        app.machine_load_rx = None;
        app.server_sync = Some(ServerSync::for_test(supervisor, credentials));
        (temp, app, link_dir)
    }

    /// The secrets of a computer whose key folder holds no value.
    fn test_secrets(keys: horizon_core::MachineKeys) -> Arc<dyn horizon_core::SecretTarget> {
        let credentials = Arc::new(horizon_core::secret::CredentialVault::new(Box::new(
            horizon_core::secret_store::SecretStore::in_memory().expect("a store in memory"),
        )));
        credentials.register(&horizon_browser::remote::RemoteBrowserConfig::default());
        let named = Arc::new(horizon_core::named_secrets::NamedSecretVault::new(Box::new(
            horizon_core::secret_store::SecretStore::in_memory().expect("a store in memory"),
        )));
        Arc::new(horizon_core::secret::SecretVault::new(keys, credentials, named))
    }

    /// The named secrets that an import of the keyring created take the path of a
    /// value that a person typed: their names enter the `secrets` section of the
    /// settings, their values stay in the secrets file, and the link queues each of
    /// them by id.
    #[test]
    fn the_secrets_of_a_network_enter_the_secrets_section_as_a_typed_name_does() {
        use horizon_core::named_secrets::SecretName;

        horizon_core::ui_thread::use_strict_mode();
        let (temp, mut app) = test_app();
        let vault = Arc::new(NamedSecretVault::new(Box::new(
            horizon_core::secret_store::SecretStore::in_memory().expect("a secrets file in memory"),
        )));
        app.adopt_named_secrets(Arc::clone(&vault));
        let name = SecretName::parse("TS_HOME_TAG").expect("the name of the recipe");
        let change = vault
            .put(&name, b"tag:horizon-worker")
            .expect("the value of the third secret");

        app.adopt_created_secrets(vec![change]);

        assert!(
            app.template_config.secrets.names.contains(&name.as_str().to_string()),
            "the name is in the secrets section: {:?}",
            app.template_config.secrets.names
        );
        let settings = serde_json::to_string(&app.template_config).expect("the settings as text");
        assert!(
            !settings.contains("tag:horizon-worker"),
            "no value of a secret enters the settings"
        );
        assert_eq!(
            vault
                .value(&name)
                .expect("the value of the secret")
                .as_deref()
                .map(Vec::as_slice),
            Some(&b"tag:horizon-worker"[..]),
            "the value is in the secrets file"
        );
        assert!(app.server_sync.is_none(), "this window follows no server");
        app.wait_for_settings_io();
        drop(temp);
    }

    /// The Server tab reads the line about a setting that another computer
    /// changed first, beside the state line of the link, and loses it with the
    /// sidebar when the person closes it.
    #[test]
    fn the_server_tab_shows_the_line_about_a_setting_of_another_computer() {
        let (_temp, mut app, _link_dir) = test_link_app();
        assert_eq!(
            app.server_view().settings,
            None,
            "nothing waits and no such line stands"
        );
        let line = "The server kept the value of another computer for the setting appearance, so this computer takes \
                    that value.";
        app.settings_lost = Some(line.to_owned());
        assert_eq!(
            app.server_view().settings.as_deref(),
            Some(line),
            "the tab reads the line under the state of the link"
        );
        app.dismiss_settings_lost();
        assert_eq!(
            app.server_view().settings,
            None,
            "the close control of the sidebar ends the line of the tab too"
        );
    }

    #[test]
    fn the_line_of_a_server_that_does_not_answer_is_away_after_the_first_attempt() {
        // The state of a link that a start leaves, and of every attempt that a
        // hello did not answer.
        let away = LinkState::Away { pending: 0 };
        assert_eq!(ServerLine::of(&away, None).text(), "Server away");
        assert_eq!(
            ServerLine::of(&LinkState::Away { pending: 2 }, None).text(),
            "Server away · 2 pending"
        );
        // Only the first attempt of a link reads `connecting`.
        assert_eq!(
            ServerLine::of(&away.clone().started(true), None).text(),
            "Server: connecting"
        );
        assert_eq!(
            ServerLine::of(&away.started(false), None).text(),
            "Server away",
            "a second attempt of a hung server does not read connecting"
        );
    }

    #[test]
    fn every_state_of_the_link_has_one_short_line() {
        assert_eq!(ServerLine::of(&LinkState::Off, None).text(), "Server: connecting");
        assert_eq!(
            ServerLine::of(&LinkState::Connecting, None).text(),
            "Server: connecting"
        );
        assert_eq!(ServerLine::of(&LinkState::Behind, None).text(), "Server: connecting");
        assert_eq!(
            ServerLine::of(&LinkState::InStep { index: 12 }, None).text(),
            "Server: in step"
        );
        assert_eq!(
            ServerLine::of(&LinkState::Away { pending: 0 }, None).text(),
            "Server away"
        );
        assert_eq!(
            ServerLine::of(&LinkState::Away { pending: 1 }, None).text(),
            "Server away · 1 pending"
        );
        assert_eq!(
            ServerLine::of(&LinkState::Away { pending: 4 }, None).text(),
            "Server away · 4 pending"
        );
        // A link that no attempt can bring back says what happened, and never that
        // it is still connecting.
        assert_eq!(
            ServerLine::of(&LinkState::Removed, None).text(),
            "This computer was removed from the server."
        );
        assert_eq!(
            ServerLine::of(&LinkState::NotLoggedIn, None).text(),
            "This computer is not logged in to the server."
        );
        for state in [
            LinkState::Removed,
            LinkState::NotLoggedIn,
            LinkState::Away { pending: 3 },
            LinkState::InStep { index: 1 },
        ] {
            assert_ne!(
                ServerLine::of(&state, None).text(),
                "Server: connecting",
                "{state:?} is not a link that is still trying"
            );
        }
    }

    #[test]
    fn the_line_follows_the_queue_of_an_away_server() {
        let quiet = ServerLine::of(&LinkState::Away { pending: 0 }, None);
        let waiting = ServerLine::of(&LinkState::Away { pending: 2 }, None);
        assert_ne!(quiet, waiting, "a change that waits changes the line");
        assert_eq!(waiting, ServerLine::of(&LinkState::Away { pending: 2 }, None));
    }

    /// B15: the line of the sidebar carries the reason of the last attempt, so a
    /// person who reads `Server away` learns whether the address answered, the name
    /// did not resolve, the key changed or the login was refused.
    #[test]
    fn the_away_line_carries_the_reason_of_the_last_attempt() {
        let away = LinkState::Away { pending: 1 };
        let line = ServerLine::of(&away, Some(LinkRefusal::HostKey));
        assert_eq!(line.text(), "Server away · 1 pending", "the short line stays short");
        assert_eq!(
            line.refusal(),
            Some("The key of the server is not the key that this computer knows."),
            "the hover and the Server tab name the reason"
        );
        for refusal in [
            LinkRefusal::Address,
            LinkRefusal::Name,
            LinkRefusal::HostKey,
            LinkRefusal::Login,
            LinkRefusal::Command,
            LinkRefusal::Silent,
            LinkRefusal::Other,
        ] {
            let line = ServerLine::of(&away, Some(refusal));
            assert_ne!(
                line.refusal(),
                ServerLine::of(&away, Some(LinkRefusal::Address))
                    .refusal()
                    .filter(|_| refusal != LinkRefusal::Address),
                "{:?} reads its own sentence",
                refusal.code()
            );
        }
        assert_eq!(
            ServerLine::of(&away, None).refusal(),
            None,
            "a link that made no failed attempt names no reason"
        );
    }
}
