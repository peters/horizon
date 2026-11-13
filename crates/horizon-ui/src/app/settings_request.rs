//! The two settings that an agent may ask for and only the person confirms.
//!
//! The server target and the browser host decide where the machine list, the
//! keys and the settings of the person go, so an agent states the change and the
//! person presses the button: `server_link_request` and `browser_host_request`
//! draw a card of Horizon's own, in the workspace of the calling agent, and the
//! request stays open until the person presses one of its two buttons or five
//! minutes pass. The agent reads `confirmed`, `declined` or `timed_out` and has
//! no way to press the card itself.
//!
//! The card is Horizon's, not the agent's: its words are constants here, its
//! buttons are the host's controls, and a press reaches [`HorizonApp`] and no
//! agent. The request names the two fields, so the card shows them exactly as the
//! store will hold them, and the fields pass the same rule as the fields of the
//! Settings window before any card is drawn.
//!
//! The store write runs on the settings worker, as the save of the Settings
//! window does, so no frame waits for a disk. A confirmed server entry also
//! starts the link of it now, as a join does; the browser host entry applies at
//! the next start, as the Settings window states.

use std::path::{Path, PathBuf};

use egui::Context;
use horizon_browser_control::block::{BlockDocument, Button, Component, KvRow, Tone};
use horizon_browser_control::manifest::block::{
    Answer, BrowserEngine, EntryStart, HOST_APPLY_TIMEOUT, Operation, Outcome, Request, ServerLinkState,
};
use horizon_browser_control::manifest::block_machines;
use horizon_browser_control::manifest::now_millis;
use horizon_core::{
    BlockPanelState, BrowserHostConfig, Config, DEFAULT_LINK_COMMAND, PanelId, PanelKind, PanelOptions, ServerConfig,
};
use horizon_sync::LinkState;

use super::HorizonApp;
use super::block_requests::BlockAnswer;
use super::browser_requests::ActorPanel;
use super::server_sync::ServerLine;

/// The name of the card that asks about the server, as its titlebar and its
/// title show it.
pub(in crate::app) const SERVER_TITLE: &str = "Horizon server";
/// The name of the card that asks about the browser host.
pub(in crate::app) const BROWSER_TITLE: &str = "Browser host";
/// The id of the button that applies the change that the card states.
pub(in crate::app) const APPLY: &str = "apply";
/// The id of the button that keeps the setting of this computer as it is.
pub(in crate::app) const DECLINE: &str = "decline";

/// Refusal code of a second setting request while one waits for the person.
pub(in crate::app) const REQUEST_PENDING: &str = "request_pending";
/// Refusal code of an engine that a browser host does not run.
pub(in crate::app) const ENGINE_UNAVAILABLE: &str = "engine_unavailable";

/// The line under the button row of the server card, after the two statements
/// of what the link changes.
pub(in crate::app) const SERVER_SENT: &str = "Your machine list, the machine keys and your settings are sent there.";
/// The second line of the server card: what Horizon does before the first link.
pub(in crate::app) const SERVER_KEPT: &str = "The settings of this computer are kept aside in a dated file first.";
/// The line of the server card that says which login the link uses when the server
/// runs on a machine of the list.
pub(in crate::app) const SERVER_MACHINE: &str =
    "The link runs on that machine with its address, its user and the machine key that a terminal of it uses.";
/// How long the answer of a confirmed server entry waits for the first answer of
/// the server. The hello of a link holds the SSH connect and one round trip, so a
/// server that is away has ended its first attempt inside this bound.
pub(in crate::app) const LINK_ANSWER_BOUND: std::time::Duration = std::time::Duration::from_secs(20);
/// The line of the browser card that says which profile of which browser the new
/// panels use, in the words of the Settings window.
pub(in crate::app) const BROWSER_USES: &str =
    "Browser panels open in that profile of Chromium on that machine, over SSH.";
/// The line of the browser card that says when the entry applies.
pub(in crate::app) const BROWSER_NEXT_START: &str = "The change applies at the next start of Horizon.";

/// The server that the card asks about, as the request named it and as the store
/// will hold it: an SSH target of the person, or a machine of the machine list.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ServerEntry {
    /// A target that the SSH configuration of the person resolves.
    Target(String),
    /// A machine of the list, whose own login the link uses.
    Machine(String),
}

