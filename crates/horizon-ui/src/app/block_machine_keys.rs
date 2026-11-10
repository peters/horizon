//! The machine keys that an agent asks Horizon to keep for the machines it
//! rents, and the terminal that connects with one.
//!
//! A key is files under the Horizon home and `ssh-keygen` is a process, so no
//! frame makes, reads or deletes one: the frame resolves the caller and its
//! workspace on the board, the coordination worker does the file work and the
//! process, and the answer is written when that work is done. The private half
//! stays in the Horizon home: an answer names the two files, and the text of the
//! private key reaches no answer, log line or card.

use std::path::Path;

use egui::Context;
use horizon_browser_control::manifest::block;
use horizon_browser_control::manifest::block_keys;
use horizon_browser_control::manifest::block_terminal::CheckedTerminal;
use horizon_core::{MachineKeyError, MachineKeys};

use super::HorizonApp;
use super::block_requests::BlockAnswer;
use super::browser_requests::ActorPanel;

impl HorizonApp {
    /// The key pairs of this Horizon home.
    fn machine_keys(&self) -> MachineKeys {
        MachineKeys::from_home(self.session_store.home())
    }

    /// The identity of the caller's workspace in the key directories: its saved
    /// local id, so every agent panel of the workspace finds the same keys and
    /// another workspace finds none of them.
    fn workspace_key(&self, actor: ActorPanel) -> Option<String> {
        self.board
            .workspace(actor.workspace_id)
            .map(|workspace| workspace.local_id.clone())
    }

    /// The identity of the caller's workspace, or the answer of a request whose
    /// workspace left the board. The key names of a workspace that is gone
    /// answer nothing else, so an agent never reaches the keys of another one.
    fn caller_workspace(
        &mut self,
        actor: ActorPanel,
        request: &block::Request,
        root: Option<&Path>,
        ctx: &Context,
    ) -> Option<String> {
        let workspace = self.workspace_key(actor);
        if workspace.is_none() {
            self.answer_key_now(
                root,
                request,
                ctx,
                block::Outcome::failed(
                    "caller_unavailable",
                    "The calling agent's workspace is no longer in this host",
                ),
            );
        }
        workspace
    }

    /// Answers `machine_key`: makes the pair when this workspace has no key of
    /// that name, and answers the public line either way.
    pub(super) fn answer_machine_key(
        &mut self,
        name: &str,
        actor: ActorPanel,
        request: &block::Request,
        root: Option<&Path>,
        ctx: &Context,
    ) {
        let Some(workspace) = self.caller_workspace(actor, request, root, ctx) else {
            return;
        };
        let keys = self.machine_keys();
        let name = name.to_owned();
        self.queue_key_answer(root, request, ctx, move || {
            match keys.ensure(&workspace, &name, block_keys::MAX_MACHINE_KEYS_PER_WORKSPACE) {
                Ok(key) => block::Outcome::MachineKey {
                    name: key.name,
                    public_key: key.public_key,
                    private_key_path: key.private_key_path,
                    known_hosts_path: key.known_hosts_path,
                    created: key.created,
                },
                Err(error) => refused(&error),
            }
        });
    }

    /// Answers `machine_keys`: the pairs of the caller's workspace.
    pub(super) fn answer_machine_keys(
        &mut self,
        actor: ActorPanel,
        request: &block::Request,
        root: Option<&Path>,
        ctx: &Context,
    ) {
        let Some(workspace) = self.caller_workspace(actor, request, root, ctx) else {
            return;
        };
        let keys = self.machine_keys();
        self.queue_key_answer(root, request, ctx, move || match keys.list(&workspace) {
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

    /// Answers `delete_machine_key`: removes the pair and its known-hosts file.
    pub(super) fn answer_deleted_machine_key(
        &mut self,
        name: &str,
        actor: ActorPanel,
        request: &block::Request,
        root: Option<&Path>,
        ctx: &Context,
    ) {
        let Some(workspace) = self.caller_workspace(actor, request, root, ctx) else {
            return;
        };
        let keys = self.machine_keys();
        let name = name.to_owned();
        self.queue_key_answer(root, request, ctx, move || match keys.delete(&workspace, &name) {
            Ok(()) => block::Outcome::DeletedMachineKey { name },
            Err(error) => refused(&error),
        });
    }

    /// Opens the terminal that an `open_terminal` with a `key` asks for. The key
    /// is checked on the coordination worker, because that reads the file
    /// system, and the panel opens on the frame that applies the check.
    pub(super) fn answer_key_terminal(
        &mut self,
        asked: &KeyTerminal<'_>,
        actor: ActorPanel,
        request: &block::Request,
        root: Option<&Path>,
        ctx: &Context,
    ) {
        let Some(workspace) = self.caller_workspace(actor, request, root, ctx) else {
            return;
        };
        let keys = self.machine_keys();
        let (terminal, title) = (asked.terminal.clone(), asked.title.map(str::to_owned));
        let name = asked.key.to_owned();
        let (root, request, ctx) = (root.map(Path::to_path_buf), request.clone(), ctx.clone());
        let opening = ctx.clone();
        self.block_claim.key_job_started();
        self.browser_create_host.io.then(
            move || keys.require(&workspace, &name),
            move |app, required| {
                app.block_claim.key_job_finished();
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
        self.block_claim.key_job_started();
        self.browser_create_host.io.then(work, move |app, outcome| {
            app.block_claim.key_job_finished();
            let outcome = outcome.unwrap_or_else(stopped);
            app.write_block_answer(BlockAnswer::new(root.as_deref(), request, outcome));
            writing.request_repaint();
        });
    }

    /// Writes one answer that the frame already knows, such as a refusal that
    /// needs no file work. The write itself stays on the coordination worker.
    fn answer_key_now(
        &mut self,
        root: Option<&Path>,
        request: &block::Request,
        ctx: &Context,
        outcome: block::Outcome,
    ) {
        self.write_block_answer(BlockAnswer::new(root, request.clone(), outcome));
        ctx.request_repaint();
    }
}

/// One `open_terminal` that names a machine key of the caller's workspace.
pub(super) struct KeyTerminal<'a> {
    /// The route and the command, checked by the caller.
    pub terminal: &'a CheckedTerminal,
    pub title: Option<&'a str>,
    /// The name of the key, checked by the caller.
    pub key: &'a str,
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
