//! Private host coordination for the public `block_panel` tool.
//!
//! An agent asks Horizon to draw a card in its own workspace: `create` with a
//! document, `update` with a new one, and `list`, `inspect`, `visibility` and
//! `close` around it. A person's clicks on the card reach its owner through
//! `next_click`, a request the host keeps open until a click arrives or the
//! wait ends. `open_terminal` asks for an SSH terminal instead of a card, which
//! the host opens for the person beside the calling agent panel, `open_panel`
//! asks for a panel of one preset of the person's settings in a folder that the
//! caller names, and `presets` reads the presets that such a panel can come from,
//! by name and kind alone. `machine_key`, `machine_keys` and `delete_machine_key`
//! keep the SSH key pairs that such a terminal connects with. `server_link_request`
//! and `browser_host_request` ask the person to change a setting of this
//! computer on a card that Horizon draws itself: the agent states the change and
//! reads the person's answer, and only the person's press writes it. A request
//! travels the path of a Device panel request: a bounded file under
//! `runtime/block-panel-requests`, claimed by this host's UI on the coordination
//! worker, applied on the UI thread, and answered with one result file that only
//! the asking actor and its host consume.
//!
//! The document is untrusted text from an agent that Horizon draws in a
//! person's workspace, so the UI validates every document again before it
//! reaches a panel, and the whole request is bounded in size here.

use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    AgentIdentity, ManifestLock,
    argument::value_or_text,
    block_terminal::TerminalRoute,
    now_millis,
    request_queue::{MAX_PENDING_REQUESTS, prune_at, queue_lock_path, read_json, request_count, write_private_json},
};
use crate::{
    block::{BlockDocument, ClickEvent},
    paths::{BrowserRuntimePaths, safe_local_id},
};

/// Refusal code of an operation that a card socket of a machine session does
/// not serve, because it changes a machine, a key or a setting of the person.
pub const CARDS_ONLY: &str = "cards_only";

/// Most cards one agent may own in one workspace at a time.
pub const MAX_CARDS_PER_OWNER: usize = 8;
/// Most SSH terminals one agent panel may open through `open_terminal` in one
/// Horizon run. Closing a terminal does not free the allowance of the run.
pub const MAX_TERMINALS_PER_OWNER: usize = 8;
/// Largest serialized request accepted. A document is smaller than this, so a
/// queue of them stays bounded whatever a caller sends.
const MAX_REQUEST_BYTES: usize = 96 * 1024;
/// How long the host has to apply a card request, on top of a wait the request
/// asks for. Every other operation has this window alone.
pub const HOST_APPLY_TIMEOUT: Duration = Duration::from_secs(10);
/// The wait `next_click` asks for when the caller names none.
pub const DEFAULT_NEXT_CLICK_TIMEOUT_MILLIS: u64 = 300_000;
/// The longest wait a `next_click` may ask for. It stays below the 3 660 000 ms
/// tool timeout that Horizon injects into its MCP clients.
pub const MAX_NEXT_CLICK_TIMEOUT_MILLIS: u64 = 3_600_000;
/// The shortest wait a `next_click` may ask for.
pub const MIN_NEXT_CLICK_TIMEOUT_MILLIS: u64 = 1_000;
/// How long the card of a setting request waits for the person's press, and
/// then removes itself. A request that names a setting never waits longer, so an
/// agent's question cannot hold a card open for an hour.
pub const SETTING_REQUEST_TIMEOUT_MILLIS: u64 = 300_000;

/// Most items that one proposal of the front desk may hold. The person reads
/// every item of a proposal at one press, so the list stays short.
pub const MAX_PROPOSAL_ITEMS: usize = 6;
/// Most characters of the title of one item of a proposal. The title is one line
/// and becomes the name of a workspace.
pub const MAX_PROPOSAL_TITLE_CHARS: usize = 60;
/// Most characters of the task of one item of a proposal. The task becomes the
/// first message of the orchestrator of the new work item.
pub const MAX_PROPOSAL_TASK_CHARS: usize = 4_000;
/// Most bytes of the folder of one item of a proposal, as an absolute path of
/// this computer.
pub const MAX_PROPOSAL_FOLDER_BYTES: usize = 1_024;
/// Most bytes of one message that the front desk passes to a work item. It is
/// the bound of one message of the person
/// (`horizon_core::agent_bridge::MAX_TEXT_BYTES`), which the dock cuts to as it
/// takes a message.
pub const MAX_WORK_ITEM_MESSAGE_BYTES: usize = 256 * 1024;
/// How long the front desk waits before it passes a second message to the same
/// work item, in milliseconds.
pub const WORK_ITEM_MESSAGE_INTERVAL_MILLIS: u64 = 10_000;
/// Most work items that `work_items` answers.
pub const MAX_WORK_ITEMS: usize = 50;
/// Most characters of the question of a work item in the answer of
/// `work_items`.
pub const MAX_WORK_ITEM_QUESTION_CHARS: usize = 160;
/// Most characters of the last text of a work item in the answer of
/// `work_items`.
pub const MAX_WORK_ITEM_LAST_CHARS: usize = 200;