impl ServerEntry {
    /// The name of the entry as the card states it and the settings store it.
    fn label(&self) -> &str {
        match self {
            Self::Target(target) => target,
            Self::Machine(machine) => machine,
        }
    }
}

/// The change that waits for the person's press, as the request named it and as
/// the store will hold it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Asked {
    /// The server entry and the command that the link runs there.
    ServerLink { entry: ServerEntry, command: String },
    /// The SSH target and the profile of the browser host entry.
    BrowserHost { ssh_target: String, profile: String },
}

/// One card of Horizon that asks the person about a setting, and the request that
/// waits for the answer.
struct Waiting {
    /// The card that asks.
    panel: PanelId,
    /// The queue that the answer goes to, when a frame answers it.
    root: Option<PathBuf>,
    /// The request to answer.
    request: Request,
    /// The change that the person confirms with [`APPLY`].
    asked: Asked,
    /// When the card goes away without a press, in milliseconds after the epoch.
    deadline_at_millis: i64,
}

/// The answer of a confirmed server entry that waits for the first answer of the
/// server.
struct Linking {
    /// The queue that the answer goes to.
    root: Option<PathBuf>,
    /// The request to answer.
    request: Request,
    /// When the person pressed, so the bound of the wait counts from the press.
    pressed_at: std::time::Instant,
}

/// The setting request that this host holds open, if any.
#[derive(Default)]
pub(in crate::app) struct SettingRequestState {
    waiting: Option<Waiting>,
    /// A confirmed server entry whose answer waits for the link.
    linking: Option<Linking>,
    /// Why the link of the running program did not start at all, when the read
    /// that starts it answered that. A confirmed server entry answers it as the
    /// reason of a link that is away.
    pub(in crate::app) link_failure: Option<String>,
}

impl SettingRequestState {
    /// Whether a card of Horizon waits for the person's press or an answer waits
    /// for the link, so the request pump keeps waking until the wait ends.
    pub(in crate::app) fn is_waiting(&self) -> bool {
        self.waiting.is_some() || self.linking.is_some()
    }
}

impl HorizonApp {
    /// Answers `server_link_request` or `browser_host_request`: checks the fields
    /// as the settings field checks them, draws the card of Horizon that states
    /// the change, and holds the request open until the person presses it.
    ///
    /// `None` means the host keeps the request: the card that asks is on the
    /// board, and a press or the deadline answers it.
    pub(super) fn answer_setting_request(
        &mut self,
        operation: &Operation,
        actor: ActorPanel,
        request: &Request,
        root: Option<&Path>,
        ctx: &Context,
    ) -> Option<Outcome> {
        if self.settings_request.is_waiting() {
            return Some(Outcome::failed(
                REQUEST_PENDING,
                "This Horizon already asks the person about a setting; wait for that card to be answered or closed",
            ));
        }
        let (asked, title, document) = match card_of(operation) {
            Ok(card) => card,
            Err(refusal) => return Some(refusal),
        };
        match self.draw_setting_card(actor, title, document, ctx) {
            Ok(panel) => {
                self.settings_request.waiting = Some(Waiting {
                    panel,
                    root: root.map(Path::to_path_buf),
                    request: request.clone(),
                    asked,
                    // The host keeps a window to write the answer inside the
                    // deadline of the request itself.
                    deadline_at_millis: request
                        .deadline_at_millis
                        .saturating_sub(i64::try_from(HOST_APPLY_TIMEOUT.as_millis()).unwrap_or(i64::MAX)),
                });
                None
            }
            Err(refusal) => Some(refusal),
        }
    }

