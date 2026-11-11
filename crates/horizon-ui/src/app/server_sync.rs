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

use horizon_core::machines::{MachineRegistry, MachineStore, Machines};
use horizon_core::{MachineError, ServerConfig};
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
    /// The generation of the records that the app read last.
    generation: u64,
    /// A read of the list is on the coordination worker.
    reloading: bool,
}

impl ServerSync {
    fn new(supervisor: Supervisor) -> Self {
        let generation = supervisor.handle().generation();
        Self {
            supervisor,
            generation,
            reloading: false,
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
}

/// Reads the machine list of this start and starts its link. It runs on a worker
/// thread, because it reads files and starts one process.
pub(in crate::app) fn load(
    registry: MachineRegistry,
    server: Option<ServerConfig>,
    wake: impl Fn() + Send + Sync + 'static,
) -> Result<MachineLoad, MachineError> {
    let Some(server) = server.filter(ServerConfig::names_server) else {
        return Machines::load(registry).map(|machines| MachineLoad {
            machines,
            server: None,
            server_failure: None,
        });
    };
    let config = match SupervisorConfig::for_server(
        server.ssh_target().unwrap_or_default(),
        server.command(),
        registry.root(),
    ) {
        Ok(config) => config,
        Err(error) => return file_list(registry, error.to_string()),
    };
    match Supervisor::start(config, wake) {
        Ok(supervisor) => {
            let sync = ServerSync::new(supervisor);
            match Machines::from_store(&registry, sync.store()) {
                Ok(machines) => Ok(MachineLoad {
                    machines,
                    server: Some(sync),
                    server_failure: None,
                }),
                // The list of the link cannot be read, so the file of this
                // computer is used and the link stops with it.
                Err(error) => file_list(registry, error.to_string()),
            }
        }
        Err(error) => file_list(registry, error.to_string()),
    }
}

/// The machine list of this computer alone, and the reason that the link to the
/// server did not start.
fn file_list(registry: MachineRegistry, reason: String) -> Result<MachineLoad, MachineError> {
    tracing::error!(%reason, "the link to the Horizon server did not start");
    Machines::load(registry).map(|machines| MachineLoad {
        machines,
        server: None,
        server_failure: Some(reason),
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

    /// Reads the machine list again when the link brought a change, on the
    /// coordination worker, so no frame reads a file of this home.
    pub(super) fn poll_server_sync(&mut self) {
        let Some(sync) = self.server_sync.as_ref() else {
            return;
        };
        let generation = sync.handle().generation();
        if generation == sync.generation || sync.reloading {
            return;
        }
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