/// Card commands. The document belongs to the agent; the panel id, revision
/// and ownership belong to the host.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum Operation {
    /// Draw a new card in the calling agent's workspace. Does not take focus.
    Create {
        /// One line, at most 80 characters; the host trims it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// The document to draw: `{"components": [...]}`.
        #[schemars(with = "BlockDocument")]
        document: serde_json::Value,
    },
    /// Replace the whole document of a card the caller owns.
    Update {
        panel_id: String,
        /// The document to draw: `{"components": [...]}`.
        #[schemars(with = "BlockDocument")]
        document: serde_json::Value,
    },
    /// Cards in the calling agent's workspace.
    List,
    /// One card of the calling agent's workspace, with its document.
    Inspect { panel_id: String },
    /// Wait for the next click on a card the caller owns. Answers the oldest
    /// waiting click at once, or `timed_out` when `timeout_millis` passes.
    NextClick {
        /// The card to wait on; every card of the caller when omitted, where
        /// the oldest waiting click of those cards answers first.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        panel_id: Option<String>,
        /// How long to wait, 1 000-3 600 000 ms; 300 000 when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_millis: Option<u64>,
    },
    /// Show or hide a card the caller owns.
    Visibility { panel_id: String, visible: bool },
    /// Close a card the caller owns. The document is dropped with it.
    Close { panel_id: String },
    /// Open an SSH terminal panel beside the caller, in the caller's workspace,
    /// without taking keyboard focus. Horizon builds the `ssh` command line
    /// from the route and adds `-t` for a remote command; the agent passes no
    /// other field, so it can add no ssh option and no argument.
    OpenTerminal {
        /// The SSH host to reach: host, user and port only. Leave it out when
        /// `machine` names a machine of this Horizon, because the record holds
        /// the route.
        #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "value_or_text")]
        ssh: Option<TerminalRoute>,
        /// One line to run on the host after login, such as
        /// `tmux new -A -s build`. Letters, digits, spaces and `. _ : / , = @ + -`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote_command: Option<String>,
        /// One line, at most 80 characters; the host trims it. Defaults to
        /// `user@host`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// Name of a machine key of the caller's workspace, as `machine_key`
        /// reports it. Horizon then connects with that key alone, through its
        /// own known-hosts file. Omitted means no key: `ssh` offers the
        /// person's own keys and ssh agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        /// Name of a machine that an agent registered in this Horizon. Horizon
        /// takes the route, the key and the tmux command from that record, so
        /// `ssh`, `key` and `remote_command` must be left out. A click on the
        /// machine row in the sidebar opens the same panel.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine: Option<String>,
    },
    /// Open a panel of a preset of the person's settings beside the caller, in
    /// the caller's workspace, without taking keyboard focus. Horizon takes the
    /// command, the arguments and the kind from the preset, so the caller adds
    /// no command, no argument and no environment. A preset of a kind that runs
    /// no local program, such as a browser page, a Device viewer, a card or an
    /// SSH terminal, is refused.
    OpenPanel {
        /// The name or the alias of a preset of the settings, exactly. A name
        /// that this Horizon does not keep is `unknown_preset`, and the answer
        /// lists the names of the presets.
        preset: String,
        /// The absolute folder that the panel runs in, at most 1 024 bytes. It
        /// must be a directory of this computer, and the caller chose it.
        directory: String,
        /// One line, at most 80 characters; the host trims it. Defaults to the
        /// name of the preset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// One line of at most 512 characters, typed into the panel one time when
        /// its program is ready. It is text, not a command: Horizon writes it into
        /// the terminal of the new panel as a person's paste and then Enter, once.
        /// It goes to a preset of an agent kind only, because a shell or a command
        /// reads no sentence. The caller can already start that agent, so the line
        /// is a message that the person can read in the panel and no new right.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
    },
    /// The presets that a panel of `open_panel` can be opened from, each with its
    /// name and its kind. It is a read: the answer holds no command, no argument,
    /// no alias and no folder of a preset, and the settings are not changed.
    Presets,
    /// Add or replace one machine of this Horizon: its name, SSH route, key, tmux
    /// session and the provider line that the caller read. The record belongs
    /// to the person, so every agent panel of this Horizon sees it and a click on
    /// its sidebar row opens a terminal without an agent.
    MachineSet {
        name: String,
        /// The SSH host to reach: host, user and port only.
        #[serde(deserialize_with = "value_or_text")]
        ssh: TerminalRoute,
        /// Name of a machine key of this Horizon, as `machine_key` reports it.
        /// Horizon then connects with that key alone.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        /// tmux session that the terminal attaches to, 1 to 40 characters of
        /// `A-Z`, `a-z`, `0-9`, `_` and `-`. The terminal runs
        /// `tmux new -A -s <session>`, so the work survives the terminal.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
        /// One line of at most 60 characters, such as `Hetzner, hel1`. It is the
        /// caller's claim and Horizon draws it in a low-emphasis style.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        /// Whether the setup of the cards of the agents of this machine ran, so
        /// this window serves a card socket for each session of the machine.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        cards: bool,
        /// The repository that this machine is ready for, such as
        /// `example-org/shop`: one line of at most 60 characters. Horizon draws
        /// it under the name of the machine in the sidebar, it reads no folder
        /// and it starts nothing from it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repository: Option<String>,
        /// The folder that the new terminals of this machine start in, such as
        /// `/home/dev/example-repo`: one absolute path of the machine.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder: Option<String>,
    },
    /// Forget one machine of this Horizon. Does not delete its key: call
    /// `delete_machine_key` for that. A terminal that the machine opened keeps
    /// its own saved connection.
    MachineDelete { name: String },
    /// How this computer reaches the Horizon server, the state of that link and
    /// the store id that this computer saw last. It reads and writes nothing, and
    /// it never answers a token, a key or a secret: an agent reads the entry that
    /// the person saved, as the settings hold it.
    ServerStatus,
    /// The browser host that the browser panels of this computer follow: the SSH
    /// target or the machine that runs the browser, its profile, its engine and
    /// the command that starts its driver, or the word `local` when the person
    /// set no host. It reads and writes nothing, and it never answers a key, a
    /// password or a token: an agent reads the entry that the person saved, as
    /// the settings hold it.
    BrowserHostStatus,
    /// The machines of this Horizon, in name order, each with whether a terminal
    /// of it is open.
    Machines,
    /// The NAMES of the named secrets that a panel of this computer gets when it
    /// starts now. It is a read: the answer holds no value, no length of a value
    /// and no hash of one, and it changes nothing.
    ///
    /// A panel reads its environment once, at its start, so a secret that a
    /// person stores while a panel runs reaches the panel that starts after it
    /// and not this one. A join that needs a stored value runs its command in a
    /// terminal that starts after the store.
    SecretNames,
    /// Make or read the key pair that Horizon keeps for one machine name in the
    /// caller's workspace, and answer its public key with the paths of the
    /// pair. Horizon makes an ed25519 pair with no passphrase under its own
    /// home, so the private half never leaves this computer and the person's own
    /// keys are not used.
    MachineKey { name: String },
    /// The key pairs of the calling agent's workspace, by name.
    MachineKeys,
    /// Delete one key pair of the calling agent's workspace, with its
    /// known-hosts file. Call it after the machine is deleted.
    DeleteMachineKey { name: String },
    /// Ask the person to link this computer to a Horizon server. Horizon draws
    /// a card of its own in the caller's workspace that states the target and
    /// the command exactly as the settings store them, and it writes the entry
    /// of the settings only when the person presses `Link`. The answer is
    /// `server_link` with the person's answer, with whether the link of the
    /// running program follows the new entry now, and, when it does, with
    /// whether the server answered: `in_step` or `away` with the reason of the
    /// link in words. The caller can press nothing itself: a request that no
    /// person presses ends as `timed_out` after five minutes, and the card goes
    /// with it.
    ServerLinkRequest {
        /// The SSH target, as the settings field `server.ssh_target` takes it:
        /// an SSH config alias, a `user@host` or an address, one word. Give this
        /// for a computer of the person, which answers the keys of the person.
        /// Omit it when `machine` names the server.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ssh_target: Option<String>,
        /// The name of a machine of the machine list that runs the server, as
        /// `machines` reports it. The link then uses the address, the user, the
        /// port, the key file and the known-hosts file of a terminal of that
        /// machine, so a machine that Horizon rented is reachable: it holds the
        /// public key of Horizon and no key of the person. Give one of this and
        /// `ssh_target`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine: Option<String>,
        /// The command that the link runs on the server, as
        /// `server.command` takes it. Omitted means the default
        /// `horizon --server-link`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
    /// Ask the person to make the browser of one machine the browser of new
    /// browser panels. Horizon draws a card of its own that states the host, the
    /// profile and the engine, and it writes `browser_host` of the settings only
    /// when the person presses `Use it`. The answer is `browser_host` with the
    /// person's answer. Both engines run on another machine: a Chromium host
    /// serves its `DevTools` port, and a Firefox host needs Firefox, geckodriver
    /// and a Horizon of the same version, because a driver of Horizon beside that
    /// browser carries the page.
    BrowserHostRequest {
        /// The SSH target, as the settings field `browser_host.ssh_target`
        /// takes it: the `Host` name of the SSH configuration, one word. Give one
        /// of this and `machine`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ssh_target: Option<String>,
        /// The name of a machine of the machine list that runs the browser, as
        /// `machines` reports it. The connect then uses the address, the user, the
        /// port, the key file and the known-hosts file of a terminal of that
        /// machine, so a machine that Horizon rented is reachable: it holds the
        /// public key of Horizon and no key of the person. Give one of this and
        /// `ssh_target`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine: Option<String>,
        /// The profile of that machine, 1 to 40 characters of `a-z`, `0-9` and
        /// `-`, with a letter or a digit at each end.
        profile: String,
        /// The engine of the browser panels; `chromium` when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        engine: Option<BrowserEngine>,
        /// The program that starts the driver of that machine, as the settings
        /// field `browser_host.command` takes it: one line of letters, digits
        /// and `._:/=@+-` with single spaces between words, at most 200
        /// characters. Omitted means the default
        /// `horizon --browser-host-driver`. A Chromium host runs no driver, so a
        /// command beside it is refused.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
    /// The work items of this Horizon, in the order of the sidebar: one entry for
    /// each workspace, with the state that the word of its row shows. It is a
    /// read of the state that a frame already holds: no file and no process.
    WorkItems,
    /// Offer work items to the person. NOTHING is created: the words of the
    /// program become one proposal in the conversation of the front desk, and
    /// Horizon creates a workspace for each item only when the person presses the
    /// control that starts the proposal. An older proposal that is still open
    /// becomes `replaced`. The answer holds the id of the proposal and no
    /// workspace.
    WorkItemPropose {
        /// One to six items, each with a title of 1 to 60 characters on one line,
        /// a task of 1 to 4 000 characters, and an optional absolute folder.
        items: Vec<ProposedItem>,
    },
    /// Pass one message of the person to the orchestrator of one work item, with
    /// the caption `via the front desk` on that message. A work item that is not
    /// on the board is `not_found`, a conversation that runs a turn is `busy`, and
    /// a second message to the same work item within ten seconds is `too_soon`;
    /// none of them sends anything.
    WorkItemMessage {
        /// The local id of the workspace of the work item, as `work_items` reports
        /// it.
        work_item: String,
        /// The message of the person, 1 byte to the bound of one message.
        text: String,
    },
}

/// One item of one `work_item_propose` request.
///
/// Every field is text of a program. Horizon shows the title, the phrase of the
/// task and the folder in the conversation of the front desk before the person
/// presses anything, and it uses the folder only as the folder of the workspace
/// that the press creates.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub struct ProposedItem {
    /// The name of the work item, one line of 1 to 60 characters.
    pub title: String,
    /// What the work item must do, 1 to 4 000 characters, in the words of the
    /// person and with what done means.
    pub task: String,
    /// The absolute folder in which the orchestrator of the work item runs. It is
    /// used only after the person's press, and only when it is a directory of
    /// this computer then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

/// One work item as `work_items` reports it.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[schemars(extend("type" = "object"))]
pub struct WorkItemSummary {
    /// The local id of the workspace of the work item, which `work_item_message`
    /// takes.
    pub work_item: String,
    pub name: String,
    /// `working`, `needs_you`, `failed`, `done` or `idle`, as the word of the row
    /// of the workspace shows it.
    pub state: String,
    /// The text of the first question that a card of the work item asks the
    /// person, cut to 160 characters, or nothing when no card asks one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// The last text of the orchestrator of the work item, cut to 200 characters,
    /// or nothing when it said nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
    /// The cost of the ended turns of the orchestrator of the work item.
    pub cost_usd: f64,
    /// The machine that the work item runs on, or nothing when it runs on this
    /// computer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}

/// What the person answered to a card that Horizon drew about a setting of this
/// computer. Only the person can press it, so an agent never reads `confirmed`
/// without a press.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// The person pressed the button that applies the change, and Horizon
    /// wrote it.
    Confirmed,
    /// The person pressed the button that keeps the setting as it is.
    Declined,
    /// No press arrived before the wait ended, or the person closed the card.
    TimedOut,
}

/// Whether the link of a server entry reached its server before the answer of a
/// request that the person confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServerLinkState {
    /// The server answered the hello, and this computer is in step with it.
    InStep,
    /// No answer arrived within the bound of the request. `reason` of
    /// [`Outcome::ServerLink`] says why in words that a person reads.
    Away,
}