    /// Draws the card that asks the question, beside the calling agent panel and
    /// without taking the keyboard from it.
    fn draw_setting_card(
        &mut self,
        actor: ActorPanel,
        title: &str,
        document: BlockDocument,
        ctx: &Context,
    ) -> Result<PanelId, Outcome> {
        let size = crate::block_widget::default_card_size(&document);
        let state = BlockPanelState {
            title: title.to_owned(),
            document,
            revision: 1,
            // Horizon wrote this document, so its buttons are the host's
            // controls and no agent reads a press on them.
            owner_actor: None,
            updated_at_millis: now_millis(),
        };
        let options = PanelOptions {
            kind: PanelKind::Block,
            block: Some(state),
            name: Some(title.to_owned()),
            size: Some(size),
            ..PanelOptions::default()
        };
        let focused = self.board.focused;
        let panel = self
            .create_panel_with_options_beside(options, actor.workspace_id, Some(actor.panel_id))
            .map_err(|error| {
                Outcome::failed(
                    "panel_unavailable",
                    &format!("Horizon could not draw the card of this setting in this workspace: {error}"),
                )
            })?;
        // The question must not take the keyboard from the panel the person uses.
        if let Some(focused) = focused {
            self.board.focus(focused);
        }
        self.mark_runtime_dirty();
        ctx.request_repaint();
        Ok(panel)
    }

    /// A press on a card that Horizon drew to ask about a setting. Returns
    /// whether the card belongs to the request that waits, so a press on any
    /// other card of the host reaches its own code.
    pub(in crate::app) fn press_setting_request(&mut self, ctx: &Context, panel: PanelId, id: &str) -> bool {
        let ours = self
            .settings_request
            .waiting
            .as_ref()
            .is_some_and(|waiting| waiting.panel == panel);
        if !ours || !matches!(id, APPLY | DECLINE) {
            return false;
        }
        let Some(waiting) = self.settings_request.waiting.take() else {
            return false;
        };
        self.remove_setting_card(&waiting, ctx);
        if id == APPLY {
            self.apply_setting(&waiting, ctx);
        } else {
            self.answer_setting(&waiting, unconfirmed_outcome(&waiting.asked, Answer::Declined));
        }
        self.mark_runtime_dirty();
        ctx.request_repaint();
        true
    }

    /// Writes the answer of one setting request now.
    fn answer_setting(&mut self, waiting: &Waiting, outcome: Outcome) {
        self.write_block_answer(BlockAnswer::new(
            waiting.root.as_deref(),
            waiting.request.clone(),
            outcome,
        ));
    }

    /// Writes the setting that the person confirmed, as the Settings window
    /// writes it. A refusal here is the answer of the request: the fields passed
    /// their rule when the card was drawn, so a refusal names a setting of the
    /// store that the new entry contradicts.
    ///
    /// A confirmed server entry answers later: the card told the person that the
    /// server receives the list, so the answer says whether the link reached it.
    fn apply_setting(&mut self, waiting: &Waiting, ctx: &Context) {
        let refused = |message: String| Outcome::failed(block_machines::INVALID_REQUEST, &message);
        match &waiting.asked {
            Asked::ServerLink { entry, command } => match self.write_server_entry(entry, command, ctx) {
                Ok(start) => {
                    self.settings_request.linking = Some(Linking {
                        root: waiting.root.clone(),
                        request: waiting.request.clone(),
                        pressed_at: std::time::Instant::now(),
                    });
                    tracing::info!(start = ?start, "a confirmed server entry wrote the settings and started its link");
                }
                Err(message) => self.answer_setting(waiting, refused(message)),
            },
            Asked::BrowserHost { ssh_target, profile } => {
                let outcome = self
                    .write_browser_host_entry(ssh_target, profile)
                    .map_or_else(refused, |()| Outcome::BrowserHost {
                        answer: Answer::Confirmed,
                    });
                self.answer_setting(waiting, outcome);
            }
        }
    }

    /// Writes the server entry of this computer and starts the link of it now, as
    /// a join does. The entry and the command are the two fields that the card
    /// stated; every other field of the store stays as it holds it.
    fn write_server_entry(&mut self, entry: &ServerEntry, command: &str, ctx: &Context) -> Result<EntryStart, String> {
        let before = self.stored_config();
        let mut stored = before.clone();
        let (ssh_target, machine) = match entry {
            ServerEntry::Target(target) => (Some(target.clone()), None),
            ServerEntry::Machine(machine) => (None, Some(machine.clone())),
        };
        stored.server = ServerConfig {
            ssh_target,
            machine,
            command: Some(command.to_owned()),
            address: stored.server.address.clone(),
        };
        stored.validate().map_err(|error| error.to_string())?;
        self.save_setting(&before, stored);
        self.restart_server_link(ctx);
        Ok(EntryStart::Linked)
    }

