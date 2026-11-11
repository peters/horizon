//! The machines that agents register with Horizon, and the list that the
//! sidebar and the `machines` operation read.
//!
//! A record is one line of a JSON file under the Horizon home, so no frame
//! reads or writes it: the frame checks the request, the coordination worker
//! checks the key that the record names and rewrites the file, and the frame
//! that applies the answer replaces the list in memory. A click on a sidebar row
//! and an agent's `open_terminal { machine }` reach the same panel, because both
//! build the terminal from the same record.

use std::path::Path;

use egui::Context;
use horizon_browser_control::manifest::block;
use horizon_browser_control::manifest::block_terminal::TerminalRoute;
use horizon_core::{MachineError, MachineRegistry, Machines, browser::manifest};

use super::HorizonApp;
use super::block_requests::BlockAnswer;

impl HorizonApp {
    /// The registry file of this Horizon home.
    pub(in crate::app) fn machine_registry(&self) -> MachineRegistry {
        MachineRegistry::from_home(self.session_store.home())
    }

    /// Reads the machine list of this Horizon home once, on its own thread at
    /// startup, so no frame reads the file and no frame starts the child of a
    /// link. A start that names a server starts the link of that server in the
    /// same thread. The list is empty until the read answers, a few
    /// milliseconds later.
    pub(super) fn load_machines(&mut self, ctx: &Context) {
        let registry = self.machine_registry();
        let server = horizon_core::server().cloned();
        let wake = {
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(super::server_sync::load(registry, server, wake));
        });
        self.machine_load_rx = Some(rx);
    }

    /// Applies the startup read when it answers. A list that a write already
    /// replaced is kept: that write holds what the file has.
    pub(super) fn poll_machine_load(&mut self) {
        let Some(rx) = self.machine_load_rx.take() else {
            return;
        };
        let asked_at = self.machine_epoch;
        match rx.try_recv() {
            Ok(Ok(load)) if asked_at == self.machine_epoch => {
                tracing::info!(count = load.machines.records().len(), "loaded the machine list");
                self.machine_list = load.machines;
                if let Some(reason) = load.server_failure {
                    self.set_sidebar_notice(format!("The machine list: {reason}"));
                }
                self.server_sync = load.server;
            }
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                tracing::warn!(%error, "could not load the machine list");
                self.set_sidebar_notice(error.to_string());
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => self.machine_load_rx = Some(rx),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                tracing::warn!("the machine list read ended before it answered");
            }
        }
    }

    /// Marks the machine list as written by this host, so a read that was asked
    /// for before the write does not replace it.
    fn machine_written(&mut self) {
        self.machine_epoch = self.machine_epoch.wrapping_add(1);
    }

    /// Answers `machine_set`: checks the record, makes sure that the key it
    /// names is there, and writes the list.
    pub(super) fn answer_machine_set(
        &mut self,
        asked: &MachineSetFields<'_>,
        request: &block::Request,
        root: Option<&Path>,
        ctx: &Context,
    ) {
        let MachineSetFields {
            name,
            ssh,
            key,
            session,
            provider,
        } = *asked;
        let checked = horizon_core::machines::check(name, ssh, session, provider).and_then(|checked| {
            let key = key.map(horizon_core::machines::machine_key_name).transpose()?;
            Ok((checked, key))
        });
        let (checked, key) = match checked {
            Ok(checked) => checked,
            Err(error) => {
                self.write_block_answer(BlockAnswer::new(root, request.clone(), refused(&error)));
                ctx.request_repaint();
                return;
            }
        };
        let record = checked.record(u64::try_from(manifest::now_millis()).unwrap_or(0));
        let keys = horizon_core::MachineKeys::from_home(self.session_store.home());
        let registry = self.machine_registry();
        let store = self.machine_store(&registry);
        let name = record.name.clone();
        self.queue_machine_answer(
            request,
            root,
            ctx,
            move || {
                if let Some(key) = key.as_deref() {
                    keys.require(key)?;
                }
                let (next, created) = Machines::from_store(&registry, store)?.with_record(record, key.as_deref())?;
                next.save()?;
                Ok::<_, MachineError>((next, created))
            },
            move |app, answer| match answer {
                Ok((next, created)) => {
                    app.machine_list = next;
                    app.machine_written();
                    block::Outcome::MachineSet { name, created }
                }
                Err(error) => refused(&error),
            },
        );
    }

    /// Answers `machine_delete`: forgets one record and keeps its key.
    pub(super) fn answer_machine_delete(
        &mut self,
        name: &str,
        request: &block::Request,
        root: Option<&Path>,
        ctx: &Context,
    ) {
        if let Err(error) = horizon_core::machines::machine_name(name) {
            self.write_block_answer(BlockAnswer::new(root, request.clone(), refused(&error)));
            ctx.request_repaint();
            return;
        }
        let registry = self.machine_registry();
        let store = self.machine_store(&registry);
        let asked = name.to_owned();
        let answer_name = asked.clone();
        self.queue_machine_answer(
            request,
            root,
            ctx,
            move || {
                let machines = Machines::from_store(&registry, store)?;
                if machines.find(&asked).is_none() {
                    return Err(MachineError::Unavailable {
                        reason: format!("This Horizon has no machine {asked}; call machines to list the ones it has"),
                    });
                }
                let next = machines.without_record(&asked);
                next.save()?;
                Ok::<_, MachineError>((next, ()))
            },
            move |app, answer| match answer {
                Ok((next, ())) => {
                    app.machine_list = next;
                    app.machine_written();
                    block::Outcome::MachineDeleted { name: answer_name }
                }
                Err(error) => refused(&error),
            },
        );
    }

    /// Answers `machines`: every record of this Horizon, in name order, with
    /// whether a terminal of it is open now.
    pub(super) fn answer_machines(&mut self, request: &block::Request, root: Option<&Path>, ctx: &Context) {
        let outcome = block::Outcome::Machines {
            machines: self.machine_summaries(),
            server: self.server_sync.as_ref().map(|sync| sync.state_code().to_owned()),
        };
        self.write_block_answer(BlockAnswer::new(root, request.clone(), outcome));
        ctx.request_repaint();
    }

    /// Forgets one machine from its sidebar menu: the record goes, its key stays,
    /// and a terminal that is already open keeps its own saved connection.
    pub(in crate::app) fn forget_machine(&mut self, name: &str, ctx: &Context) {
        let registry = self.machine_registry();
        let store = self.machine_store(&registry);
        let forgetting = name.to_owned();
        let notice_name = forgetting.clone();
        let writing = ctx.clone();
        self.browser_create_host.io.then(
            move || {
                let machines = Machines::from_store(&registry, store)?;
                let next = machines.without_record(&forgetting);
                if next.records().len() == machines.records().len() {
                    return Err(MachineError::Unavailable {
                        reason: format!("This Horizon has no machine {forgetting} anymore"),
                    });
                }
                next.save()?;
                Ok(next)
            },
            move |app, forgotten| match forgotten {
                Some(Ok(next)) => {
                    app.machine_list = next;
                    app.machine_written();
                    tracing::info!(machine = %notice_name, "forgot a machine");
                }
                Some(Err(error)) => app.set_sidebar_notice(format!("{notice_name}: {error}")),
                None => {}
            },
        );
        writing.request_repaint();
    }

    /// The machines of this Horizon as `machines` reports them.
    pub(in crate::app) fn machine_summaries(&self) -> Vec<block::MachineSummary> {
        let pending = self.pending_machines();
        self.machine_list
            .records()
            .iter()
            .map(|machine| block::MachineSummary {
                pending: pending.contains(machine.name.as_str()),
                name: machine.name.clone(),
                host: machine.host.clone(),
                user: machine.user.clone(),
                port: machine.port,
                key: self.machine_list.key(&machine.name).map(str::to_owned),
                session: machine.session.clone(),
                provider: machine.provider.clone(),
                added_at_millis: machine.added_at_millis,
                terminal_open: self.machine_panel(&machine.name).is_some(),
            })
            .collect()
    }

    /// The machines that have an intent which the server has not decided.
    pub(in crate::app) fn pending_machines(&self) -> std::collections::BTreeSet<String> {
        self.server_sync
            .as_ref()
            .map(|sync| sync.handle().pending())
            .unwrap_or_default()
    }

    /// The frame of a terminal that this machine opened, in any workspace.
    pub(in crate::app) fn machine_panel(&self, name: &str) -> Option<horizon_core::PanelId> {
        self.board
            .panels
            .iter()
            .find(|panel| panel.machine() == Some(name))
            .map(|panel| panel.id)
    }

    /// Queues one machine-list job on the coordination worker and writes its
    /// answer for the caller when the worker is done.
    fn queue_machine_answer<T: Send + 'static>(
        &mut self,
        request: &block::Request,
        root: Option<&Path>,
        ctx: &Context,
        work: impl FnOnce() -> Result<(Machines, T), MachineError> + Send + 'static,
        answer: impl FnOnce(&mut HorizonApp, Result<(Machines, T), MachineError>) -> block::Outcome + Send + 'static,
    ) {
        let (root, request) = (root.map(Path::to_path_buf), request.clone());
        let writing = ctx.clone();
        self.block_claim.key_job_started();
        self.browser_create_host.io.then(work, move |app, worked| {
            app.block_claim.key_job_finished();
            let outcome = match worked {
                Some(worked) => answer(app, worked),
                None => stopped(),
            };
            app.write_block_answer(BlockAnswer::new(root.as_deref(), request, outcome));
            writing.request_repaint();
        });
    }
}

