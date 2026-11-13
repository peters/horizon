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
    Answer, BrowserEngine, EntryStart, HOST_APPLY_TIMEOUT, Operation, Outcome, Request,
};
use horizon_browser_control::manifest::block_machines;
use horizon_browser_control::manifest::now_millis;
use horizon_core::{
    BlockPanelState, BrowserHostConfig, BrowserHostEngine, Config, DEFAULT_LINK_COMMAND, PanelId, PanelKind,
    PanelOptions, ServerConfig,
};

use super::HorizonApp;
use super::block_requests::BlockAnswer;
use super::browser_requests::ActorPanel;

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
/// The line under the button row of the server card, after the two statements
/// of what the link changes.
pub(in crate::app) const SERVER_SENT: &str = "Your machine list, the machine keys and your settings are sent there.";
/// The second line of the server card: what Horizon does before the first link.
pub(in crate::app) const SERVER_KEPT: &str = "The settings of this computer are kept aside in a dated file first.";
/// The line of the browser card that says which profile of which browser the new
/// panels use, in the words of the Settings window. A Firefox host runs a driver
/// of Horizon beside its browser, so that machine needs the same Horizon.
pub(in crate::app) const BROWSER_USES: &str = "Browser panels open in that profile of that machine, over SSH. A Firefox host runs Firefox and geckodriver and needs a horizon binary of the same version on that machine; a Chromium host needs a Chromium program there and no horizon.";
/// The line of the browser card that says when the entry applies.
pub(in crate::app) const BROWSER_NEXT_START: &str = "The change applies at the next start of Horizon.";

/// The change that waits for the person's press, as the request named it and as
/// the store will hold it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Asked {
    /// The SSH target and the command of the server entry.
    ServerLink { ssh_target: String, command: String },
    /// The SSH target, the profile and the engine of the browser host entry.
    BrowserHost {
        ssh_target: String,
        profile: String,
        engine: BrowserHostEngine,
    },
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

/// The setting request that this host holds open, if any.
#[derive(Default)]
pub(in crate::app) struct SettingRequestState {
    waiting: Option<Waiting>,
}

impl SettingRequestState {
    /// Whether a card of Horizon waits for the person's press, so the request
    /// pump keeps waking until the wait ends.
    pub(in crate::app) fn is_waiting(&self) -> bool {
        self.waiting.is_some()
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
        let outcome = if id == APPLY {
            self.apply_setting(&waiting, ctx)
        } else {
            unconfirmed_outcome(&waiting.asked, Answer::Declined)
        };
        self.remove_setting_card(&waiting, ctx);
        self.write_block_answer(BlockAnswer::new(waiting.root.as_deref(), waiting.request, outcome));
        self.mark_runtime_dirty();
        ctx.request_repaint();
        true
    }

    /// Writes the setting that the person confirmed, as the Settings window
    /// writes it. A refusal here is the answer of the request: the fields passed
    /// their rule when the card was drawn, so a refusal names a setting of the
    /// store that the new entry contradicts.
    fn apply_setting(&mut self, waiting: &Waiting, ctx: &Context) -> Outcome {
        let written = match &waiting.asked {
            Asked::ServerLink { ssh_target, command } => {
                self.write_server_entry(ssh_target, command, ctx)
                    .map(|start| Outcome::ServerLink {
                        answer: Answer::Confirmed,
                        start: Some(start),
                    })
            }
            Asked::BrowserHost {
                ssh_target,
                profile,
                engine,
            } => self
                .write_browser_host_entry(ssh_target, profile, *engine)
                .map(|()| Outcome::BrowserHost {
                    answer: Answer::Confirmed,
                }),
        };
        written.unwrap_or_else(|message| Outcome::failed(block_machines::INVALID_REQUEST, &message))
    }

    /// Writes the server entry of this computer and starts the link of it now, as
    /// a join does. The SSH target and the command are the two fields that the
    /// card stated; every other field of the entry stays as the store holds it.
    fn write_server_entry(&mut self, ssh_target: &str, command: &str, ctx: &Context) -> Result<EntryStart, String> {
        let before = self.stored_config();
        let mut stored = before.clone();
        stored.server = ServerConfig {
            ssh_target: Some(ssh_target.to_owned()),
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
    fn write_browser_host_entry(
        &mut self,
        ssh_target: &str,
        profile: &str,
        engine: BrowserHostEngine,
    ) -> Result<(), String> {
        let before = self.stored_config();
        let mut stored = before.clone();
        stored.browser_host = BrowserHostConfig {
            ssh_target: Some(ssh_target.to_owned()),
            profile: Some(profile.to_owned()),
            engine,
            ..stored.browser_host
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
        Asked::ServerLink { .. } => Outcome::ServerLink { answer, start: None },
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
        Operation::ServerLinkRequest { ssh_target, command } => {
            let target = ssh_target.trim();
            let command = command.as_deref().map_or(DEFAULT_LINK_COMMAND, str::trim);
            let entry = ServerConfig {
                ssh_target: Some(target.to_owned()),
                command: Some(command.to_owned()),
                address: None,
            };
            entry
                .validate()
                .map_err(|error| Outcome::failed(block_machines::INVALID_REQUEST, &error.to_string()))?;
            let asked = Asked::ServerLink {
                ssh_target: target.to_owned(),
                command: command.to_owned(),
            };
            Ok((asked, SERVER_TITLE, server_document(target, command)))
        }
        Operation::BrowserHostRequest {
            ssh_target,
            profile,
            engine,
        } => {
            let target = ssh_target.trim();
            let profile = profile.trim();
            let engine = match engine.unwrap_or(BrowserEngine::Chromium) {
                BrowserEngine::Chromium => BrowserHostEngine::Chromium,
                BrowserEngine::Firefox => BrowserHostEngine::Firefox,
            };
            let entry = BrowserHostConfig {
                ssh_target: Some(target.to_owned()),
                profile: Some(profile.to_owned()),
                engine,
                ..BrowserHostConfig::default()
            };
            entry
                .validate()
                .map_err(|error| Outcome::failed(block_machines::INVALID_REQUEST, &error.to_string()))?;
            let asked = Asked::BrowserHost {
                ssh_target: target.to_owned(),
                profile: profile.to_owned(),
                engine,
            };
            Ok((asked, BROWSER_TITLE, browser_document(target, profile, engine)))
        }
        _ => Err(Outcome::failed(
            block_machines::INVALID_REQUEST,
            "This operation asks no setting of this computer",
        )),
    }
}

/// The card that asks the person to link this computer to a Horizon server, with
/// the target and the command exactly as the settings store them.
fn server_document(ssh_target: &str, command: &str) -> BlockDocument {
    BlockDocument {
        components: vec![
            Component::Heading {
                text: format!("Link this computer to the Horizon server on {ssh_target}?"),
                subtitle: None,
                badge: None,
            },
            Component::Kv {
                rows: vec![
                    KvRow {
                        label: "Server target".to_owned(),
                        value: ssh_target.to_owned(),
                        mono: true,
                    },
                    KvRow {
                        label: "Command".to_owned(),
                        value: command.to_owned(),
                        mono: true,
                    },
                ],
            },
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
    }
}

/// The card that asks the person to use the browser of another machine for new
/// browser panels, with the target, the profile and the engine.
fn browser_document(ssh_target: &str, profile: &str, engine: BrowserHostEngine) -> BlockDocument {
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
                        value: match engine {
                            BrowserHostEngine::Chromium => "Chromium".to_owned(),
                            BrowserHostEngine::Firefox => "Firefox, with Horizon on that machine".to_owned(),
                        },
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