    /// Writes `browser_host` of this computer, which the browser panels read at
    /// the next start of Horizon, as the Settings window states.
    fn write_browser_host_entry(&mut self, ssh_target: &str, profile: &str) -> Result<(), String> {
        let before = self.stored_config();
        let mut stored = before.clone();
        stored.browser_host = BrowserHostConfig {
            ssh_target: Some(ssh_target.to_owned()),
            profile: Some(profile.to_owned()),
        };
        stored.validate().map_err(|error| error.to_string())?;
        self.save_setting(&before, stored);
        Ok(())
    }

    /// Queues the store write of a confirmed setting on the settings worker, and
    /// applies what can apply now to this run.
    fn save_setting(&mut self, before: &Config, stored: Config) {
        // The sections that this write changes, so the worker queues one intent
        // per shared section after the store is written. The server entry stays
        // with this computer, so it queues none.
        let shared = horizon_core::settings_sync::changed_sections(before, &stored);
        self.apply_runtime_config(&stored);
        self.settings_io.save_config(self.config_path.clone(), stored, shared);
    }

    /// The settings that the store holds, as the Settings window keeps its
    /// baseline: the saved values, not a draft that the person did not save.
    fn stored_config(&self) -> Config {
        self.settings
            .as_ref()
            .map_or_else(|| self.template_config.clone(), |editor| editor.original.clone())
    }

    /// Removes the card that asked, because the person pressed it.
    fn remove_setting_card(&mut self, waiting: &Waiting, ctx: &Context) {
        let focused = self.board.focused == Some(waiting.panel);
        let _ = self.close_panel_returning_teardown(waiting.panel);
        if focused && let Some(caller) = super::browser_requests::actor_panel(&self.board, &waiting.request.actor) {
            self.board.focus(caller.panel_id);
        }
        ctx.request_repaint();
    }

    /// Answers a confirmed server entry when the link of it knows whether the server
    /// answered: `in_step` when it did, and `away` with the reason of the link in
    /// words when it did not, or when the bound of the wait passed.
    ///
    /// A frame reads the published snapshot of the link and never the lock of its
    /// state, so no frame waits for the link. The link itself runs on its worker, so
    /// the wait costs this thread no blocking call.
    pub(super) fn settle_link_answer(&mut self) {
        let Some(linking) = self.settings_request.linking.as_ref() else {
            return;
        };
        let expired = linking.pressed_at.elapsed() >= LINK_ANSWER_BOUND;
        let (link, reason) = match self.server_sync.as_ref() {
            Some(sync) => {
                let snapshot = sync.snapshot();
                match snapshot.state() {
                    LinkState::InStep { .. } => (Some(ServerLinkState::InStep), None),
                    LinkState::Removed | LinkState::NotLoggedIn => (
                        Some(ServerLinkState::Away),
                        Some(ServerLine::of(snapshot.state(), snapshot.refusal()).text()),
                    ),
                    // The first attempt of this link failed, or the bound passed
                    // while it was still connecting.
                    _ => match snapshot.refusal() {
                        Some(refusal) => (Some(ServerLinkState::Away), Some(refusal.sentence().to_owned())),
                        None if expired => (
                            Some(ServerLinkState::Away),
                            Some(format!(
                                "The server did not answer within {} seconds.",
                                LINK_ANSWER_BOUND.as_secs()
                            )),
                        ),
                        None => return,
                    },
                }
            }
            // The link did not start at all: the read that starts it answered the
            // reason in words, which the sidebar already shows.
            None => match self.settings_request.link_failure.clone() {
                Some(reason) => (Some(ServerLinkState::Away), Some(reason)),
                None if expired => (
                    Some(ServerLinkState::Away),
                    Some(format!(
                        "The server did not answer within {} seconds.",
                        LINK_ANSWER_BOUND.as_secs()
                    )),
                ),
                None => return,
            },
        };
        let Some(linking) = self.settings_request.linking.take() else {
            return;
        };
        let outcome = Outcome::ServerLink {
            answer: Answer::Confirmed,
            start: Some(EntryStart::Linked),
            link,
            reason,
        };
        self.write_block_answer(BlockAnswer::new(linking.root.as_deref(), linking.request, outcome));
        self.mark_runtime_dirty();
    }

