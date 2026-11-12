//! The machine keys that an agent asks Horizon to keep for the machines it
//! rents, and the terminal that connects with one.
//!
//! A key is files under the Horizon home and `ssh-keygen` is a process, so no
//! frame makes, reads or deletes one: the frame resolves the caller on the
//! board, the coordination worker does the file work and the process, and the
//! answer is written when that work is done. The keys belong to the Horizon
//! home, so every agent panel of this Horizon may use one by name. The private
//! half stays in the Horizon home: an answer names the two files, and the text
//! of the private key reaches no answer, log line or card.

use std::path::Path;

use egui::Context;
use horizon_browser_control::manifest::block;
use horizon_browser_control::manifest::block_keys;
use horizon_browser_control::manifest::block_terminal::CheckedTerminal;
use horizon_core::{MachineKeyError, MachineKeys};

use super::HorizonApp;
use super::block_requests::BlockAnswer;

impl HorizonApp {
    /// The key pairs of this Horizon home.
    fn machine_keys(&self) -> MachineKeys {
        MachineKeys::from_home(self.session_store.home())
    }

    /// Answers `machine_key`: makes the pair when this Horizon home has no key
    /// of that name, and answers the public line either way. A key that this
    /// request made goes to the server, so a second computer can log in to the
    /// same machine without another key.
    pub(super) fn answer_machine_key(
        &mut self,
        name: &str,
        request: &block::Request,
        root: Option<&Path>,
        ctx: &Context,
    ) {
        let keys = self.machine_keys();
        let name = name.to_owned();
        let link = self.link();
        self.queue_key_answer(root, request, ctx, move || {
            match keys.ensure(&name, block_keys::MAX_MACHINE_KEYS) {
                Ok(key) => {
                    if key.created {
                        share_key(link.as_ref(), &key.name);
                    }
                    block::Outcome::MachineKey {
                        name: key.name,
                        public_key: key.public_key,
                        private_key_path: key.private_key_path,
                        known_hosts_path: key.known_hosts_path,
                        created: key.created,
                    }
                }
                Err(error) => refused(&error),
            }
        });
    }

    /// Answers `machine_keys`: the pairs of this Horizon home.
    pub(super) fn answer_machine_keys(&mut self, request: &block::Request, root: Option<&Path>, ctx: &Context) {
        let keys = self.machine_keys();
        self.queue_key_answer(root, request, ctx, move || match keys.list() {
            Ok(keys) => block::Outcome::MachineKeys {
                keys: keys
                    .into_iter()
                    .map(|key| block::MachineKeySummary {
                        name: key.name,
                        public_key: key.public_key,
                        private_key_path: key.private_key_path,
                        known_hosts_path: key.known_hosts_path,
                        created_at_millis: key.created_at_millis,
                    })
                    .collect(),
            },
            Err(error) => refused(&error),
        });
    }

    /// Answers `delete_machine_key`: removes the pair and its known-hosts file,
    /// and removes the value from the server, so no computer keeps a login that
    /// this person ended.
    pub(super) fn answer_deleted_machine_key(
        &mut self,
        name: &str,
        request: &block::Request,
        root: Option<&Path>,
        ctx: &Context,
    ) {
        let keys = self.machine_keys();
        let name = name.to_owned();
        let link = self.link();
        self.queue_key_answer(root, request, ctx, move || match keys.delete(&name) {
            Ok(()) => {
                remove_key(link.as_ref(), &name);
                block::Outcome::DeletedMachineKey { name }
            }
            Err(error) => refused(&error),
        });
    }

    /// The link of this Horizon to its server, for the worker that keeps a key.
    fn link(&self) -> Option<horizon_sync::SyncHandle> {
        self.server_sync.as_ref().map(|sync| sync.handle().clone())
    }

    /// Opens the terminal that an `open_terminal` with a `key` asks for. The key
    /// is checked on the coordination worker, because that reads the file
    /// system, and the panel opens on the frame that applies the check.
    pub(super) fn answer_key_terminal(
        &mut self,
        asked: &KeyTerminal<'_>,
        request: &block::Request,
        root: Option<&Path>,
        ctx: &Context,
    ) {
        let keys = self.machine_keys();
        let (terminal, title) = (asked.terminal.clone(), asked.title.map(str::to_owned));
        let name = asked.key.to_owned();
        let (root, request, ctx) = (root.map(Path::to_path_buf), request.clone(), ctx.clone());
        let opening = ctx.clone();
        self.block_claim.worker_job_started();
        self.browser_create_host.io.then(
            move || keys.require(&name),
            move |app, required| {
                app.block_claim.worker_job_finished();
                let outcome = match app.request_caller(&request) {
                    Err(refusal) => refusal,
                    Ok(actor) => match required {
                        Some(Ok(key)) => {
                            app.open_terminal(&terminal, title.as_deref(), Some(&key), actor, &request.actor, &opening)
                        }
                        Some(Err(error)) => refused(&error),
                        None => stopped(),
                    },
                };
                app.write_block_answer(BlockAnswer::new(root.as_deref(), request, outcome));
                ctx.request_repaint();
            },
        );
    }

    /// Queues one key operation as a job of the coordination worker and writes
    /// its answer for the caller when the worker is done.
    fn queue_key_answer(
        &mut self,
        root: Option<&Path>,
        request: &block::Request,
        ctx: &Context,
        work: impl FnOnce() -> block::Outcome + Send + 'static,
    ) {
        let (root, request) = (root.map(Path::to_path_buf), request.clone());
        let writing = ctx.clone();
        self.block_claim.worker_job_started();
        self.browser_create_host.io.then(work, move |app, outcome| {
            app.block_claim.worker_job_finished();
            let outcome = outcome.unwrap_or_else(stopped);
            app.write_block_answer(BlockAnswer::new(root.as_deref(), request, outcome));
            writing.request_repaint();
        });
    }
}

/// One `open_terminal` that names a machine key of this Horizon home.
pub(super) struct KeyTerminal<'a> {
    /// The route and the command, checked by the caller.
    pub terminal: &'a CheckedTerminal,
    pub title: Option<&'a str>,
    /// The name of the key, checked by the caller.
    pub key: &'a str,
}

/// Queues the value of a key that this request made, so the other computers of
/// the person use the same pair. The queue holds the name and never the value.
fn share_key(link: Option<&horizon_sync::SyncHandle>, name: &str) {
    let Some(link) = link else {
        return;
    };
    let Ok(id) = horizon_core::SecretId::machine_key(name) else {
        return;
    };
    if let Err(error) = link.queue_secret(&id) {
        tracing::warn!(%error, id = %id, "a machine key of this computer did not enter the queue");
    }
}

/// Queues the removal of a key that this request deleted.
fn remove_key(link: Option<&horizon_sync::SyncHandle>, name: &str) {
    let Some(link) = link else {
        return;
    };
    let Ok(id) = horizon_core::SecretId::machine_key(name) else {
        return;
    };
    if let Err(error) = link.queue_secret_remove(&id) {
        tracing::warn!(%error, id = %id, "the removal of a machine key did not enter the queue");
    }
}

/// The refusal of one key error, with the code that the tool reports.
fn refused(error: &MachineKeyError) -> block::Outcome {
    block::Outcome::failed(error.code(), &error.to_string())
}

/// The answer of a worker job that ended before it answered.
fn stopped() -> block::Outcome {
    block::Outcome::failed(
        block_keys::KEY_UNAVAILABLE,
        "The Horizon worker that keeps machine keys stopped before it answered; ask again",
    )
}

#[cfg(test)]
mod tests;
