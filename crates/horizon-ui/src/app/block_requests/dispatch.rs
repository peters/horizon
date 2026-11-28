//! The dispatch of one claimed request: the caller first, then the operation that
//! answers it.
//!
//! A front desk reaches the three operations of the work items and nothing else. The
//! operations of a machine, a secret name or the server answer here from the state
//! that this computer keeps in memory.

use std::path::Path;

use egui::Context;
use horizon_browser_control::manifest::block_machines;
use horizon_browser_control::manifest::block_terminal;
use horizon_core::browser::manifest::block::{self, Operation, Outcome, Request};

use super::super::HorizonApp;
use super::super::block_machine_keys;
use super::super::browser_requests::ActorPanel;
use super::{CardCaller, answer_secret_names, caller_unavailable};

impl HorizonApp {
    /// Applies one request and returns its answer, or `None` for a `next_click`
    /// the host now holds open.
    pub(super) fn apply_block_request_at(
        &mut self,
        request: &Request,
        root: Option<&Path>,
        ctx: &Context,
    ) -> Option<Outcome> {
        let caller = match self.request_caller(request) {
            Ok(caller) => caller,
            Err(refusal) => return Some(refusal),
        };
        // The front desk owns no workspace and no card, so it reaches the three
        // operations of the work items and nothing else of this tool.
        if matches!(caller, CardCaller::FrontDesk) {
            return Some(self.answer_front_desk(&request.operation, ctx));
        }
        match &request.operation {
            Operation::Create { title, document } => {
                Some(self.create_card(title.as_deref(), document, caller, &request.actor, ctx))
            }
            Operation::Update { panel_id, document } => Some(self.update_card(panel_id, document, &request.actor, ctx)),
            Operation::List => Some(Outcome::Panels {
                panels: self.owner_workspace(&request.actor).map_or_else(Vec::new, |workspace| {
                    self.workspace_card_summaries(workspace, &request.actor)
                }),
            }),
            Operation::Inspect { panel_id } => Some(self.inspect_card(panel_id, &request.actor)),
            Operation::NextClick { panel_id, .. } => self.begin_click_wait(root, request, panel_id.as_deref()),
            Operation::Visibility { panel_id, visible } => {
                Some(self.change_card(panel_id, *visible, &request.actor, ctx))
            }
            Operation::Close { panel_id } => Some(self.close_card(panel_id, &request.actor, ctx)),
            Operation::OpenTerminal {
                ssh,
                remote_command,
                title,
                key,
                machine,
            } => match caller {
                CardCaller::Panel(actor) => self.answer_open_terminal(
                    &OpenTerminalFields::of(
                        ssh.as_ref(),
                        remote_command.as_deref(),
                        title.as_deref(),
                        key.as_deref(),
                        machine.as_deref(),
                    ),
                    actor,
                    request,
                    root,
                    ctx,
                ),
                CardCaller::Orchestrator { .. } | CardCaller::Session { .. } | CardCaller::FrontDesk => {
                    Some(caller_unavailable())
                }
            },
            Operation::OpenPanel {
                preset,
                directory,
                title,
                prompt,
            } => self.answer_open_panel(
                &super::super::block_panels::OpenPanelFields {
                    preset,
                    directory,
                    title: title.as_deref(),
                    prompt: prompt.as_deref(),
                },
                request,
                root,
                ctx,
            ),
            Operation::MachineSet { .. }
            | Operation::MachineDelete { .. }
            | Operation::Machines
            | Operation::MachineKey { .. }
            | Operation::MachineKeys
            | Operation::DeleteMachineKey { .. } => self.answer_machine_request(&request.operation, request, root, ctx),
            Operation::ServerStatus => Some(self.answer_server_status()),
            Operation::BrowserHostStatus => Some(self.answer_browser_host_status()),
            Operation::SecretNames => Some(answer_secret_names()),
            Operation::Presets => Some(self.answer_presets()),
            Operation::ServerLinkRequest { .. } | Operation::BrowserHostRequest { .. } => match caller {
                CardCaller::Panel(actor) => self.answer_setting_request(&request.operation, actor, request, root, ctx),
                CardCaller::Orchestrator { .. } | CardCaller::Session { .. } | CardCaller::FrontDesk => {
                    Some(caller_unavailable())
                }
            },
            // The four operations of the workers are the ones that an orchestrator
            // of a workspace and its workers reach. The front desk has its own path
            // above, and a session of a machine is no caller of them.
            Operation::WorkerStart { program, task } => Some(self.answer_worker_start(program, task, caller, ctx)),
            Operation::Workers => Some(self.answer_workers(caller)),
            Operation::WorkerMessage { worker, text } => Some(self.answer_worker_message(worker, text, caller, ctx)),
            Operation::WorkerReport { text } => Some(self.answer_worker_report(text, caller, ctx)),
            // The three operations of the work items are the ones that the front
            // desk reaches, and its own path above answers them. Every other caller
            // gets the refusal of a caller that cannot ask for what it asks.
            Operation::WorkItems | Operation::WorkItemPropose { .. } | Operation::WorkItemMessage { .. } => {
                Some(caller_unavailable())
            }
        }
    }
    /// Answers `server_status`: how this computer reaches its server, the state of
    /// that link and the store id that this computer saw last. It reads the entry
    /// that the settings hold in memory and the snapshot of the link, so no frame
    /// waits for a file, and it writes nothing.
    fn answer_server_status(&self) -> Outcome {
        let entry = &self.template_config.server;
        let (link, store) = self.server_sync.as_ref().map_or(("off", None), |sync| {
            let store = sync.store_id();
            (sync.state_code(), store)
        });
        Outcome::ServerStatus {
            server: Box::new(block::ServerLink {
                ssh_target: entry.ssh_target.clone(),
                machine: entry.machine.clone(),
                command: entry.command.clone(),
                address: entry.address.clone(),
                link: link.to_owned(),
                store,
            }),
        }
    }