/// Whether a setting that the person confirmed applies to the running program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntryStart {
    /// The link of the running program reads the new entry now, so no start is
    /// needed.
    Linked,
    /// The entry is stored, and the program must start again one time before
    /// the link reads it.
    RestartNeeded,
}

/// The engine of the browser that a browser host runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserEngine {
    /// Chromium, which runs on another machine through the SSH tunnel of
    /// Horizon.
    Chromium,
    /// Firefox, which runs behind a driver of Horizon on that machine. That
    /// machine needs Firefox, geckodriver and the same version of Horizon.
    Firefox,
}

/// Public card input. Model providers refuse a tool whose input schema has
/// `oneOf`, `anyOf` or `allOf` at the top level, so the advertised schema is a
/// plain object that refers to the operations below it. The JSON a caller
/// sends is the operation itself, exactly as the host queue reads it.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(transparent)]
#[schemars(extend("type" = "object"))]
pub struct BlockPanelInput(Operation);

impl BlockPanelInput {
    #[must_use]
    pub fn into_operation(self) -> Operation {
        self.0
    }
}

/// One machine key of a workspace as `machine_keys` reports it. The private
/// half stays in the Horizon home and never appears in an answer; the answer
/// names its file, because the agent runs as the same user as Horizon.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[schemars(extend("type" = "object"))]
pub struct MachineKeySummary {
    pub name: String,
    /// The public key as `ssh-keygen` wrote it, one line, ready for
    /// `authorized_keys` on the machine.
    pub public_key: String,
    /// The private half, as an absolute path. The agent may run `ssh -i` with
    /// it for its own commands; Horizon connects the panel itself.
    pub private_key_path: String,
    /// The known-hosts file of this key alone, as an absolute path.
    pub known_hosts_path: String,
    pub created_at_millis: u64,
}

/// How this computer reaches the Horizon server, as `server_status` reports it.
///
/// Every field is the entry that the person saved in the settings and the state of
/// the link of this run. A token, a private key and a secret are not part of such
/// an entry, and this answer adds none.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[schemars(extend("type" = "object"))]
pub struct ServerLink {
    /// The SSH target of the entry, when the person named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_target: Option<String>,
    /// The name of a machine of the machine list that runs the server, when the
    /// entry names one. The link then uses the address and the login of that
    /// machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// The command that the link runs on that computer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The address of the listener of the server, `host:port`, when this computer
    /// reaches it over TCP after a join.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// The state of the link: `off` when this computer follows no server, or
    /// `connecting`, `in_step`, `behind`, `away`, `removed` or `not_logged_in`.
    pub link: String,
    /// The store id that this computer saw at its last hello, when it saw one. Two
    /// computers that read one server answer the same id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
}

/// The browser host that the browser panels of this computer follow, as
/// `browser_host_status` reports it.
///
/// Every field is the entry that the person saved in the settings, with the
/// surrounding spaces trimmed. A key, a password and a token are not part of such
/// an entry, and this answer adds none.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[schemars(extend("type" = "object"))]
pub struct BrowserHost {
    /// `host` when the settings name a browser host, or `local` when they name
    /// none and the browser panels of this computer use its own browser. A
    /// `local` answer names no target, no profile, no driver command and no
    /// engine of an entry that no panel follows.
    pub state: String,
    /// The SSH target of the entry, when the person named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_target: Option<String>,
    /// The name of a machine of the machine list that runs the browser, when the
    /// entry names one. The tunnel of a panel then uses the address, the user,
    /// the port, the key file and the known-hosts file of a terminal of that
    /// machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// The profile of that machine whose pages the new browser panels use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// The engine of that machine: `chromium` through its `DevTools` port, or
    /// `firefox` through the driver of Horizon that runs beside it.
    pub engine: String,
    /// The program that starts the driver of that machine, when the person set
    /// one instead of the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// One machine of this Horizon as `machines` reports it.
///
/// `provider` is the agent's claim, not verified Horizon state. `terminal_open`
/// is the state of the terminal that this machine opened now; it is not a
/// reachability check.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[schemars(extend("type" = "object"))]
pub struct MachineSummary {
    pub name: String,
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The repository that this machine is ready for, when the agent set one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// The folder that the new terminals of this machine start in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    pub added_at_millis: u64,
    /// Whether a terminal panel of this machine is open in this Horizon.
    pub terminal_open: bool,
    /// Whether the server has not decided a change to this machine yet, so the
    /// sidebar shows it as pending.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
    /// Whether the key of this machine is not on this computer, because a person
    /// deleted it here or on another computer. A click on the machine then opens
    /// nothing, because the only login left is the one of the person's own keys.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub key_missing: bool,
}

/// One preset that a panel of `open_panel` can be opened from: the name of the
/// settings and its kind, such as `claude`, `codex` or `shell`.
///
/// The name is what `open_panel` takes, and the kind tells an agent which preset
/// starts a program of the kind that runs the calling agent. The entry is the
/// preset name and the kind alone: the command, the arguments, the alias, the
/// resume rule and the SSH route of a preset stay in the settings.
///
/// The list names the presets that `open_panel` accepts. A preset of a browser
/// page, a Device viewer, a card or an SSH terminal runs no program of this
/// computer, so it has no folder and is left out.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[schemars(extend("type" = "object"))]
pub struct PresetSummary {
    /// The name of the preset, exactly as `open_panel` takes it.
    pub name: String,
    /// The kind of the preset, as the settings store it, such as `claude`.
    pub kind: String,
}

/// One card as `list` reports it.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[schemars(extend("type" = "object"))]
pub struct CardSummary {
    pub panel_id: String,
    pub title: String,
    pub revision: u64,
    /// Whether the calling agent created this card.
    pub owned_by_caller: bool,
    /// Clicks that wait for the owner.
    pub pending_clicks: usize,
    /// Whether a `next_click` of the owner waits on this card, so a click
    /// reaches it at once.
    pub listening: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
#[schemars(extend("type" = "object"))]
pub enum Outcome {
    Created {
        panel_id: String,
        revision: u64,
    },
    Updated {
        panel_id: String,
        revision: u64,
    },
    Panels {
        panels: Vec<CardSummary>,
    },
    Inspected {
        panel_id: String,
        title: String,
        revision: u64,
        document: BlockDocument,
        visible: bool,
        /// Whether an agent panel of the owner is still in this workspace.
        agent_present: bool,
        owned_by_caller: bool,
        /// Clicks that wait for the owner.
        pending_clicks: usize,
        /// Whether a `next_click` of the owner waits on this card.
        listening: bool,
    },
    /// One click the person made on the card.
    Clicked {
        click: ClickEvent,
    },
    /// No click arrived before the wait ended.
    TimedOut {
        timed_out: bool,
    },
    Visibility {
        panel_id: String,
        visible: bool,
    },
    /// One card that is gone. `by_owner` says whether the agent that created the
    /// card closed it; a card whose owner panel left the workspace can be closed
    /// by another agent panel of that workspace, and `closed_by` names the panel
    /// that closed it.
    Closed {
        panel_id: String,
        by_owner: bool,
        closed_by: String,
    },
    /// One panel of a preset that the caller now has in its workspace.
    Opened {
        panel_id: String,
    },
    /// The presets that a panel of `open_panel` can be opened from, in the order
    /// of the settings. An entry is a name and a kind, and no command, argument,
    /// alias or folder of a preset is part of an answer.
    Presets {
        presets: Vec<PresetSummary>,
    },
    /// One machine key pair of the caller's workspace, and whether Horizon
    /// made it for this request.
    MachineKey {
        name: String,
        public_key: String,
        /// The private half as an absolute path. The text of the key is in no
        /// answer, log line or message.
        private_key_path: String,
        /// The known-hosts file of this key alone, as an absolute path.
        known_hosts_path: String,
        created: bool,
    },
    /// The machine key pairs of the caller's workspace.
    MachineKeys {
        keys: Vec<MachineKeySummary>,
    },
    /// One machine that the caller added or replaced, and whether the name is
    /// new.
    MachineSet {
        name: String,
        created: bool,
    },
    /// One machine that the caller forgot.
    MachineDeleted {
        name: String,
    },
    /// How this computer reaches its Horizon server now. The value is boxed, so
    /// the outcome that every card operation answers stays small.
    ServerStatus {
        server: Box<ServerLink>,
    },
    /// The browser host that the browser panels of this computer follow. The
    /// value is boxed, so the outcome that every card operation answers stays
    /// small.
    BrowserHostStatus {
        host: Box<BrowserHost>,
    },
    /// The names of the named secrets that a panel of this computer gets now, in
    /// name order. A name is an environment variable name of at most 64
    /// characters. No value, no length of a value and no hash of one is part of
    /// the answer.
    SecretNames {
        names: Vec<String>,
    },
    /// The machines of this Horizon, in name order.
    Machines {
        machines: Vec<MachineSummary>,
        /// The state of the link to the server: `off` when this Horizon keeps
        /// its machine list on this computer alone, or `connecting`, `in_step`,
        /// `behind` or `away`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        server: Option<String>,
    },
    /// One machine key pair that the caller deleted.
    DeletedMachineKey {
        name: String,
    },
    /// The answer of the person to the card that asked to link this computer to
    /// a Horizon server. `start` says whether the link of the running program
    /// follows the new entry now, and it is absent when the person did not
    /// confirm. `link` says whether the server answered within the bound of the
    /// request, so `in_step` is the only answer that reads as a server that is
    /// ready; `reason` holds the words of a link that is `away`.
    ServerLink {
        answer: Answer,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start: Option<EntryStart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        link: Option<ServerLinkState>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// The answer of the person to the card that asked to use the browser of
    /// another machine for new browser panels.
    BrowserHost {
        answer: Answer,
    },
    /// The work items of this Horizon, in the order of the sidebar. An entry is
    /// the local id of a workspace, its name, the state of its orchestrator, the
    /// question of its first card, the last text of its orchestrator, the cost of
    /// its ended turns and the machine it runs on.
    WorkItems {
        items: Vec<WorkItemSummary>,
    },
    /// One proposal that the conversation of the front desk now holds, with the
    /// words that Horizon says about it. Nothing was created: the person starts
    /// the work items of the proposal with a press, or not at all.
    Proposed {
        proposal: String,
        /// The words of Horizon that the program reads: the person decides, and
        /// one message follows when they do.
        text: String,
    },
    /// One message that Horizon passed from the front desk to the orchestrator of
    /// one work item.
    WorkItemMessage {
        work_item: String,
    },
    Failed {
        code: String,
        message: String,
    },
}

impl Outcome {
    #[must_use]
    pub fn failed(code: &str, message: &str) -> Self {
        Self::Failed {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl Operation {
    /// Whether this operation belongs to a card alone, so the server of a card
    /// socket of a machine session serves it.
    #[must_use]
    pub fn is_card(&self) -> bool {
        matches!(
            self,
            Self::Create { .. }
                | Self::Update { .. }
                | Self::List
                | Self::Inspect { .. }
                | Self::NextClick { .. }
                | Self::Visibility { .. }
                | Self::Close { .. }
        )
    }

    /// The bounded wait a `next_click` asks for, if this operation waits.
    #[must_use]
    pub fn wait_timeout_millis(&self) -> Option<u64> {
        match self {
            Self::NextClick { timeout_millis, .. } => Some(
                timeout_millis
                    .unwrap_or(DEFAULT_NEXT_CLICK_TIMEOUT_MILLIS)
                    .clamp(MIN_NEXT_CLICK_TIMEOUT_MILLIS, MAX_NEXT_CLICK_TIMEOUT_MILLIS),
            ),
            // The card of a setting request waits for the person, not for the
            // caller, so the caller names no wait and the bound is fixed.
            Self::ServerLinkRequest { .. } | Self::BrowserHostRequest { .. } => Some(SETTING_REQUEST_TIMEOUT_MILLIS),
            _ => None,
        }
    }
}

/// How long a request of this operation stays valid for the host: the wait it
/// asks for and the window to apply it.
#[must_use]
pub fn request_timeout(operation: &Operation) -> Duration {
    let wait = operation
        .wait_timeout_millis()
        .map_or(Duration::ZERO, Duration::from_millis);
    HOST_APPLY_TIMEOUT + wait
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Request {
    pub request_id: String,
    pub actor: String,
    pub host_instance: String,
    pub deadline_at_millis: i64,
    pub operation: Operation,
}

#[derive(Deserialize, Serialize)]
struct ResultEnvelope {
    actor: String,
    host_instance: String,
    outcome: Outcome,
}

fn directory(root: &Path) -> PathBuf {
    root.join("runtime/block-panel-requests")
}

fn path(root: &Path, id: &str, kind: &str) -> PathBuf {
    directory(root).join(format!("{}.{kind}.json", safe_local_id(id)))
}

/// Queue a bounded request. Only Horizon-injected identities may use this API.
///
/// # Errors
/// Rejects a missing host identity, an invalid actor, an oversized request, a
/// full queue and storage failures.
pub fn enqueue(identity: AgentIdentity<'_>, operation: Operation, timeout: Duration) -> io::Result<Request> {
    enqueue_at(BrowserRuntimePaths::resolve().root(), identity, operation, timeout)
}

/// Queue a request under an explicit runtime root.
///
/// # Errors
/// Rejects a missing host identity, an invalid actor, an oversized request, a
/// full queue and storage failures.
pub fn enqueue_at(
    root: &Path,
    identity: AgentIdentity<'_>,
    operation: Operation,
    timeout: Duration,
) -> io::Result<Request> {
    super::agent::validate_actor(identity.actor)?;
    let host = identity.host_instance.filter(|host| super::valid_host_instance(host));
    if !identity.workspace_scoped() || host.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "block_panel requires an agent launched inside Horizon with its host identity",
        ));
    }
    let size = serde_json::to_vec(&operation).map_err(io::Error::other)?.len();
    if size > MAX_REQUEST_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Block panel request is too large",
        ));
    }
    let dir = directory(root);
    std::fs::create_dir_all(&dir)?;
    let _lock = ManifestLock::acquire(&queue_lock_path(&dir))?;
    prune_at(&dir)?;
    if request_count(&dir)? >= MAX_PENDING_REQUESTS {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Block panel request queue is full",
        ));
    }
    let request = Request {
        request_id: horizon_browser::new_action_id(),
        actor: identity.actor.into(),
        host_instance: host.unwrap_or_default().into(),
        deadline_at_millis: now_millis().saturating_add(i64::try_from(timeout.as_millis()).unwrap_or(i64::MAX)),
        operation,
    };
    write_private_json(&path(root, &request.request_id, "request"), &request)?;
    Ok(request)
}