/// The fields of one `machine_set` request, as the dispatch reads them.
pub(in crate::app) struct MachineSetFields<'a> {
    pub(in crate::app) name: &'a str,
    pub(in crate::app) ssh: &'a TerminalRoute,
    pub(in crate::app) key: Option<&'a str>,
    pub(in crate::app) session: Option<&'a str>,
    pub(in crate::app) provider: Option<&'a str>,
}

/// The refusal of one machine error, with the code that the tool reports.
pub(in crate::app) fn refused(error: &MachineError) -> block::Outcome {
    block::Outcome::failed(error.code(), &error.to_string())
}

/// The answer of a worker job that ended before it answered.
fn stopped() -> block::Outcome {
    block::Outcome::failed(
        horizon_core::machines::MACHINE_UNAVAILABLE,
        "The Horizon worker that keeps the machine list stopped before it answered; call machines again",
    )
}

/// Waits for the startup read of the machine list, so a test that writes a
/// record is not overwritten by it.
#[cfg(test)]
pub(in crate::app) fn settle_machine_load(app: &mut HorizonApp) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.machine_load_rx.is_some() && std::time::Instant::now() < deadline {
        app.poll_machine_load();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(app.machine_load_rx.is_none(), "the machine list did not load");
}

#[cfg(test)]
mod tests;