    /// Answers `browser_host_status`: the browser host that the browser panels
    /// of this computer follow. It reads the entry that the settings hold in
    /// memory and writes nothing, so no frame waits for a file, and the answer
    /// holds no key, no password and no token.
    fn answer_browser_host_status(&self) -> Outcome {
        let entry = &self.template_config.browser_host;
        let host = if entry.names_host() {
            block::BrowserHost {
                state: "host".to_owned(),
                ssh_target: entry.ssh_target().map(str::to_owned),
                machine: entry.machine().map(str::to_owned),
                profile: entry.profile().map(str::to_owned),
                engine: entry.engine().as_str().to_owned(),
                command: entry.driver_command().map(str::to_owned),
            }
        } else {
            // An entry that names no host serves the browser of this computer, so
            // no field of an entry that a person dropped reaches the answer.
            block::BrowserHost {
                state: "local".to_owned(),
                engine: horizon_core::BrowserHostEngine::default().as_str().to_owned(),
                ..block::BrowserHost::default()
            }
        };
        Outcome::BrowserHostStatus { host: Box::new(host) }
    }

    /// Answers `open_terminal`: a route of its own, or the record of a machine of
    /// this Horizon. A caller that names a machine passes none of the fields that
    /// the record holds.
    fn answer_open_terminal(
        &mut self,
        asked: &OpenTerminalFields<'_>,
        actor: ActorPanel,
        request: &Request,
        root: Option<&Path>,
        ctx: &Context,
    ) -> Option<Outcome> {
        let OpenTerminalFields {
            ssh,
            remote_command,
            title,
            key,
            machine,
        } = *asked;
        if let Err(refusal) = block_machines::check_single_target(ssh.is_some(), machine.is_some()) {
            return Some(Outcome::failed(refusal.code, &refusal.message));
        }
        if let Some(machine) = machine {
            for (field, given) in [
                ("ssh", ssh.is_some()),
                ("key", key.is_some()),
                ("remote_command", remote_command.is_some()),
            ] {
                if given {
                    return Some(Outcome::failed(
                        block_machines::INVALID_REQUEST,
                        &format!(
                            "open_terminal takes {field} from the machine record: leave {field} out when you pass machine"
                        ),
                    ));
                }
            }
            self.answer_machine_terminal(machine, title, actor, request, root, ctx);
            return None;
        }
        let Some(ssh) = ssh else {
            return Some(Outcome::failed(
                block_machines::INVALID_REQUEST,
                "open_terminal needs ssh or machine",
            ));
        };
        let terminal = match block_terminal::check(ssh, remote_command) {
            Ok(terminal) => terminal,
            Err(refusal) => return Some(Outcome::failed(refusal.code, &refusal.message)),
        };
        match key {
            None => Some(self.open_terminal(&terminal, title, None, actor, &request.actor, ctx)),
            Some(key) => {
                // The key lives in a file, so the worker checks it and the frame
                // that applies the check opens the panel.
                self.answer_key_terminal(
                    &block_machine_keys::KeyTerminal {
                        terminal: &terminal,
                        title,
                        key,
                    },
                    request,
                    root,
                    ctx,
                );
                None
            }
        }
    }

    /// The mark of a panel that an agent panel opened: the display name of the
    /// kind of that panel, which is Horizon's own text and never a string of the
    /// request.
    pub(in crate::app) fn opener_mark(&self, actor: ActorPanel) -> String {
        self.board
            .panel(actor.panel_id)
            .map_or_else(|| "Agent".to_owned(), |panel| panel.kind.display_name().to_owned())
    }

    /// The actor of the panel that opened a panel, in the same `horizon:<local
    /// id>` form that a card owner and a browser panel owner carry.
    ///
    /// The mark above names a kind, so two agent panels of one kind leave it
    /// ambiguous. The actor names the one panel that asked.
    pub(in crate::app) fn opener_actor(&self, actor: ActorPanel) -> Option<String> {
        self.board
            .panel(actor.panel_id)
            .map(|panel| horizon_core::browser_actor(&panel.local_id))
    }
}

/// The fields of one `open_terminal` request, as the dispatch reads them.
struct OpenTerminalFields<'a> {
    ssh: Option<&'a block_terminal::TerminalRoute>,
    remote_command: Option<&'a str>,
    title: Option<&'a str>,
    key: Option<&'a str>,
    machine: Option<&'a str>,
}

impl<'a> OpenTerminalFields<'a> {
    /// The fields of the operation, as borrowed values of the request.
    fn of(
        ssh: Option<&'a block_terminal::TerminalRoute>,
        remote_command: Option<&'a str>,
        title: Option<&'a str>,
        key: Option<&'a str>,
        machine: Option<&'a str>,
    ) -> Self {
        Self {
            ssh,
            remote_command,
            title,
            key,
            machine,
        }
    }
}