    /// Answers the card that waited when its deadline passed, or when the person
    /// closed it without a press. A wait that no person answered is `timed_out`,
    /// and the card goes with it.
    pub(super) fn settle_setting_requests(&mut self, ctx: &Context) -> bool {
        let Some(waiting) = self.settings_request.waiting.as_ref() else {
            return false;
        };
        let open = self.board.panel(waiting.panel).is_some();
        let expired = waiting.deadline_at_millis <= now_millis();
        if open && !expired {
            return false;
        }
        let Some(waiting) = self.settings_request.waiting.take() else {
            return false;
        };
        if open {
            let _ = self.close_panel_returning_teardown(waiting.panel);
        }
        self.write_block_answer(BlockAnswer::new(
            waiting.root.as_deref(),
            waiting.request,
            unconfirmed_outcome(&waiting.asked, Answer::TimedOut),
        ));
        self.mark_runtime_dirty();
        ctx.request_repaint();
        true
    }
}

/// The answer of a setting request that no press confirmed. A confirmed request
/// writes the setting first and builds its own answer, so the link of the running
/// program is never claimed here.
fn unconfirmed_outcome(asked: &Asked, answer: Answer) -> Outcome {
    match asked {
        Asked::ServerLink { .. } => Outcome::ServerLink {
            answer,
            start: None,
            link: None,
            reason: None,
        },
        Asked::BrowserHost { .. } => Outcome::BrowserHost { answer },
    }
}

/// The change that one request names, its card title and its card, or the
/// refusal that the caller reads.
///
/// Both fields pass the rule of the settings field that stores them before a card
/// exists, and the card shows the values that the store will hold.
fn card_of(operation: &Operation) -> Result<(Asked, &'static str, BlockDocument), Outcome> {
    match operation {
        Operation::ServerLinkRequest {
            ssh_target,
            machine,
            command,
        } => {
            let target = ssh_target.as_deref().map(str::trim).filter(|target| !target.is_empty());
            let named = machine.as_deref().map(str::trim).filter(|machine| !machine.is_empty());
            let (ssh_target, machine_name) = match (target, named) {
                (Some(target), None) => (Some(target), None),
                (None, Some(machine)) => (None, Some(machine)),
                (Some(_), Some(_)) => {
                    return Err(Outcome::failed(
                        block_machines::INVALID_REQUEST,
                        "A server link request names one server: give `machine` for a machine of the list, or `ssh_target` for a computer of the person, and not both",
                    ));
                }
                (None, None) => {
                    return Err(Outcome::failed(
                        block_machines::INVALID_REQUEST,
                        "A server link request names no server: give `machine` with the name of a machine of the list, or `ssh_target` with an SSH target",
                    ));
                }
            };
            let command = command.as_deref().map_or(DEFAULT_LINK_COMMAND, str::trim);
            let entry = ServerConfig {
                ssh_target: ssh_target.map(str::to_owned),
                machine: machine_name.map(str::to_owned),
                command: Some(command.to_owned()),
                address: None,
            };
            entry
                .validate()
                .map_err(|error| Outcome::failed(block_machines::INVALID_REQUEST, &error.to_string()))?;
            let entry = match machine_name {
                Some(machine) => ServerEntry::Machine(machine.to_owned()),
                None => ServerEntry::Target(ssh_target.unwrap_or_default().to_owned()),
            };
            let document = server_document(&entry, command);
            Ok((
                Asked::ServerLink {
                    entry,
                    command: command.to_owned(),
                },
                SERVER_TITLE,
                document,
            ))
        }
        Operation::BrowserHostRequest {
            ssh_target,
            profile,
            engine,
        } => {
            let target = ssh_target.trim();
            let profile = profile.trim();
            if engine.is_some_and(|engine| engine != BrowserEngine::Chromium) {
                return Err(Outcome::failed(
                    ENGINE_UNAVAILABLE,
                    "A browser host runs Chromium only. Firefox on another machine is not built yet: set up the browser on Chromium, or use a Firefox of this computer.",
                ));
            }
            let entry = BrowserHostConfig {
                ssh_target: Some(target.to_owned()),
                profile: Some(profile.to_owned()),
            };
            entry
                .validate()
                .map_err(|error| Outcome::failed(block_machines::INVALID_REQUEST, &error.to_string()))?;
            let asked = Asked::BrowserHost {
                ssh_target: target.to_owned(),
                profile: profile.to_owned(),
            };
            Ok((asked, BROWSER_TITLE, browser_document(target, profile)))
        }
        _ => Err(Outcome::failed(
            block_machines::INVALID_REQUEST,
            "This operation asks no setting of this computer",
        )),
    }
}