/// Whether this host has a request waiting. Does not claim or remove it.
///
/// # Errors
/// Returns coordination I/O errors. Malformed individual requests are ignored.
pub fn has_pending(host: &str) -> io::Result<bool> {
    has_pending_at(BrowserRuntimePaths::resolve().root(), host)
}

/// Same as [`has_pending`], against an explicit runtime root.
///
/// # Errors
/// Returns coordination I/O errors. Malformed individual requests are ignored.
pub fn has_pending_at(root: &Path, host: &str) -> io::Result<bool> {
    let dir = directory(root);
    if !dir.exists() {
        return Ok(false);
    }
    let _lock = ManifestLock::acquire(&queue_lock_path(&dir))?;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().ends_with(".request.json") {
            continue;
        }
        let Ok(Some(request)) = read_json::<Request>(&entry.path()) else {
            continue;
        };
        if request.host_instance == host && entry.path() == path(root, &request.request_id, "request") {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Atomically claim only the requests addressed to this host. The UI checks
/// the live workspace, the owner and the document.
///
/// # Errors
/// Returns coordination I/O errors. Malformed individual requests cannot block
/// the queue.
pub fn claim(host: &str) -> io::Result<Vec<Request>> {
    claim_at(BrowserRuntimePaths::resolve().root(), host)
}

/// Same as [`claim`], against an explicit runtime root.
///
/// # Errors
/// Returns coordination I/O errors. Malformed individual requests cannot block
/// the queue.
pub fn claim_at(root: &Path, host: &str) -> io::Result<Vec<Request>> {
    let dir = directory(root);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let _lock = ManifestLock::acquire(&queue_lock_path(&dir))?;
    prune_at(&dir)?;
    let mut requests = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().ends_with(".request.json") {
            continue;
        }
        let Ok(Some(request)) = read_json::<Request>(&entry.path()) else {
            continue;
        };
        if request.host_instance != host || entry.path() != path(root, &request.request_id, "request") {
            continue;
        }
        std::fs::remove_file(entry.path())?;
        requests.push(request);
    }
    Ok(requests)
}

/// Publish a host result for one claimed request.
///
/// # Errors
/// Returns storage failures; callers must not replay a mutation on failure.
pub fn complete(request: &Request, outcome: Outcome) -> io::Result<()> {
    complete_at(BrowserRuntimePaths::resolve().root(), request, outcome)
}

/// Same as [`complete`], against an explicit runtime root.
///
/// # Errors
/// Returns storage failures; callers must not replay a mutation on failure.
pub fn complete_at(root: &Path, request: &Request, outcome: Outcome) -> io::Result<()> {
    write_private_json(
        &path(root, &request.request_id, "result"),
        &ResultEnvelope {
            actor: request.actor.clone(),
            host_instance: request.host_instance.clone(),
            outcome,
        },
    )
}

/// Consume only a result for the exact requesting actor and host.
///
/// # Errors
/// Returns storage failures or a mismatched result identity.
pub fn take_result(request: &Request) -> io::Result<Option<Outcome>> {
    take_result_at(BrowserRuntimePaths::resolve().root(), request)
}

/// Same as [`take_result`], against an explicit runtime root.
///
/// # Errors
/// Returns storage failures or a mismatched result identity.
pub fn take_result_at(root: &Path, request: &Request) -> io::Result<Option<Outcome>> {
    let path = path(root, &request.request_id, "result");
    let Some(result) = read_json::<ResultEnvelope>(&path)? else {
        return Ok(None);
    };
    if result.actor != request.actor || result.host_instance != request.host_instance {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Block panel result identity mismatch",
        ));
    }
    std::fs::remove_file(path)?;
    Ok(Some(result.outcome))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::block_machines;

    fn identity(actor: &str) -> AgentIdentity<'_> {
        AgentIdentity::new(actor, Some(super::super::host_instance()))
    }

    fn document() -> serde_json::Value {
        serde_json::json!({
            "components": [
                {"type": "heading", "text": "cloud-1"},
                {"type": "buttons", "items": [{"id": "stop", "label": "Stop", "tone": "warn"}]}
            ]
        })
    }

    #[test]
    fn the_public_input_is_an_object_schema_and_the_operation_itself() {
        let sent = serde_json::json!({"operation": "create", "document": document()});
        assert_eq!(
            serde_json::from_value::<BlockPanelInput>(sent.clone())
                .expect("public input")
                .into_operation(),
            serde_json::from_value::<Operation>(sent).expect("operation"),
            "a caller sends the operation, not a wrapper field"
        );
        assert!(
            serde_json::from_value::<BlockPanelInput>(serde_json::json!({
                "operation": "update", "panel_id": "p", "document": document(), "visible": true
            }))
            .is_err(),
            "an unknown field is still refused"
        );
        assert!(
            serde_json::from_value::<BlockPanelInput>(serde_json::json!({"operation": "wait_for_click"})).is_err(),
            "an unknown operation is still refused"
        );
        let schema = serde_json::to_value(schemars::schema_for!(BlockPanelInput)).expect("schema");
        let object = schema.as_object().expect("a schema object");
        assert_eq!(schema["type"], serde_json::json!("object"));
        for combinator in ["oneOf", "anyOf", "allOf"] {
            assert!(
                !object.contains_key(combinator),
                "providers refuse a top-level {combinator}: {schema}"
            );
        }
        assert!(object.contains_key("$ref"), "the operations stay behind one reference");
        assert!(
            schema.to_string().contains("next_click"),
            "the operations are described"
        );
        assert!(
            schema.to_string().contains("open_terminal"),
            "the terminal operation is described"
        );
        for operation in [
            "machine_key",
            "machine_keys",
            "delete_machine_key",
            "machine_set",
            "machine_delete",
            "machines",
        ] {
            assert!(
                schema.to_string().contains(operation),
                "the key operation {operation} is described: {schema}"
            );
        }
    }

    #[test]
    fn the_three_work_item_operations_read_as_the_operation_that_a_caller_sends() {
        let propose = serde_json::json!({
            "operation": "work_item_propose",
            "items": [
                {"title": "Fix login", "task": "Fix issue 1987", "folder": "/home/person/work"},
                {"title": "Cold start", "task": "Make it faster"}
            ]
        });
        let Operation::WorkItemPropose { items } = serde_json::from_value::<BlockPanelInput>(propose)
            .expect("the proposal operation")
            .into_operation()
        else {
            panic!("the proposal operation");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].folder.as_deref(), Some("/home/person/work"));
        assert_eq!(items[1].folder, None, "a folder is optional");
        super::super::block_proposal::check_items(&items).expect("the items of this proposal pass");

        let message = serde_json::json!({
            "operation": "work_item_message",
            "work_item": "8f14e45f-ceea-4a67-9f0b-3a1f2b3c4d5e",
            "text": "start with the redirect"
        });
        let Operation::WorkItemMessage { work_item, text } = serde_json::from_value::<BlockPanelInput>(message)
            .expect("the message operation")
            .into_operation()
        else {
            panic!("the message operation");
        };
        assert_eq!(work_item, "8f14e45f-ceea-4a67-9f0b-3a1f2b3c4d5e");
        assert_eq!(text, "start with the redirect");

        let read = serde_json::from_value::<BlockPanelInput>(serde_json::json!({"operation": "work_items"}))
            .expect("the read operation")
            .into_operation();
        assert_eq!(read, Operation::WorkItems);
        assert!(
            !read.is_card(),
            "the work items of the front desk are no card operation"
        );
        assert!(
            serde_json::from_value::<BlockPanelInput>(serde_json::json!({"operation": "work_items"})).is_ok(),
            "the read of the work items takes no field"
        );
        let schema = serde_json::to_string(&schemars::schema_for!(BlockPanelInput)).expect("schema");
        for operation in ["work_items", "work_item_propose", "work_item_message", "ProposedItem"] {
            assert!(schema.contains(operation), "the schema describes {operation}: {schema}");
        }
    }

    #[test]
    fn the_answers_of_the_work_item_operations_name_their_fields() {
        let items = serde_json::to_value(Outcome::WorkItems {
            items: vec![WorkItemSummary {
                work_item: "8f14e45f-ceea-4a67-9f0b-3a1f2b3c4d5e".into(),
                name: "Fix login".into(),
                state: "needs_you".into(),
                question: Some("Which file?".into()),
                last: None,
                cost_usd: 0.25,
                machine: None,
            }],
        })
        .expect("serialize");
        assert_eq!(items["status"], serde_json::json!("work_items"));
        assert_eq!(items["items"][0]["state"], serde_json::json!("needs_you"));
        assert_eq!(items["items"][0]["question"], serde_json::json!("Which file?"));
        assert!(
            items["items"][0].get("last").is_none(),
            "a work item that said nothing answers no last text"
        );

        let proposed = serde_json::to_value(Outcome::Proposed {
            proposal: "3f7c1d9e".into(),
            text: "The person decides. You get one message when they do.".into(),
        })
        .expect("serialize");
        assert_eq!(proposed["status"], serde_json::json!("proposed"));
        assert_eq!(proposed["proposal"], serde_json::json!("3f7c1d9e"));

        let sent = serde_json::to_value(Outcome::WorkItemMessage {
            work_item: "8f14e45f-ceea-4a67-9f0b-3a1f2b3c4d5e".into(),
        })
        .expect("serialize");
        assert_eq!(sent["status"], serde_json::json!("work_item_message"));
    }

    #[test]
    fn a_closed_card_names_whether_its_owner_closed_it_and_which_panel_did() {
        let owned = serde_json::to_value(Outcome::Closed {
            panel_id: "card".into(),
            by_owner: true,
            closed_by: "agent".into(),
        })
        .expect("serialize");
        assert_eq!(owned["status"], serde_json::json!("closed"));
        assert_eq!(owned["panel_id"], serde_json::json!("card"));
        assert_eq!(owned["by_owner"], serde_json::json!(true));
        assert_eq!(owned["closed_by"], serde_json::json!("agent"));

        let other = serde_json::to_value(Outcome::Closed {
            panel_id: "card".into(),
            by_owner: false,
            closed_by: "other-agent".into(),
        })
        .expect("serialize");
        assert_eq!(
            other["by_owner"],
            serde_json::json!(false),
            "a card of an owner that left the workspace says that another agent closed it"
        );
    }

    #[test]
    fn open_terminal_needs_one_route_and_answers_one_panel_id() {
        let sent = serde_json::json!({
            "operation": "open_terminal",
            "ssh": {"host": "lab-1"}
        });
        let Operation::OpenTerminal {
            ssh,
            remote_command,
            title,
            key,
            machine,
        } = serde_json::from_value::<BlockPanelInput>(sent)
            .expect("public input")
            .into_operation()
        else {
            panic!("open_terminal did not parse as OpenTerminal");
        };
        let ssh = ssh.expect("the route of the request");
        assert_eq!(ssh.host, "lab-1");
        assert_eq!(ssh.user, None);
        assert_eq!(ssh.port, None);
        assert_eq!(remote_command, None);
        assert_eq!(title, None);
        assert_eq!(key, None, "a terminal without a key offers the person's own keys");
        assert_eq!(machine, None, "a terminal without a machine takes its route from ssh");

        let opened = serde_json::to_value(Outcome::Opened {
            panel_id: "panel".into(),
        })
        .expect("serialize");
        assert_eq!(opened["status"], serde_json::json!("opened"));
        assert_eq!(opened["panel_id"], serde_json::json!("panel"));
    }

    #[test]
    fn open_panel_takes_one_optional_prompt_line_and_no_command() {
        let sent = serde_json::json!({
            "operation": "open_panel",
            "preset": "Claude Code",
            "directory": "/home/person/work",
            "prompt": "Read .horizon-plan/BRIEF.md and do todo t4 only."
        });
        let Operation::OpenPanel {
            preset,
            directory,
            title,
            prompt,
        } = serde_json::from_value::<BlockPanelInput>(sent)
            .expect("public input")
            .into_operation()
        else {
            panic!("open_panel did not parse as OpenPanel");
        };
        assert_eq!(preset, "Claude Code");
        assert_eq!(directory, "/home/person/work");
        assert_eq!(title, None, "a panel without a title takes the name of its preset");
        assert_eq!(
            prompt.as_deref(),
            Some("Read .horizon-plan/BRIEF.md and do todo t4 only.")
        );
        for refused in [
            serde_json::json!({
                "operation": "open_panel", "preset": "Claude Code",
                "directory": "/home/person/work", "command": "rm -rf /"
            }),
            serde_json::json!({
                "operation": "open_panel", "preset": "Claude Code",
                "directory": "/home/person/work", "prompt": ["two lines"]
            }),
        ] {
            assert!(
                serde_json::from_value::<BlockPanelInput>(refused.clone()).is_err(),
                "open_panel still takes no other field: {refused}"
            );
        }
    }

    #[test]
    fn every_operation_parses_and_an_unknown_field_is_rejected() {
        let create = serde_json::json!({"operation": "create", "title": "cloud-1", "document": document()});
        let Operation::Create { title, document: sent } = serde_json::from_value(create).expect("create") else {
            panic!("create did not parse as Create");
        };
        assert_eq!(title.as_deref(), Some("cloud-1"));
        assert_eq!(sent, document());
        for (operation, expected) in [
            (serde_json::json!({"operation": "list"}), "list"),
            (
                serde_json::json!({"operation": "update", "panel_id": "p", "document": document()}),
                "update",
            ),
            (serde_json::json!({"operation": "inspect", "panel_id": "p"}), "inspect"),
            (
                serde_json::json!({"operation": "next_click", "panel_id": "p"}),
                "next_click",
            ),
            (
                serde_json::json!({"operation": "visibility", "panel_id": "p", "visible": false}),
                "visibility",
            ),
            (serde_json::json!({"operation": "close", "panel_id": "p"}), "close"),
            (
                serde_json::json!({
                    "operation": "open_terminal",
                    "ssh": {"host": "lab-1", "user": "deploy", "port": 2222},
                    "remote_command": "tmux new -A -s build"
                }),
                "open_terminal",
            ),
            (
                serde_json::json!({
                    "operation": "open_panel",
                    "preset": "Claude Code",
                    "directory": "/home/person/work"
                }),
                "open_panel",
            ),
            (serde_json::json!({"operation": "server_status"}), "server_status"),
            (
                serde_json::json!({"operation": "browser_host_status"}),
                "browser_host_status",
            ),
            (serde_json::json!({"operation": "secret_names"}), "secret_names"),
            (serde_json::json!({"operation": "presets"}), "presets"),
        ] {
            let parsed: Operation = serde_json::from_value(operation).expect("operation");
            let name = match parsed {
                Operation::Create { .. } => "create",
                Operation::Update { .. } => "update",
                Operation::List => "list",
                Operation::Inspect { .. } => "inspect",
                Operation::NextClick { .. } => "next_click",
                Operation::Visibility { .. } => "visibility",
                Operation::Close { .. } => "close",
                Operation::OpenTerminal { .. } => "open_terminal",
                Operation::OpenPanel { .. } => "open_panel",
                Operation::Presets => "presets",
                Operation::MachineSet { .. } => "machine_set",
                Operation::MachineDelete { .. } => "machine_delete",
                Operation::ServerStatus => "server_status",
                Operation::BrowserHostStatus => "browser_host_status",
                Operation::SecretNames => "secret_names",
                Operation::Machines => "machines",
                Operation::MachineKey { .. } => "machine_key",
                Operation::WorkItems => "work_items",
                Operation::WorkItemPropose { .. } => "work_item_propose",
                Operation::WorkItemMessage { .. } => "work_item_message",
                Operation::MachineKeys => "machine_keys",
                Operation::DeleteMachineKey { .. } => "delete_machine_key",
                Operation::ServerLinkRequest { .. } => "server_link_request",
                Operation::BrowserHostRequest { .. } => "browser_host_request",
            };
            assert_eq!(name, expected);
        }
    }

    /// A field that is an object is what an agent program that builds every tool
    /// argument as a string sends as the JSON text of the object.
    #[test]
    fn the_ssh_route_of_a_terminal_and_of_a_machine_record_is_read_as_its_text() {
        let route = serde_json::json!({"host": "lab-1", "user": "deploy", "port": 2222});
        let text = serde_json::to_string(&route).expect("the text of a route");
        let Operation::OpenTerminal { ssh: Some(ssh), .. } =
            serde_json::from_value::<BlockPanelInput>(serde_json::json!({"operation": "open_terminal", "ssh": text}))
                .expect("public input")
                .into_operation()
        else {
            panic!("open_terminal did not read the text of a route");
        };
        assert_eq!(ssh.host, "lab-1");
        assert_eq!(ssh.user.as_deref(), Some("deploy"));
        assert_eq!(ssh.port, Some(2222));

        let Operation::MachineSet { ssh, .. } = serde_json::from_value::<BlockPanelInput>(serde_json::json!({
            "operation": "machine_set", "name": "build-1", "ssh": text
        }))
        .expect("public input")
        .into_operation() else {
            panic!("machine_set did not read the text of a route");
        };
        assert_eq!((ssh.host.as_str(), ssh.port), ("lab-1", Some(2222)));
        assert!(
            serde_json::from_value::<BlockPanelInput>(serde_json::json!({
                "operation": "machine_set", "name": "build-1", "ssh": "not a route"
            }))
            .is_err(),
            "a text that is no route is still refused"
        );
    }

    #[test]
    fn an_unknown_field_beside_a_variant_and_a_wait_without_a_card() {
        // Serde cannot reject an unknown field beside a unit variant, so the
        // check is asserted where a caller can send one: on a variant that
        // carries fields.
        for unknown_field in [
            serde_json::json!({"operation": "create", "document": document(), "visible": true}),
            serde_json::json!({"operation": "update", "panel_id": "p", "document": document(), "visible": true}),
        ] {
            assert!(
                serde_json::from_value::<Operation>(unknown_field.clone()).is_err(),
                "{unknown_field} must be refused"
            );
        }
        let unknown_operation = serde_json::json!({"operation": "wait_for_click", "panel_id": "p"});
        assert!(serde_json::from_value::<Operation>(unknown_operation).is_err());
        assert!(
            serde_json::from_value::<Operation>(serde_json::json!({
                "operation": "open_terminal",
                "ssh": {"host": "lab-1"},
                "command": "tmux new"
            }))
            .is_err(),
            "open_terminal takes no free-form command, option or argument"
        );
        let any_card = serde_json::from_value::<Operation>(serde_json::json!({"operation": "next_click"}))
            .expect("a wait without a card parses");
        assert_eq!(
            any_card,
            Operation::NextClick {
                panel_id: None,
                timeout_millis: None
            },
            "a wait without a panel_id covers every card of the caller"
        );
    }

    #[test]
    fn a_terminal_of_a_machine_takes_its_route_from_the_record() {
        let sent = serde_json::json!({"operation": "open_terminal", "machine": "build-1"});
        let Operation::OpenTerminal {
            ssh,
            remote_command,
            key,
            machine,
            ..
        } = serde_json::from_value::<BlockPanelInput>(sent)
            .expect("public input")
            .into_operation()
        else {
            panic!("open_terminal did not parse as OpenTerminal");
        };
        assert_eq!(machine.as_deref(), Some("build-1"));
        assert_eq!(ssh, None, "the record holds the route");
        assert_eq!(remote_command, None, "the record holds the tmux session");
        assert_eq!(key, None, "the record holds the key");

        assert!(
            serde_json::from_value::<Operation>(serde_json::json!({
                "operation": "open_terminal",
                "machine": "build-1",
                "command": "tmux new -A -s build"
            }))
            .is_err(),
            "a machine terminal takes no free-form field"
        );
        assert!(block_machines::check_single_target(true, true).is_err());
        assert!(block_machines::check_single_target(true, false).is_ok());
        assert!(block_machines::check_single_target(false, true).is_ok());
    }

    #[test]
    fn the_machine_operations_answer_a_record_and_never_a_command_line() {
        let set = serde_json::from_value::<Operation>(serde_json::json!({
            "operation": "machine_set",
            "name": "build-1",
            "ssh": {"host": "203.0.113.10", "user": "root", "port": 2222},
            "key": "build-1",
            "session": "build-1",
            "provider": "Hetzner, hel1",
            "repository": "example-org/shop",
            "folder": "/home/dev/example-repo",
            "cards": true
        }))
        .expect("machine_set parses");
        let Operation::MachineSet {
            name,
            ssh,
            key,
            session,
            provider,
            repository,
            folder,
            cards,
        } = set
        else {
            panic!("machine_set did not parse as MachineSet");
        };
        assert_eq!(name, "build-1");
        assert_eq!(ssh.host, "203.0.113.10");
        assert_eq!(ssh.port, Some(2222));
        assert_eq!(key.as_deref(), Some("build-1"));
        assert_eq!(session.as_deref(), Some("build-1"));
        assert_eq!(provider.as_deref(), Some("Hetzner, hel1"));
        assert_eq!(repository.as_deref(), Some("example-org/shop"));
        assert_eq!(
            folder.as_deref(),
            Some("/home/dev/example-repo"),
            "machine_set carries the folder of the new terminals"
        );
        assert!(cards, "machine_set carries the mark of the card setup");
        assert!(
            serde_json::from_value::<Operation>(serde_json::json!({
                "operation": "machine_set",
                "name": "build-1",
                "ssh": {"host": "203.0.113.10"},
                "command": "tmux new -A -s build"
            }))
            .is_err(),
            "a record holds no free-form command"
        );
        assert!(matches!(
            serde_json::from_value::<Operation>(serde_json::json!({"operation": "machines"})).expect("list"),
            Operation::Machines
        ));
        let answer = serde_json::to_value(Outcome::MachineSet {
            name: "build-1".into(),
            created: true,
        })
        .expect("serialize");
        assert_eq!(answer["status"], serde_json::json!("machine_set"));
        assert_eq!(answer["created"], serde_json::json!(true));
        let listed = serde_json::to_value(Outcome::Machines {
            server: Some("in_step".into()),
            machines: vec![MachineSummary {
                name: "build-1".into(),
                host: "203.0.113.10".into(),
                user: Some("root".into()),
                port: None,
                key: Some("build-1".into()),
                session: Some("build-1".into()),
                provider: Some("Hetzner, hel1".into()),
                repository: Some("example-org/shop".into()),
                folder: None,
                added_at_millis: 7,
                terminal_open: true,
                pending: true,
                key_missing: true,
            }],
        })
        .expect("serialize");
        assert_eq!(listed["status"], serde_json::json!("machines"));
        assert_eq!(listed["machines"][0]["terminal_open"], serde_json::json!(true));
        assert_eq!(listed["machines"][0]["pending"], serde_json::json!(true));
        assert_eq!(
            listed["machines"][0]["key_missing"],
            serde_json::json!(true),
            "a machine whose key was deleted says so"
        );
        assert_eq!(listed["server"], serde_json::json!("in_step"));
    }

    #[test]
    fn the_key_operations_answer_the_public_key_and_never_the_private_half() {
        let made = serde_json::from_value::<Operation>(serde_json::json!({
            "operation": "machine_key", "name": "build-1"
        }))
        .expect("machine_key parses");
        assert_eq!(made, Operation::MachineKey { name: "build-1".into() });
        assert_eq!(
            serde_json::from_value::<Operation>(serde_json::json!({"operation": "machine_keys"})).expect("list"),
            Operation::MachineKeys
        );
        assert_eq!(
            serde_json::from_value::<Operation>(serde_json::json!({
                "operation": "delete_machine_key", "name": "build-1"
            }))
            .expect("delete"),
            Operation::DeleteMachineKey { name: "build-1".into() }
        );
        for unknown in [
            serde_json::json!({"operation": "machine_key", "name": "build-1", "path": "/home/peters/.ssh/id"}),
            serde_json::json!({"operation": "machine_key", "name": "build-1", "passphrase": ""}),
            serde_json::json!({"operation": "delete_machine_key", "name": "build-1", "force": true}),
        ] {
            assert!(
                serde_json::from_value::<Operation>(unknown.clone()).is_err(),
                "an agent passes a name and nothing else: {unknown}"
            );
        }

        let answer = serde_json::to_value(Outcome::MachineKey {
            name: "build-1".into(),
            public_key: "ssh-ed25519 AAAA horizon-machine-build-1".into(),
            private_key_path: "/home/horizon/machine-keys/ws/build-1".into(),
            known_hosts_path: "/home/horizon/machine-keys/ws/build-1.known_hosts".into(),
            created: true,
        })
        .expect("serialize");
        assert_eq!(answer["status"], serde_json::json!("machine_key"));
        assert_eq!(answer["created"], serde_json::json!(true));
        assert_eq!(
            answer["public_key"],
            serde_json::json!("ssh-ed25519 AAAA horizon-machine-build-1")
        );
        assert_eq!(
            answer["private_key_path"],
            serde_json::json!("/home/horizon/machine-keys/ws/build-1"),
            "the answer names the file of the private half"
        );
        assert_eq!(
            answer["known_hosts_path"],
            serde_json::json!("/home/horizon/machine-keys/ws/build-1.known_hosts")
        );
        assert!(
            !answer.to_string().contains("PRIVATE"),
            "an answer never holds the private half: {answer}"
        );

        let list = serde_json::to_value(Outcome::MachineKeys {
            keys: vec![MachineKeySummary {
                name: "build-1".into(),
                public_key: "ssh-ed25519 AAAA horizon-machine-build-1".into(),
                private_key_path: "/home/horizon/machine-keys/ws/build-1".into(),
                known_hosts_path: "/home/horizon/machine-keys/ws/build-1.known_hosts".into(),
                created_at_millis: 1_700_000_000_000,
            }],
        })
        .expect("serialize");
        assert_eq!(list["status"], serde_json::json!("machine_keys"));
        assert_eq!(list["keys"][0]["name"], serde_json::json!("build-1"));
        assert_eq!(
            list["keys"][0]["created_at_millis"],
            serde_json::json!(1_700_000_000_000u64)
        );
        assert_eq!(
            list["keys"][0]["private_key_path"],
            serde_json::json!("/home/horizon/machine-keys/ws/build-1")
        );
        assert_eq!(
            list["keys"][0]["known_hosts_path"],
            serde_json::json!("/home/horizon/machine-keys/ws/build-1.known_hosts")
        );

        let deleted = serde_json::to_value(Outcome::DeletedMachineKey { name: "build-1".into() }).expect("serialize");
        assert_eq!(deleted["status"], serde_json::json!("deleted_machine_key"));
        assert_eq!(deleted["name"], serde_json::json!("build-1"));
    }

    #[test]
    fn a_next_click_wait_is_bounded_and_widens_only_its_own_request_window() {
        let wait = |timeout_millis| Operation::NextClick {
            panel_id: Some("card".into()),
            timeout_millis,
        };
        assert_eq!(
            wait(None).wait_timeout_millis(),
            Some(DEFAULT_NEXT_CLICK_TIMEOUT_MILLIS)
        );
        assert_eq!(wait(Some(60_000)).wait_timeout_millis(), Some(60_000));
        assert_eq!(
            wait(Some(MAX_NEXT_CLICK_TIMEOUT_MILLIS + 1)).wait_timeout_millis(),
            Some(MAX_NEXT_CLICK_TIMEOUT_MILLIS)
        );
        assert_eq!(wait(Some(0)).wait_timeout_millis(), Some(MIN_NEXT_CLICK_TIMEOUT_MILLIS));
        assert_eq!(Operation::List.wait_timeout_millis(), None);

        assert_eq!(request_timeout(&Operation::List), HOST_APPLY_TIMEOUT);
        assert_eq!(
            request_timeout(&wait(Some(60_000))),
            HOST_APPLY_TIMEOUT + Duration::from_secs(60)
        );
        assert!(
            DEFAULT_NEXT_CLICK_TIMEOUT_MILLIS + u64::try_from(HOST_APPLY_TIMEOUT.as_millis()).unwrap_or(u64::MAX)
                < 3_660_000,
            "a default wait fits the injected MCP client timeout"
        );
    }

    #[test]
    fn a_click_event_round_trips_with_every_field_the_agent_reads() {
        let click = ClickEvent {
            event_id: 4,
            panel_id: "card".into(),
            component_id: "continue".into(),
            revision: 2,
            selected: vec!["size.cx32".into()],
            at_millis: 1_700_000_000_000,
            stale: true,
        };
        let encoded = serde_json::to_value(&click).expect("serialize");
        for field in [
            "event_id",
            "panel_id",
            "component_id",
            "revision",
            "selected",
            "at_millis",
            "stale",
        ] {
            assert!(encoded.get(field).is_some(), "the click omits {field}");
        }
        assert_eq!(serde_json::from_value::<ClickEvent>(encoded).expect("parse"), click);

        let answered = serde_json::to_value(Outcome::Clicked { click }).expect("serialize");
        assert_eq!(answered["status"], serde_json::json!("clicked"));
        assert!(answered.get("click").is_some());
        let timed_out = serde_json::to_value(Outcome::TimedOut { timed_out: true }).expect("serialize");
        assert_eq!(timed_out["status"], serde_json::json!("timed_out"));
        assert_eq!(timed_out["timed_out"], serde_json::json!(true));
    }

    #[test]
    fn a_queued_request_round_trips_and_only_its_actor_consumes_the_result() {
        let root = tempfile::tempdir().expect("runtime root");
        let request = enqueue_at(
            root.path(),
            identity("horizon:agent-a"),
            Operation::Update {
                panel_id: "card".into(),
                document: document(),
            },
            Duration::from_secs(10),
        )
        .expect("enqueue");
        assert!(has_pending_at(root.path(), super::super::host_instance()).expect("pending"));
        // Another host neither sees nor claims this request.
        assert!(!has_pending_at(root.path(), "another-host").expect("pending"));
        assert!(claim_at(root.path(), "another-host").expect("claim").is_empty());
        let claimed = claim_at(root.path(), super::super::host_instance()).expect("claim");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].operation, request.operation);
        assert!(!has_pending_at(root.path(), super::super::host_instance()).expect("pending"));

        complete_at(
            root.path(),
            &request,
            Outcome::Updated {
                panel_id: "card".into(),
                revision: 3,
            },
        )
        .expect("complete");
        let mut other = request.clone();
        other.actor = "horizon:agent-b".into();
        assert!(take_result_at(root.path(), &other).is_err(), "another actor is refused");
        let outcome = take_result_at(root.path(), &request).expect("result").expect("answer");
        assert!(matches!(outcome, Outcome::Updated { revision: 3, .. }));
        assert!(
            take_result_at(root.path(), &request).expect("result").is_none(),
            "a result is consumed once"
        );
    }

    #[test]
    fn a_request_carries_its_deadline_and_an_unsupported_identity_is_refused() {
        let root = tempfile::tempdir().expect("runtime root");
        let before = now_millis();
        let request = enqueue_at(
            root.path(),
            identity("horizon:agent-a"),
            Operation::List,
            Duration::from_secs(10),
        )
        .expect("enqueue");
        assert!(request.deadline_at_millis >= before + 9_900);
        assert!(request.deadline_at_millis <= before + 10_100);
        let expired = enqueue_at(
            root.path(),
            identity("horizon:agent-a"),
            Operation::List,
            Duration::from_secs(0),
        )
        .expect("enqueue");
        assert!(expired.deadline_at_millis <= now_millis());

        for (actor, host) in [("plain-actor", Some("host")), ("horizon:agent-a", None)] {
            let refused = enqueue_at(
                root.path(),
                AgentIdentity::new(actor, host),
                Operation::List,
                Duration::from_secs(5),
            );
            assert!(refused.is_err(), "{actor} with host {host:?} must be refused");
        }
    }

    #[test]
    fn the_names_of_the_stored_secrets_are_a_read_of_names_alone() {
        let asked = serde_json::from_value::<Operation>(serde_json::json!({"operation": "secret_names"}))
            .expect("secret_names parses");
        assert_eq!(asked, Operation::SecretNames);
        assert_eq!(asked.wait_timeout_millis(), None, "the read waits for nothing");

        let answer = serde_json::to_value(Outcome::SecretNames {
            names: vec!["TS_HOME_CLIENT_ID".into(), "TS_HOME_CLIENT_SECRET".into()],
        })
        .expect("serialize");
        assert_eq!(answer["status"], serde_json::json!("secret_names"));
        assert_eq!(
            answer["names"],
            serde_json::json!(["TS_HOME_CLIENT_ID", "TS_HOME_CLIENT_SECRET"])
        );
        assert_eq!(
            answer.as_object().map(serde_json::Map::len),
            Some(2),
            "the answer holds the status and the names, and no value: {answer}"
        );
    }

    #[test]
    fn a_setting_request_parses_and_its_answer_names_the_persons_press() {
        let link = serde_json::from_value::<Operation>(serde_json::json!({
            "operation": "server_link_request",
            "ssh_target": "server-vm",
            "command": "/opt/horizon --server-link"
        }))
        .expect("server_link_request parses");
        assert_eq!(
            link,
            Operation::ServerLinkRequest {
                ssh_target: Some("server-vm".into()),
                machine: None,
                command: Some("/opt/horizon --server-link".into()),
            }
        );
        assert_eq!(
            serde_json::from_value::<Operation>(serde_json::json!({
                "operation": "server_link_request", "ssh_target": "server-vm"
            }))
            .expect("the command is optional"),
            Operation::ServerLinkRequest {
                ssh_target: Some("server-vm".into()),
                machine: None,
                command: None,
            },
            "without a command the link runs the default"
        );
        assert_eq!(
            serde_json::from_value::<Operation>(serde_json::json!({
                "operation": "server_link_request", "machine": "rented-1"
            }))
            .expect("a machine of the list names the server"),
            Operation::ServerLinkRequest {
                ssh_target: None,
                machine: Some("rented-1".into()),
                command: None,
            },
            "a machine of the list carries no SSH target, and the link uses its login"
        );
    }

    /// A browser host request names its host, its profile and its engine, and a
    /// command only with the engine that runs a driver here.
    #[test]
    fn a_browser_host_request_parses_and_its_engine_holds_its_command() {
        let browser = serde_json::from_value::<Operation>(serde_json::json!({
            "operation": "browser_host_request",
            "ssh_target": "browser-vm",
            "profile": "work",
            "engine": "chromium"
        }))
        .expect("browser_host_request parses");
        assert_eq!(
            browser,
            Operation::BrowserHostRequest {
                ssh_target: Some("browser-vm".into()),
                machine: None,
                profile: "work".into(),
                engine: Some(BrowserEngine::Chromium),
                command: None,
            }
        );
        assert!(
            serde_json::from_value::<Operation>(serde_json::json!({
                "operation": "browser_host_request", "ssh_target": "browser-vm", "profile": "work",
                "engine": "webkit"
            }))
            .is_err(),
            "an engine outside the two known ones is refused"
        );
        assert_eq!(
            serde_json::from_value::<Operation>(serde_json::json!({
                "operation": "browser_host_request",
                "ssh_target": "browser-vm",
                "profile": "work",
                "engine": "firefox",
                "command": "/opt/horizon/horizon --browser-host-driver"
            }))
            .expect("a Firefox host names the program of its driver"),
            Operation::BrowserHostRequest {
                ssh_target: Some("browser-vm".into()),
                machine: None,
                profile: "work".into(),
                engine: Some(BrowserEngine::Firefox),
                command: Some("/opt/horizon/horizon --browser-host-driver".into()),
            },
            "the command is the field of the settings that starts the driver"
        );
        let on_machine = serde_json::from_value::<Operation>(serde_json::json!({
            "operation": "browser_host_request",
            "machine": "rented-1",
            "profile": "work"
        }))
        .expect("a browser host of a machine of the list parses");
        assert_eq!(
            on_machine,
            Operation::BrowserHostRequest {
                ssh_target: None,
                machine: Some("rented-1".into()),
                profile: "work".into(),
                engine: None,
                command: None,
            },
            "a browser host of a rented machine carries no SSH target, and its connect uses that machine's login"
        );
        for unknown in [
            serde_json::json!({
                "operation": "server_link_request", "ssh_target": "server-vm", "address": "server:7000"
            }),
            serde_json::json!({
                "operation": "browser_host_request", "ssh_target": "browser-vm", "profile": "work",
                "backend": "chromium"
            }),
        ] {
            assert!(
                serde_json::from_value::<Operation>(unknown.clone()).is_err(),
                "a setting request names no other field: {unknown}"
            );
        }
    }

    /// The answer of one setting request names the person's press, and says whether
    /// the running Horizon follows a server entry now.
    #[test]
    fn the_answer_of_a_setting_request_names_the_press() {
        let confirmed = serde_json::to_value(Outcome::ServerLink {
            answer: Answer::Confirmed,
            start: Some(EntryStart::Linked),
            link: Some(ServerLinkState::InStep),
            reason: None,
        })
        .expect("serialize");
        assert_eq!(confirmed["status"], serde_json::json!("server_link"));
        assert_eq!(confirmed["answer"], serde_json::json!("confirmed"));
        assert_eq!(confirmed["start"], serde_json::json!("linked"));
        let declined = serde_json::to_value(Outcome::ServerLink {
            answer: Answer::Declined,
            start: None,
            link: None,
            reason: None,
        })
        .expect("serialize");
        assert!(
            declined.get("start").is_none(),
            "a person who pressed Not now starts nothing: {declined}"
        );
        assert_eq!(
            serde_json::to_value(Outcome::BrowserHost {
                answer: Answer::TimedOut
            })
            .expect("serialize")["answer"],
            serde_json::json!("timed_out")
        );
    }

    /// The answer of a confirmed server entry says whether the server answered, and
    /// the reason of a link that is away reaches the agent in words.
    #[test]
    fn a_confirmed_server_link_answers_with_the_state_of_its_link() {
        let in_step = serde_json::to_value(Outcome::ServerLink {
            answer: Answer::Confirmed,
            start: Some(EntryStart::Linked),
            link: Some(ServerLinkState::InStep),
            reason: None,
        })
        .expect("serialize");
        assert_eq!(in_step["link"], serde_json::json!("in_step"));
        assert!(
            in_step.get("reason").is_none(),
            "a link that answered names no reason: {in_step}"
        );
        let away = serde_json::to_value(Outcome::ServerLink {
            answer: Answer::Confirmed,
            start: Some(EntryStart::Linked),
            link: Some(ServerLinkState::Away),
            reason: Some("The key of the server is not the key that this computer knows.".into()),
        })
        .expect("serialize");
        assert_eq!(away["link"], serde_json::json!("away"));
        assert_eq!(
            away["reason"],
            serde_json::json!("The key of the server is not the key that this computer knows."),
            "the reason of a link that is away reaches the agent in words"
        );
    }

    #[test]
    fn a_setting_request_waits_for_the_person_and_never_for_an_hour() {
        for operation in [
            Operation::ServerLinkRequest {
                ssh_target: Some("server-vm".into()),
                machine: None,
                command: None,
            },
            Operation::BrowserHostRequest {
                ssh_target: Some("browser-vm".into()),
                machine: None,
                profile: "work".into(),
                engine: None,
                command: None,
            },
        ] {
            assert_eq!(
                operation.wait_timeout_millis(),
                Some(SETTING_REQUEST_TIMEOUT_MILLIS),
                "the card of a setting asks the person for five minutes"
            );
            assert_eq!(
                request_timeout(&operation),
                HOST_APPLY_TIMEOUT + Duration::from_millis(SETTING_REQUEST_TIMEOUT_MILLIS)
            );
        }
        assert_eq!(SETTING_REQUEST_TIMEOUT_MILLIS, 300_000);
    }

    #[test]
    fn an_oversized_request_is_refused_before_it_reaches_the_queue() {
        let root = tempfile::tempdir().expect("runtime root");
        let huge = serde_json::json!({"components": [{"type": "text", "text": "x".repeat(MAX_REQUEST_BYTES)}]});
        let refused = enqueue_at(
            root.path(),
            identity("horizon:agent-a"),
            Operation::Update {
                panel_id: "card".into(),
                document: huge,
            },
            Duration::from_secs(5),
        );
        assert!(refused.is_err());
        assert!(!has_pending_at(root.path(), super::super::host_instance()).expect("pending"));
    }
}