/// The card that asks the person to link this computer to a Horizon server, with
/// the entry and the command exactly as the settings store them.
///
/// A server on a machine of the list states the machine, and the line that says
/// which login the link uses: the machine holds the machine key of Horizon, so the
/// person cannot reach it with their own SSH configuration.
fn server_document(entry: &ServerEntry, command: &str) -> BlockDocument {
    let mut rows = vec![KvRow {
        label: match entry {
            ServerEntry::Target(_) => "Server target".to_owned(),
            ServerEntry::Machine(_) => "Machine".to_owned(),
        },
        value: entry.label().to_owned(),
        mono: true,
    }];
    rows.push(KvRow {
        label: "Command".to_owned(),
        value: command.to_owned(),
        mono: true,
    });
    let mut components = vec![
        Component::Heading {
            text: match entry {
                ServerEntry::Target(target) => format!("Link this computer to the Horizon server on {target}?"),
                ServerEntry::Machine(machine) => {
                    format!("Link this computer to the Horizon server on the machine {machine}?")
                }
            },
            subtitle: None,
            badge: None,
        },
        Component::Kv { rows },
    ];
    if matches!(entry, ServerEntry::Machine(_)) {
        components.push(Component::Text {
            text: SERVER_MACHINE.to_owned(),
        });
    }
    let rest = BlockDocument {
        components: vec![
            Component::Text {
                text: SERVER_SENT.to_owned(),
            },
            Component::Text {
                text: SERVER_KEPT.to_owned(),
            },
            Component::Buttons {
                items: vec![
                    Button {
                        id: APPLY.to_owned(),
                        label: "Link".to_owned(),
                        tone: Tone::Primary,
                    },
                    Button {
                        id: DECLINE.to_owned(),
                        label: "Not now".to_owned(),
                        tone: Tone::Neutral,
                    },
                ],
            },
        ],
    };
    components.extend(rest.components);
    BlockDocument { components }
}

/// The card that asks the person to use the browser of another machine for new
/// browser panels, with the target, the profile and the engine.
fn browser_document(ssh_target: &str, profile: &str) -> BlockDocument {
    BlockDocument {
        components: vec![
            Component::Heading {
                text: format!("Use the browser on {ssh_target} for new browser panels?"),
                subtitle: None,
                badge: None,
            },
            Component::Kv {
                rows: vec![
                    KvRow {
                        label: "Browser".to_owned(),
                        value: "Chromium".to_owned(),
                        mono: false,
                    },
                    KvRow {
                        label: "Profile".to_owned(),
                        value: profile.to_owned(),
                        mono: true,
                    },
                    KvRow {
                        label: "SSH target".to_owned(),
                        value: ssh_target.to_owned(),
                        mono: true,
                    },
                ],
            },
            Component::Text {
                text: BROWSER_USES.to_owned(),
            },
            Component::Text {
                text: BROWSER_NEXT_START.to_owned(),
            },
            Component::Buttons {
                items: vec![
                    Button {
                        id: APPLY.to_owned(),
                        label: "Use it".to_owned(),
                        tone: Tone::Primary,
                    },
                    Button {
                        id: DECLINE.to_owned(),
                        label: "Not now".to_owned(),
                        tone: Tone::Neutral,
                    },
                ],
            },
        ],
    }
}

/// The card of a setting request, for the tests of this module and of the
/// skills that describe it.
#[cfg(test)]
pub(in crate::app) fn document_of(operation: &Operation) -> Option<BlockDocument> {
    card_of(operation).ok().map(|(_, _, document)| document)
}

#[cfg(test)]
mod tests;
