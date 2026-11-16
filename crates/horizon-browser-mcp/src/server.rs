use horizon_browser::{BrowserControlAction, BrowserControlValue};
use horizon_browser_control::manifest::{self, AuditPageRequest};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        tool::ToolCallContext,
        wrapper::{Json, Parameters},
    },
    model::{
        CallToolRequestParams, CallToolResponse, Implementation, ListToolsResult, PaginatedRequestParams,
        ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
    tool, tool_router,
};

use crate::controller::{BrowserController, MAX_ACTION_TIMEOUT_MILLIS};
use crate::model::{
    ActInput, ActKind, ActionOutput, AttachedFileOutput, AuditInput, AuditOutput, BrowserListOutput, CloseInput,
    CloseOutput, CreateInput, CreateOutput, DownloadsInput, DownloadsOutput, EvaluateInput, EvaluateOutput,
    HandoffInput, HandoffOutput, HttpAuthInput, HttpAuthOutput, NavigateInput, NavigateOutput, NetworkInput,
    NetworkOutput, NetworkWatchInput, NetworkWatchOutput, NodeOutput, NodesOutput, PanelInput, QueryInput, ResizeInput,
    ResizeOutput, SnapshotInput, SnapshotOutput, VideoInput, VideoOutput, VisibilityInput, VisibilityOutput, WaitInput,
    WaitOutput, ZoomInput, ZoomOutput,
};
use crate::network_watch::NetworkWatchState;

/// The one tool that a card socket of a machine session lists.
const CARD_TOOL: &str = "block_panel";

/// What a card socket of a machine says: its one tool, and how an agent uses it.
const CARDS_INSTRUCTIONS: &str = "This socket serves the cards of one session of one machine, and nothing else: block_panel draws a card in the Horizon workspace of that machine, and the press of the person comes back from next_click. Only the card operations are served: no terminal, no browser panel and no machine key. Keep next_click open, because a card of a machine has no wake line. A call that a lost link cut answers with an error: call again.";

const DEFAULT_SNAPSHOT_NODES: u32 = 250;
const DEFAULT_QUERY_RESULTS: u32 = 50;

/// Stdio MCP service for live Horizon browser panels.
#[derive(Clone, Debug)]
pub struct HorizonBrowserMcp {
    controller: BrowserController,
    network_watch: NetworkWatchState,
    inline_screenshots: bool,
    tool_router: ToolRouter<Self>,
}

impl HorizonBrowserMcp {
    /// Build a service using the injected Horizon panel identity when present.
    #[must_use]
    pub fn from_environment() -> Self {
        Self::new(BrowserController::from_environment())
    }

    /// Build a service for the one agent of a host that runs in this
    /// process, such as a standalone remote host.
    #[must_use]
    pub fn for_host(actor: &str, host_instance: &str) -> Self {
        Self::new(BrowserController::for_host(actor, host_instance))
    }

    /// Build the service of one card socket of a machine session: it lists
    /// `block_panel` alone and refuses every operation outside the card ones.
    #[must_use]
    pub fn for_cards(actor: &str, host_instance: &str) -> Self {
        let mut server = Self::new(BrowserController::for_host(actor, host_instance).only_cards());
        let mut router = Self::tool_router();
        for tool in router.list_all() {
            if tool.name != CARD_TOOL {
                router.disable_route(tool.name.into_owned());
            }
        }
        server.tool_router = router;
        server
    }

    /// The names of the tools that this server lists, which a test of a card
    /// socket holds to the one tool of the card contract.
    #[cfg(test)]
    pub(crate) fn tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .iter()
            .map(|tool| tool.name.clone().into_owned())
            .collect()
    }

    fn new(controller: BrowserController) -> Self {
        crate::controller::spawn_attachment_janitor();
        Self {
            controller,
            network_watch: NetworkWatchState::default(),
            inline_screenshots: false,
            tool_router: Self::tool_router(),
        }
    }

    /// Also return the PNG of each `browser_screenshot` as an image block.
    #[must_use]
    pub fn with_inline_screenshots(mut self) -> Self {
        self.inline_screenshots = true;
        self
    }
}

#[tool_router]
impl HorizonBrowserMcp {
    #[tool(
        name = "device_panel",
        description = "Manage native Device viewers in the calling agent's current Horizon workspace; they are read-only for agents (a person may turn Interact on in the UI, no operation can). Operations: create(endpoint,identity?,ssh?), list, inspect(panel_id), visibility(panel_id,visible), reveal(panel_id), reconnect(panel_id), screenshot(panel_id,copy_to_clipboard?), close(panel_id). Screenshot returns a private PNG of the full connected desktop, excluding local crop/scale controls, with dimensions; copy_to_clipboard defaults false and clipboard_requested reports native host dispatch only. The latest eight exports survive until panel close or normal host exit. Screenshots do not change visibility, focus, canvas or Interact; disconnected/stopped/no-frame viewers refuse capture. Optional identity contains creator-supplied machine_name, hostname, numeric ip_addresses and tailscale_name, not verified identity. Optional ssh {host,user?,port?} reaches endpoint on that host's loopback through an SSH tunnel (ssh -W) using the Horizon machine's own SSH configuration and keys; the host must already be trusted in known_hosts, and never pass credentials, key paths or ssh options. An untrusted host is a refusal, not an unreachable host: while connection is disconnected, connection_failure types it as host_not_trusted or host_key_changed, with the ssh host and port, the observed key type and SHA256 fingerprint when a probe saw one, and the action a person takes; the probe repeats the tunnel's own ssh configuration, so the fingerprint is the key ssh saw, and it is absent when a ProxyJump, ProxyCommand or HostKeyAlias keeps a direct probe from repeating that connection; no operation trusts a host, and Horizon accepts no key, fingerprint or ssh option. Inspect/list report ssh for tunnelled viewers. A viewer whose endpoint and ssh route do not change reconnects by itself after a lost link, with waits of 3, 6, 12, 24 and then 30 seconds, reset by a connection that lasted ten seconds: while it waits, connection is reconnecting, reconnect.attempt counts the next attempt from 1, reconnect.next_attempt_in_millis gives the wait, and connection_error keeps the words of the loss, while the last image stays on screen and image_displayed is false. A refused host key is never retried and stays disconnected with connection_failure. Only a person stops the attempts, with the Stop control of the viewer; no operation stops or starts one, so do not reconnect a viewer that is reconnecting. Inspect/list return identity and server details (name and desktop_size); server details are last-observed when disconnected and cleared on reconnect. Endpoints must be explicit numeric loopback addresses with nonzero ports. Create returns a stable id immediately, not proof of a live image: inspect connection, image_received, image_displayed and frame_sequence. Viewers that are not drawn (hidden, off canvas, behind a fullscreen panel) keep receiving and keep uploading about once a second, so image_received and frame_sequence stay live evidence while image_displayed is false; received_frame_sequence counts worker image updates independently of those uploads. Both reset on reconnect; neither is a heartbeat, so an unchanged desktop is not a connection failure. A viewer displayed earlier in this connection (last_displayed_age_millis present) that now reports not_rendered with exclusion outside_canvas was navigated away from by the person: keep testing and recording, do not pause, and do not reveal just to advance counters. Visible means host presentation state; image_displayed describes the latest completed UI frame. New hosts return diagnostics with decoded frames, sampling pause, frame ages and presentation reason. Static frames alone never prove a stalled stream. Reveal is for first presentation only: once this viewer has presented a non-discarded image, it returns navigation_preserved without changing visibility, fullscreen, focus or canvas, including after reconnect or ownership transfer. Restored viewers also return navigation_preserved when prior display history is unavailable: reconnect grants transport ownership, never navigation permission. Continue background testing and inspect; use the UI to return to the viewer. Never close/recreate a viewer to bypass this protection. The first Reveal brings an owned viewer into view without reconnecting or stealing keyboard focus, then answers once the host has drawn the connected image after the reveal (image_displayed true), or after at most three seconds with the observation explaining why not (presentation and diagnostics.host.exclusion); stopped or disconnected viewers answer at once. In the reveal answer image_displayed is true only for a draw after the reveal applied; presentation still describes the latest pass. A host that runs no UI frames cannot draw the viewer and answers when the bound expires. A drawn image proves presentation, not motion. Do not ask a human to verify visibility. Reconnect explicitly acquires an unowned/restored viewer; another owner's viewer cannot be mutated. Video records the full VNC desktop as private WebM: operation video with action start, status, or stop. It requires the exact owning agent for every action, continues offscreen, has no audio, targets 5 fps at the full desktop size, and stops on disconnect, after five minutes, or at 256 MiB. The host writes a frame only when the desktop changed, so a screen that changes less often keeps every state and reports a low effective_fps. recording.capture.quality is unknown, good, degraded or poor: degraded and poor mean the host wrote fewer of the states the desktop offered than it could, so a brief state can be absent, and that must be reported instead of claiming motion evidence. Stop is asynchronous; poll status until recording.capture.active and recording.finalizing are both false, then check encoder_failed, quality, frames_encoded and frames_offered before using the host-local path. Copy the file before panel close or four subsequent recordings. Close releases only the owned viewer connection and never terminates its target. Restores never reconnect automatically. Requires a supporting Horizon host; host response is bounded to 15 seconds. Native input is separate, through explicitly configured standalone device CLI/MCP targets."
    )]
    async fn device_panel(
        &self,
        Parameters(input): Parameters<horizon_browser_control::manifest::device::DevicePanelInput>,
    ) -> Result<Json<horizon_browser_control::manifest::device::Outcome>, String> {
        self.controller.device_panel(input.into_operation()).await.map(Json)
    }

    #[tool(
        name = "block_panel",
        description = "Draw a card in the calling agent's current Horizon workspace. A card is a small surface of plain text that the agent composes: a heading, body text, label and value rows, a badge, a progress bar, a divider, selectable rows, buttons and links. Horizon draws it beside the person's terminals. The person's presses reach the agent that owns the card: each click on a button or on a selectable row is one click event, and next_click answers the oldest click that waits. Operations: create(title?,document), update(panel_id,document), list, inspect(panel_id), next_click(panel_id?,timeout_millis?), visibility(panel_id,visible), close(panel_id), open_terminal(ssh?,remote_command?,title?,key?,machine?), open_panel(preset,directory,title?), presets, machine_set(name,ssh,key?,session?,provider?,repository?,folder?), machine_delete(name), machines, secret_names, machine_key(name), machine_keys, delete_machine_key(name). The document is {\"components\":[...]} with at most 64 components in draw order, and the whole document is at most 32 KiB. Horizon reads it as an object or as the JSON text of the object, because a caller that builds every tool argument as a string can only send the text. Each component carries plain text only: no images, tables or inputs, and no field that Horizon fetches or opens by itself. A link component {label,url} is the one field that Horizon opens, and only when the person presses it: url is an absolute http or https URL with a host, 1 to 2048 characters, without credentials, spaces or control characters, and a card holds at most 8 links with a label of 1 to 80 characters; a violation is invalid_document at the path of the field. The card draws the label as a link and the host of the url under it, and a press opens the url in a new browser panel of the cards workspace beside the card, through the same path as a browser panel of the canvas picker, so the browser host and profile rules of the settings apply. A press sends no click and reaches no agent, so a link still works while the agent that drew it is gone, and nothing is fetched until the person presses. Use a link for a page the person should open; never put a token or a signed URL in one. An invalid document is refused as invalid_document or document_too_large, and the message names the offending path, such as components[2].items[0].id. Create draws the card in the caller's workspace without taking keyboard focus and returns panel_id and revision 1; the panel is visible until someone hides it. Update replaces the whole document and returns the next revision; the revision rises with every new document, so a card and the person's screen are described by the latest revision. Only the agent that created a card may update, hide, show, close or wait on it: another agent of the same workspace gets not_owner, and Horizon refuses a ninth card of one agent in one workspace with too_many_cards. A list with selection single lets the person select one row; the selection is Horizon's own state, is not saved, and is dropped for a row that a new document no longer holds. A row with selected true is the row that the list starts with: a click reports it until the person selects another row. Only a list with selection single may hold a selected row, and at most one row of a list. next_click answers {click: {event_id, panel_id, component_id, revision, selected, at_millis, stale}} as soon as a click waits, and {timed_out: true} when timeout_millis passes without one; timeout_millis is 1000-3600000 (default 300000). component_id is the button id or list row id the person hit. revision is the document revision the person saw, not the current one. selected holds the row ids that were selected at that moment in document order, so a Continue button tells the agent which option was chosen. stale is true when the document changed after the person pressed, so confirm the intent before acting on a stale click. list and inspect report pending_clicks and listening; listening is true while a next_click of the owner waits on that card, alone or on every card of the owner, which delivers the next click at once and types nothing into the agent's terminal. Without a waiting next_click, a click is kept for 5 minutes, at most 32 per card in insertion order, and Horizon wakes the owner by typing one fixed line into the owner's agent terminal: A button was pressed on your Horizon card. Call the block_panel tool with operation next_click to read it. That line arrives only when an agent panel of the owner is idle and holds no keyboard focus, at most once per streak of clicks and at most every 30 seconds per agent panel; a card cannot put its own text into a terminal. A click on a full queue is dropped and the card says that too many clicks wait; an older click is never dropped for a newer one. Every click reaches an agent at most once, so keep a listen loop: call next_click, act, update the card, call next_click again, and on timed_out call it again while the card matters. Without panel_id, next_click waits on every card that the caller owns and answers the oldest click of those cards first; that is the normal listen loop, because the person presses whichever card they see. One next_click waits per card, and one without panel_id waits per agent: a later wait of the same form answers the older one with timed_out first, and a wait that names a card and an any-card wait of its agent run together, where a click goes to the wait that names its card first. An any-card wait of an agent without a card answers no_cards. Closing a card answers a waiting next_click with panel_unavailable. When no agent panel of the owner is in the workspace, buttons and rows are drawn disabled, every badge is drawn neutral with the prefix \"Last seen: \" and a status frame says Its agent is not here. The person keeps the last document. list and inspect work for every agent of the workspace: a card of another workspace is panel_unavailable, and a panel that is not a card is not_block_panel. inspect returns the document, its revision, whether the card is visible, whether an agent panel of its owner is still in the workspace (agent_present; false means the person keeps the last document with no agent behind it), and owned_by_caller. Every operation is scoped to the agent panel that Horizon launched and to its workspace: a caller without that panel gets caller_unavailable, and a request of another host gets wrong_host. Text on a card is the agent's claim, not verified Horizon state, and the card can look like a Horizon dialog; never show a credential, token or private key on one. open_terminal opens an SSH terminal panel in the caller's workspace, beside the calling agent panel, without taking keyboard focus, and answers {panel_id}. It names either ssh or machine, never both and never neither: the other target is invalid_request, and the message names the field. Its ssh object holds host and optional user and port, and nothing else: Horizon builds the ssh command line itself, with the port, the host, and the remote command as one argument, and adds -t when remote_command is present, so tmux new -A -s name gets a terminal. host and user accept letters, digits, '.', '_' and '-' (and ':' in host for IPv6) and cannot start with '-'; a port is 1-65535; a host, user or port that breaks a rule is invalid_ssh_route. remote_command is one line of letters, digits, spaces and '. _ : / , = @ + -', at most 512 characters, and cannot start with '-' or hold a quote, ';', '&', '|', '$', a backtick, '<', '>', '(', ')', a backslash or a control character; a violation is invalid_remote_command. An agent passes no ssh option, identity file, extra argument or environment, so it cannot inject one. title is one line of at most 80 characters and defaults to user@host. One agent panel may open at most eight terminals in one Horizon run (too_many_terminals), and a terminal that Horizon cannot open is terminal_unavailable. The titlebar of a panel opened this way carries the Horizon mark \"opened by <agent>\", which is saved with the panel. The panel is an ordinary SSH terminal afterwards: the person types in it and closes it, and no operation of this tool reads it, writes to it or closes it. open_panel opens a panel of one preset of the person's settings beside the calling agent panel, in the caller's workspace, without taking keyboard focus, and answers {panel_id}. It is how an agent shows a task that runs in its own folder: pass the name or the alias of a preset exactly as the person saved it, so Horizon takes the command, the arguments and the kind from that preset, and pass the absolute folder that the panel runs in. The caller passes no command, no argument, no environment and no prompt, so it cannot change what the preset runs; a name that this Horizon does not keep is unknown_preset and the message lists the names of the presets and no command. directory is an absolute path of at most 1024 bytes without a control character, and it must be a directory of this computer: a folder that is not there, a relative path and a file that is not a folder are directory_missing or invalid_request. title is one line of at most 80 characters and defaults to the name of the preset. A preset of a kind that runs no local program, such as a browser page, a Device viewer, a card or an SSH terminal, is invalid_request, because the folder of the panel is the point of this operation. One agent panel opens at most eight panels in ten minutes and the ninth answer is rate_limited with the seconds to wait; a panel that Horizon cannot open is panel_unavailable. The titlebar of the panel carries the same mark as a terminal, opened by <agent>, and the panel is the person's afterwards: no operation of this tool reads it, writes to it or closes it. presets answers {presets: [{name, kind}]} with the presets that open_panel can open a panel from, in the order of the settings, and it is a read: an entry is the name to pass and the kind as the settings store it, such as claude, codex or shell, and a command, an argument, an alias and a folder of a preset are in no answer. A preset of a browser page, a Device viewer, a card or an SSH terminal runs no program of this computer, so it has no folder and is left out. Compare the kind with the panel that runs you to choose a helper of your own kind, and compare a name exactly: it is the person's text. machine_set(name,ssh,key?,session?,provider?,repository?) adds or replaces one machine of this Horizon and answers {name, created}. A machine is a saved route that the person can connect with one click in the sidebar, without an agent: name, the ssh route, an optional machine key, an optional tmux session, an optional one-line provider text, at most 60 characters, such as \"Hetzner, hel1\", which is your claim and which Horizon draws dimmed, and an optional one-line repository, at most 60 characters, such as example-org/shop, which Horizon draws under the name of the machine in the sidebar; leave the repository out while the machine is ready for none. folder is the folder that the machine holds the repository in, such as /home/dev/example-repo: one absolute path of at most 512 bytes with no control character, and a new terminal of a workspace placed on this machine starts there, so pass the folder that you read with realpath after you brought the repository; a path outside the rule is invalid_folder, and one repository folder for each machine is the rule of this version. session is 1 to 40 characters of A-Z, a-z, 0-9, _ and - and cannot start with -, because the terminal runs tmux new -A -s <session>; a name outside the rule is invalid_session_name. A machine name follows the machine key rule (invalid_machine_name) and a key name outside it is invalid_key_name. The record holds no command line: Horizon builds the ssh argv and the tmux command from the record when a panel opens, so a saved record can never run a saved command. The machines belong to the person and to this Horizon home, not to a workspace or a session: every agent panel of this Horizon sees them, they survive a restart, and one Horizon keeps at most 64 of them (too_many_machines). machine_delete(name) forgets one record and answers {name}; it never deletes the key, so call delete_machine_key after you delete the machine. An unknown name is machine_unavailable. machines answers {machines: [{name, host, user, port, key, session, provider, repository, folder, added_at_millis, terminal_open, key_missing}]} in name order, where terminal_open says whether a terminal of that machine is open in this Horizon now; it is not a reachability check, and Horizon never polls a machine. secret_names answers {names: [...]} with the names of the named secrets that an agent panel of this computer receives as environment variables when it starts, in name order; it is a read, no value and no length or hash of a value is in the answer, and a panel keeps the environment of its own start, so a secret that a person stores while your panel runs reaches the panel that starts after it and not yours. key_missing is true when the record binds a key that is not on this computer, because it was deleted here or on another computer: a click on that machine opens nothing and says so, so call machine_key and install the new public line on the machine. open_terminal with machine takes ssh, key and remote_command from the record, so pass none of them: the panel that opens is the panel that a click on the machine row opens, and a second click focuses it. A record that no longer passes its rules opens nothing. When the person chooses to forget a machine in the sidebar, the record goes and the key stays until delete_machine_key. machine_key(name) makes or reads the SSH key that Horizon keeps for one rented machine, and answers {name, public_key, private_key_path, known_hosts_path, created}; put public_key in the authorized_keys of the machine at rent time, call open_terminal with key set to that name, and delete_machine_key(name) once the machine is deleted. The two paths are absolute paths of Horizon files: private_key_path is the private half, which you may read and pass to ssh -i in your own shell for the commands that set the machine up, and known_hosts_path is the known-hosts file of that key alone. The text of the private key is in no answer, log line or message. A name is 1 to 40 characters of 'a-z', '0-9' and '-', with no '-' at either end (invalid_key_name), and one Horizon home holds at most 64 keys (too_many_keys). Horizon makes an ed25519 pair with no passphrase under its own home, answers the public line and the two paths, and reports created false when the pair was already there. With key, Horizon adds -i <its private key> -o IdentitiesOnly=yes -o IdentityAgent=none -o UserKnownHostsFile=<its known-hosts file for that key> -o StrictHostKeyChecking=accept-new to the command line it builds, so the machine sees that one key, the person's own keys and ssh agent are not offered, and first contact with the host key is pinned for that key alone. It never takes a path, a passphrase, a key file or an ssh option from you, and the text of the private half never appears in an answer, a log line or a card. The keys belong to the Horizon home and not to a workspace, so every agent panel of this Horizon may use any of them by name: a later agent, in any workspace, reaches a machine that an earlier agent rented. machine_keys answers {keys: [{name, public_key, private_key_path, known_hosts_path, created_at_millis}]} for this Horizon home. delete_machine_key removes the pair and its known-hosts file, so both paths stop existing. An unknown or deleted key is key_unavailable, and a restored panel whose key was deleted still starts and shows ssh's own error. Tell the person on a card what the terminal is for. A request expires if the host does not apply it within 10 seconds plus any next_click wait, and the call answers host_timeout 5 seconds after that. Requires a supporting Horizon host."
    )]
    async fn block_panel(
        &self,
        Parameters(input): Parameters<horizon_browser_control::manifest::block::BlockPanelInput>,
    ) -> Result<Json<horizon_browser_control::manifest::block::Outcome>, String> {
        self.controller.block_panel(input.into_operation()).await.map(Json)
    }

    #[tool(
        name = "browser_screenshot",
        description = "Capture the latest decoded viewport pixels of a ready browser panel in the calling agent's Horizon workspace. Returns a private PNG path and its original dimensions; this is a retained frame, not a fresh navigation or a full-page screenshot. Optional copy_to_clipboard (default false) also requests an image copy on the Horizon host clipboard; clipboard_requested reports dispatch, not OS acknowledgement. Requires a live Horizon host, not a standalone browser. Acquires or renews the caller's browser claim and rechecks it at host dispatch and result delivery. Refuses another live owner, active human steering and pending handoff. Does not change focus, viewport, visibility or canvas. Keep sensitive screenshots private. The latest eight exports per panel survive until panel close or host exit."
    )]
    async fn browser_screenshot(
        &self,
        Parameters(input): Parameters<horizon_browser_control::manifest::device::ScreenshotInput>,
    ) -> Result<Json<horizon_browser_control::manifest::device::Screenshot>, String> {
        use horizon_browser_control::manifest::device::{Operation, Outcome};
        match self
            .controller
            .device_panel(Operation::BrowserScreenshot { input })
            .await?
        {
            Outcome::Screenshot { capture } => Ok(Json(capture)),
            Outcome::Failed { code, message } => Err(format!("{code}: {message}")),
            _ => Err("host returned an unexpected screenshot result".into()),
        }
    }

    #[tool(
        name = "browser_remote_allocations",
        description = "List remote allocations owned by the calling agent in its current workspace, including unresolved holds after panels disappear. Use operation=list, then operation=reconcile with one returned reference to check that exact retired session at its original provider. This read-only provider check takes up to 15 seconds. Active, unidentified, unauthenticated or uncertain sessions keep their capacity. Only exact-session absence frees the corresponding lease. Repeated reconciliation is safe; credentials and provider session identifiers are never returned."
    )]
    async fn browser_remote_allocations(
        &self,
        Parameters(input): Parameters<crate::model::RemoteAllocationsInput>,
    ) -> Result<Json<crate::model::HostJson>, String> {
        self.controller
            .remote_allocations(input.reference()?)
            .await
            .map(|value| Json(crate::model::HostJson(value)))
    }

    #[tool(
        name = "browser_provider_usage",
        description = "Read shared remote-provider usage without creating a browser session. Omit provider to list all configured profiles or pass one configured provider name. Each profile uses its own configured credential bindings. Returns running/allowed and queued counts, the actual sample time, and per-provider errors or unsupported status. Counts include competing users and CI jobs and are informational, never a capacity reservation. Requires the same live Horizon host identity as remote session creation; a local standalone browser host has no remote-provider configuration. Never supply credentials or endpoints."
    )]
    async fn browser_provider_usage(
        &self,
        Parameters(input): Parameters<crate::controller::provider_usage::ProviderUsageInput>,
    ) -> Result<Json<crate::model::HostJson>, String> {
        self.controller
            .provider_usage(input.provider)
            .await
            .map(|value| Json(crate::model::HostJson(value)))
    }

    #[tool(
        name = "browser_provider_devices",
        description = "Discover any browser, OS and device combination currently offered by a configured remote provider account, without allocating a session. Pass provider and optional search words and offset. Returns at most 50 combinations and next_offset; pass a returned target to browser_create with backend omitted. No preconfigured device target is needed. Availability in this catalog is not account entitlement, live capacity or a reservation. The host uses only its configured or explicitly granted credentials; never supply credentials, endpoints or raw capabilities."
    )]
    async fn browser_provider_devices(
        &self,
        Parameters(input): Parameters<crate::controller::provider_usage::ProviderDevicesInput>,
    ) -> Result<Json<crate::model::HostJson>, String> {
        self.controller
            .provider_devices(input)
            .await
            .map(|value| Json(crate::model::HostJson(value)))
    }

    #[tool(
        name = "browser_list",
        description = "List live Horizon browser panels with their safe capabilities. For an agent identity injected by Horizon, only panels in that agent's current workspace are listed; identities from outside Horizon keep unscoped discovery. A panel's visible field is host presentation state, not workspace membership. Every panel reports profile: the shared profile that it uses, or `private`; a remote session reports none. Raw CDP, BiDi, and WebDriver endpoints are never exposed."
    )]
    fn browser_list(&self) -> Result<Json<BrowserListOutput>, String> {
        let panels = self.controller.list_panels().map_err(|error| error.to_string())?;
        Ok(Json(BrowserListOutput { panels }))
    }

    #[tool(
        name = "browser_create",
        description = "Create a browser panel in the calling agent's Horizon workspace and wait until its backend is ready and, when url is given, that page has committed; if the page has not committed within a bounded startup wait the panel is still returned with navigation=pending, and if the first page failed to load it is returned with navigation=failed and navigation_error (the panel is controllable; navigate again). Use this when browser_list is empty, or with an explicit backend to open a panel of another engine beside your panel. Reuse an existing panel for iframes, popups, dialogs, and consent flows; never create a helper panel for them. An agent may hold one panel per browser engine: with a Chromium panel open, backend=firefox or backend=safari opens a panel of that engine without allow_additional. A second panel of the same engine, a create without backend while the agent owns a panel, or a remote target while it owns one, needs allow_additional=true, which you set only when the user explicitly requests an independent session. Set visible=false for a live background panel; omit backend to use Horizon's configured browser. Set target to a reference returned by browser_provider_devices or a configured remote target name to run the session at that remote target instead of a local browser (Horizon resolves the provider, capabilities and credentials; the panel then reports remote_target and classic WebDriver without network capture; the allocated device is verified against the target's requirement from the provider's own evidence, a physical requirement the evidence does not confirm fails as remote_device_rejected after Horizon attempts to release the session, and a ready panel reports remote_device) and omit backend. Optional orientation=portrait or landscape requires target, overrides configured orientation for this session only, and requires matching orientation measured on the first committed document before readiness; a pending, failed or unverified first document is refused with a typed orientation error and exact-session release is attempted. Panels report remote_orientation and orientation_support. Bare hostnames default to HTTPS and explicit HTTP is preserved. A create that names no profile opens in the profile of the browser host when the settings name one, and in the shared profile `default` of this computer otherwise. A create that writes a profile name opens in that shared profile of this computer, and `default` is such a name: the answer says in `note` where the page runs. A create takes a profile of this computer or a target, never both: a remote session runs in the profile of its provider, so a create that names both is refused with a message that names profile and target. The shared profile is where the user signed in, so a new panel of it is signed in too, and a sign-out in one panel signs out every panel of that profile. `private` opens a browser of its own that starts signed in nowhere and leaves no folder at close. A browser host serves several panels of one profile, for Chromium and for Firefox: each panel is its own page of the one browser of that profile, so a second create is signed in where the first one signed in, and on a browser host `private` opens a page of an isolated browser context of that machine, which reads none of the profile's sign-ins and which the browser removes when the page closes. Every panel reports profile: the shared profile that it uses, or `private`."
    )]
    async fn browser_create(&self, Parameters(input): Parameters<CreateInput>) -> Result<Json<CreateOutput>, String> {
        if let Some(refusal) = input.refusal() {
            return Err(refusal);
        }
        // Matches the request normalization: a blank url is no navigation.
        let url_requested = input.url.as_deref().is_some_and(|url| !url.trim().is_empty());
        let receipt = self
            .controller
            .create(
                manifest::RemoteCreateParameters {
                    url: input.url,
                    backend: input.backend.map(Into::into),
                    target: input.target,
                    orientation: input.orientation.map(Into::into),
                    visible: input.visible.unwrap_or(true),
                    profile: input.profile,
                },
                input.allow_additional.unwrap_or(false),
                input.timeout_millis,
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(Json(CreateOutput {
            action_id: receipt.action_id,
            panel: receipt.panel,
            navigation: crate::model::CreateNavigationState::resolve(receipt.navigation, url_requested),
            navigation_error: receipt.navigation_error,
            startup_millis: receipt.startup_millis,
            note: receipt.note,
        }))
    }

    #[tool(
        name = "browser_duplicate",
        description = "Open the source panel's current URL in a new panel in the same workspace, sharing cookies and persistent website storage. Local Chromium and Firefox. Navigation, input, and automation remain independent; logout affects all shared panels. Does not copy unsaved forms, history, or live JavaScript state. Use only when additional panels are requested. A private source panel opens another private panel, which shares no sign-in with it: use browser_create with a profile name when the new panel must be signed in."
    )]
    async fn browser_duplicate(
        &self,
        Parameters(input): Parameters<crate::model::DuplicateInput>,
    ) -> Result<Json<CreateOutput>, String> {
        let receipt = self
            .controller
            .duplicate(&input.panel_id, input.visible.unwrap_or(true), input.timeout_millis)
            .await
            .map_err(|error| error.to_string())?;
        Ok(Json(CreateOutput {
            action_id: receipt.action_id,
            panel: receipt.panel,
            navigation: crate::model::CreateNavigationState::resolve(receipt.navigation, true),
            navigation_error: receipt.navigation_error,
            startup_millis: receipt.startup_millis,
            note: receipt.note,
        }))
    }

    #[tool(
        name = "browser_visibility",
        description = "Show or hide a live browser panel without stopping its browser session, network capture, ownership, or MCP control. Hidden panels remain listed and auditable. For a Horizon-injected agent identity the panel must already be in the calling agent's workspace; visible=true never moves a panel there."
    )]
    async fn browser_visibility(
        &self,
        Parameters(input): Parameters<VisibilityInput>,
    ) -> Result<Json<VisibilityOutput>, String> {
        let receipt = self
            .controller
            .set_visibility(&input.panel_id, input.visible, input.timeout_millis)
            .await
            .map_err(|error| error.to_string())?;
        Ok(Json(VisibilityOutput {
            action_id: receipt.action_id,
            panel: receipt.panel,
        }))
    }

    #[tool(
        name = "browser_close",
        description = "Close a browser panel you own in your workspace and stop its session. Use it when the user is done with the panel or when a remote device session must be released now; the call returns once the session teardown has completed, so the panel is gone from browser_list and a remote allocation has been released (a remote release can take up to about 35 s; a teardown_timeout error means the panel is closed but teardown was still running when timeout_millis elapsed). Read anything you still need from browser_audit first: the journal file is retained on disk, but browser_audit answers only for a live panel. It is not a way to hide a panel (use browser_visibility) and cannot close panels outside the calling agent's workspace."
    )]
    async fn browser_close(&self, Parameters(input): Parameters<CloseInput>) -> Result<Json<CloseOutput>, String> {
        let receipt = self
            .controller
            .close(&input.panel_id, input.timeout_millis)
            .await
            .map_err(|error| error.to_string())?;
        Ok(Json(CloseOutput {
            action_id: receipt.action_id,
            panel_id: receipt.panel_id,
            closed: true,
        }))
    }

    #[tool(
        name = "browser_navigate",
        description = "Navigate a live browser panel through Horizon's audited backend-neutral control path and report a typed outcome. By default it returns once the top-level document committed (wait=commit); wait=dispatched returns as soon as the engine handed the command to the backend, without awaiting the browser's acceptance (a rejection after that point shows as a failed page state, not as an action error; use commit when you need confirmation), and wait=dom_content_loaded after DOMContentLoaded. The result carries requested_url, committed_url, title, loading, redirected, elapsed_millis and state; check completed, because a wait that exceeds timeout_millis (1000-60000 ms, enforced in full by the engine; smaller values are raised to 1000) returns state=timed_out with the latest page state instead of an error. Unreachable destinations and rejected commands are errors. Safari's classic WebDriver has no dispatch-only navigation: every wait returns once the page loaded or the bound elapsed."
    )]
    async fn browser_navigate(
        &self,
        Parameters(input): Parameters<NavigateInput>,
    ) -> Result<Json<NavigateOutput>, String> {
        let wait = input.wait.unwrap_or_default();
        let receipt = self
            .controller
            .execute_engine_bounded(
                &input.panel_id,
                BrowserControlAction::Navigate {
                    url: input.url,
                    wait: wait.into(),
                    timeout_millis: Some(bounded_navigation_timeout(input.timeout_millis)),
                },
                bounded_navigation_timeout(input.timeout_millis),
            )
            .await
            .map_err(|error| error.to_string())?;
        match receipt.value {
            BrowserControlValue::Navigation { navigation } => Ok(Json(NavigateOutput::from_outcome(
                input.panel_id,
                receipt.action_id,
                navigation,
            ))),
            _ => Err("browser returned an unexpected navigation result".to_string()),
        }
    }

    #[tool(
        name = "browser_orientation",
        description = "Rotate a remote device to portrait or landscape and wait for device and measured page acknowledgement. Check orientation_support first; unsupported endpoints and local browsers return orientation_unsupported. Returns requested/applied orientation and CSS viewport dimensions. Reacquire semantic refs afterwards. A timeout can follow an applied rotation; inspect the panel before retrying. Preserves ownership and handoff rules."
    )]
    async fn browser_orientation(
        &self,
        Parameters(input): Parameters<crate::model::OrientationInput>,
    ) -> Result<Json<crate::model::OrientationOutput>, String> {
        let action = input.action()?;
        let receipt = self
            .controller
            .execute_engine_bounded(&input.panel_id, action, input.timeout_millis())
            .await
            .map_err(|error| error.to_string())?;
        crate::model::OrientationOutput::from_result(input.panel_id, receipt.action_id, &receipt.value).map(Json)
    }

    #[tool(
        name = "browser_resize",
        description = "Set an existing local Chromium or Firefox panel's exact page content viewport in CSS pixels (320-8000 per axis). Supply both width and height, or reset=true without dimensions to resume host panel sizing. Waits for measured innerWidth/innerHeight and returns requested and applied sizes and zoom_percent (under a browser_zoom page zoom, applied is requested divided by zoom_percent/100); host layout and navigation preserve the pin until reset or session replacement. The canvas panel is unchanged and letterboxes the page. Remote devices return remote_viewport_fixed; use browser_orientation to rotate a supported remote device; unsupported backends return viewport_unsupported. Preserves workspace ownership, user steering and handoff rules. Video encoding dimensions and max_width do not change this viewport."
    )]
    async fn browser_resize(&self, Parameters(input): Parameters<ResizeInput>) -> Result<Json<ResizeOutput>, String> {
        let action = input.action()?;
        let timeout = input.timeout_millis();
        let receipt = self
            .controller
            .execute_engine_bounded(&input.panel_id, action, timeout)
            .await
            .map_err(|error| error.to_string())?;
        ResizeOutput::from_result(input.panel_id, receipt.action_id, &receipt.value).map(Json)
    }

    #[tool(
        name = "browser_zoom",
        description = "Zoom an existing local Chromium or Firefox panel's page like the browser's zoom level, for reflow checks such as 200 percent: percent 25-500, or reset=true for 100. The page lays out in a CSS viewport of the panel size (or the browser_resize pin) divided by percent/100, media queries follow it, and the panel magnifies the frame; devicePixelRatio does not change. Waits for the measured innerWidth/innerHeight and returns requested_percent, applied_percent and css_viewport. The zoom survives navigation and host layout until reset or session replacement; browser_resize results report zoom_percent. A zoom whose CSS viewport would exceed 8000 pixels per axis is refused with invalid_input; if the panel grows later, the zoomed viewport stops at 8000 pixels on that axis. Safari and remote sessions return unsupported_backend. Preserves workspace ownership, user steering and handoff rules."
    )]
    async fn browser_zoom(&self, Parameters(input): Parameters<ZoomInput>) -> Result<Json<ZoomOutput>, String> {
        let action = input.action()?;
        let timeout = input.timeout_millis();
        let receipt = self
            .controller
            .execute_engine_bounded(&input.panel_id, action, timeout)
            .await
            .map_err(|error| error.to_string())?;
        ZoomOutput::from_result(input.panel_id, receipt.action_id, &receipt.value).map(Json)
    }

    #[tool(
        name = "browser_snapshot",
        description = "Return a bounded semantic snapshot with short-lived refs, including child frames on local Chromium and Firefox. Use frame refs for click and fill. Take a new snapshot after navigation or page changes."
    )]
    async fn browser_snapshot(
        &self,
        Parameters(input): Parameters<SnapshotInput>,
    ) -> Result<Json<SnapshotOutput>, String> {
        let receipt = self
            .controller
            .execute(
                &input.panel_id,
                BrowserControlAction::Snapshot {
                    max_nodes: input.max_nodes.unwrap_or(DEFAULT_SNAPSHOT_NODES),
                },
                input.timeout_millis,
            )
            .await
            .map_err(|error| error.to_string())?;
        let BrowserControlValue::Snapshot { snapshot } = receipt.value else {
            return Err("browser returned an unexpected snapshot result".to_string());
        };
        Ok(Json(SnapshotOutput::new(input.panel_id, receipt.action_id, snapshot)))
    }

    #[tool(
        name = "browser_query",
        description = "Query the document and child frames on local Chromium and Firefox with a CSS selector. Use returned refs to click or fill fields inside cross-origin frames. Other backends query only the top-level document."
    )]
    async fn browser_query(&self, Parameters(input): Parameters<QueryInput>) -> Result<Json<NodesOutput>, String> {
        let receipt = self
            .controller
            .execute(
                &input.panel_id,
                BrowserControlAction::Query {
                    selector: input.selector,
                    max_results: input.max_results.unwrap_or(DEFAULT_QUERY_RESULTS),
                },
                input.timeout_millis,
            )
            .await
            .map_err(|error| error.to_string())?;
        let BrowserControlValue::Nodes {
            generation,
            revision,
            nodes,
        } = receipt.value
        else {
            return Err("browser returned an unexpected query result".to_string());
        };
        Ok(Json(NodesOutput {
            panel_id: input.panel_id,
            action_id: receipt.action_id,
            generation,
            revision,
            nodes: nodes.into_iter().map(NodeOutput::from).collect(),
        }))
    }

    #[tool(
        name = "browser_act",
        description = "Click (including trusted double-click with count=2), fill, press a key or chord on the focused element (key such as Tab, Shift+Tab, Enter, Escape, ArrowDown or Control+a, optional count 1-100; read focus afterward with browser_evaluate or browser_screenshot), scroll, reload, go back, go forward, drop_files on a visible drop target (local Chromium and Firefox), or set_files to attach host files to an input[type=file] (target the input itself, even when hidden; files are absolute paths under your work root; the result lists the attached names and sizes). Prefer a fresh ref from browser_snapshot or browser_query over a selector."
    )]
    async fn browser_act(&self, Parameters(input): Parameters<ActInput>) -> Result<Json<ActionOutput>, String> {
        // Request shape errors are backend-independent: the builder and the
        // protocol check run before any backend or panel state is consulted.
        let action = input
            .build_action()
            .map_err(|message| format!("browser action refused (invalid_input): {message}"))?;
        action
            .validate()
            .map_err(|message| format!("browser action refused (invalid_input): {message}"))?;
        self.controller
            .validate_attachment_target(&input.panel_id, &action)
            .map_err(|error| error.to_string())?;
        let action_kind = input.action;
        let receipt = self
            .controller
            .execute(&input.panel_id, action, input.timeout_millis)
            .await
            .map_err(|error| error.to_string())?;
        require_action_completed(action_kind, &receipt.value)?;
        let mut output = ActionOutput::completed(input.panel_id, receipt.action_id);
        if let BrowserControlValue::Files { files } = receipt.value {
            output.files = Some(files.into_iter().map(AttachedFileOutput::from).collect());
        }
        Ok(Json(output))
    }

    #[tool(
        name = "browser_evaluate",
        description = "Evaluate an explicit JavaScript expression in the top-level document. Promises are awaited within timeout_millis (1-60000 ms, default 15000), including queue time; expiry returns evaluation_timeout naming the bound and elapsed time. The expression is audited only by character count."
    )]
    async fn browser_evaluate(
        &self,
        Parameters(input): Parameters<EvaluateInput>,
    ) -> Result<Json<EvaluateOutput>, String> {
        let timeout_millis = bounded_evaluation_timeout(input.timeout_millis);
        let receipt = self
            .controller
            .execute_engine_bounded(
                &input.panel_id,
                BrowserControlAction::Evaluate {
                    expression: input.expression,
                    timeout_millis: Some(timeout_millis),
                },
                timeout_millis,
            )
            .await
            .map_err(|error| error.to_string())?;
        let BrowserControlValue::Json { value } = receipt.value else {
            return Err("browser returned an unexpected evaluation result".to_string());
        };
        Ok(Json(EvaluateOutput {
            panel_id: input.panel_id,
            action_id: receipt.action_id,
            value,
        }))
    }

    #[tool(
        name = "browser_network",
        description = "Start, inspect, or stop a bounded browser network capture. Start before navigation; set include_http_bodies for native bounded response bodies. Inspect status or tail -f the returned private NDJSON path with jq/rg. Chromium WebSocket frames are protocol-native; Firefox WebSocket frames use disclosed page instrumentation; HTTP lifecycle and bodies are native on both. Chromium cannot return a body the page drained with response.blob(); such http_response_body records carry an error and no payload, so read text()/arrayBuffer() or leave the body unread when the bytes matter. Safari is unsupported."
    )]
    async fn browser_network(
        &self,
        Parameters(input): Parameters<NetworkInput>,
    ) -> Result<Json<NetworkOutput>, String> {
        let action = input.build_action()?;
        let receipt = self
            .controller
            .execute(&input.panel_id, action, input.timeout_millis)
            .await
            .map_err(|error| error.to_string())?;
        let BrowserControlValue::Network { capture } = receipt.value else {
            return Err("browser returned an unexpected network capture result".to_string());
        };
        Ok(Json(NetworkOutput::new(input.panel_id, receipt.action_id, capture)))
    }

    #[tool(
        name = "browser_network_watch",
        description = "Wait up to 60 seconds for a bounded batch of matching records from the active Horizon-owned network capture. Resume with capture_id and next_sequence; filters run before records are returned, payloads are excluded by default, and no capture path is accepted or exposed."
    )]
    async fn browser_network_watch(
        &self,
        Parameters(input): Parameters<NetworkWatchInput>,
    ) -> Result<Json<NetworkWatchOutput>, String> {
        self.network_watch.watch(&self.controller, input).await.map(Json)
    }

    #[tool(
        name = "browser_http_auth",
        description = "Set or clear HTTP Basic and Digest credentials for a live local Chromium or Firefox panel. This is the only way to supply a username and password for HTTP authentication: MCP clients, CLI run plans, and prompt jobs all call this tool. If you encounter HTTP authentication, call set with the username and password the user provided. Pass origin (http://host[:port] or https://host[:port]) when known; if omitted, the current page origin is used, and set fails when the page has none. Credentials are provided only to matching server Basic or Digest challenges for that origin, never to another origin, a proxy, or a retry of the same request. Passwords never enter the action audit or Horizon config. Clear drops live-session credentials for later intercepted challenges; it does not revoke Authorization values the browser already cached, so open a new panel for a clean unauthenticated session. Safari and remote device sessions return unsupported_backend."
    )]
    async fn browser_http_auth(
        &self,
        Parameters(input): Parameters<HttpAuthInput>,
    ) -> Result<Json<HttpAuthOutput>, String> {
        let action = input.build_action()?;
        let receipt = self
            .controller
            .execute(&input.panel_id, action, input.timeout_millis)
            .await
            .map_err(|error| error.to_string())?;
        if !matches!(receipt.value, BrowserControlValue::Accepted) {
            return Err("browser returned an unexpected HTTP auth result".to_string());
        }
        Ok(Json(input.output(receipt.action_id)))
    }

    #[tool(
        name = "browser_video",
        description = "Start, pause, resume, inspect, or stop a bounded WebM recording of the panel's page pixels. Start-only options: quality (1-100), compression_level (0-10), fps (1-30), max_width (320-1920, caps the longest encoded side), max_file_bytes (4096-1073741824). Omitted options keep host browser.video settings; host defaults are quality 90 and source-frame sizing with codec alignment, bounded by a 3840-pixel longest side and 8294400 pixels; larger frames are downscaled proportionally. Pause skips time in the file; stop finalizes a private .webm path. The host writes a frame only when the page pixels changed, so a static page keeps one frame and reports a low effective_fps. quality grades sample coverage as unknown, good, degraded or poor; degraded and poor mean the host fell behind and a brief page state can be absent, so report it. Works on Chromium, Firefox, and Safari because it samples the existing decoded frame slot. Page pixels never enter audit."
    )]
    async fn browser_video(&self, Parameters(input): Parameters<VideoInput>) -> Result<Json<VideoOutput>, String> {
        let action = input.build_action()?;
        let receipt = self
            .controller
            .execute(&input.panel_id, action, input.timeout_millis)
            .await
            .map_err(|error| error.to_string())?;
        let BrowserControlValue::Video { capture } = receipt.value else {
            return Err("browser returned an unexpected video capture result".to_string());
        };
        Ok(Json(VideoOutput::new(input.panel_id, receipt.action_id, capture)))
    }

    #[tool(
        name = "browser_downloads",
        description = "Read the panel's download folder and the downloads this session observed, newest last: each row reports the file name, state (in_progress, completed or canceled), received bytes, total bytes when known, and the absolute path once the browser named the file it wrote. A local Chromium or Firefox panel saves every download of its page into that one panel-owned folder, including a download a person starts, so a page-initiated download is no longer silent: read this list instead of guessing where a file went. reporting=false means this exact session could not observe downloads, so an empty list is not proof that nothing was saved. Rows are bounded; dropped counts reports this session forgot, and their files stay in the folder. Read the file at its path with ordinary read-only tools, or name one completed row in fetch to carry its bytes in the answer (at most 8 MiB, as an application/octet-stream resource block, when the file lives on a host of another computer); this tool starts no download, writes no file and returns no download URL, because a download URL can carry a credential. Safari and remote sessions return unsupported_backend."
    )]
    async fn browser_downloads(
        &self,
        Parameters(input): Parameters<DownloadsInput>,
    ) -> Result<Json<DownloadsOutput>, String> {
        let receipt = self
            .controller
            .execute(&input.panel_id, BrowserControlAction::Downloads, input.timeout_millis)
            .await
            .map_err(|error| error.to_string())?;
        let BrowserControlValue::Downloads { downloads } = receipt.value else {
            return Err("browser returned an unexpected download list".to_string());
        };
        let output = DownloadsOutput::new(input.panel_id, receipt.action_id, downloads, input.fetch.as_deref());
        Ok(Json(output))
    }

    #[tool(
        name = "browser_wait",
        description = "Wait for a CSS selector to become present, visible, or hidden as one audited engine-side action: the browser driver observes the page itself (no repeated queries) and returns the matched nodes and elapsed_millis, or a typed failure (wait_timeout, wait_navigation_invalidated when the page navigated, wait_ownership_lost, wait_handoff_pending, wait_superseded when a later wait on the same panel replaced it, browser_unavailable when the browser backend stops). timeout_millis is 1000-60000 (default 10000) and is enforced in full by the engine; poll_millis is accepted for compatibility and ignored."
    )]
    async fn browser_wait(&self, Parameters(input): Parameters<WaitInput>) -> Result<Json<WaitOutput>, String> {
        wait_for_selector(&self.controller, input).await.map(Json)
    }

    #[tool(
        name = "browser_handoff",
        description = "Requires the advertised handoff capability. Remote sessions return unsupported_backend without starting a handoff or wait; manual remote steering is unsupported. For supported local sessions, pause page actions and ask the user to steer in a progress message, then keep this turn active awaiting this tool until they select Done — hand back to agent. Omit timeout_millis for the default 15-minute human wait; do not substitute a 60000 ms page-action timeout. If the client yields a running invocation, keep awaiting that same invocation. Never end the turn with a final waiting message. On timeout, inspect browser_panel once: if handoff_pending is true, call browser_handoff again with resume_request_id from the timeout and the default timeout; if handoff completed, take a fresh snapshot and continue. Also take a fresh snapshot when this tool returns handoff_pending=false. Stop on explicit cancellation, panel closure, lost ownership, or an unrecoverable connection failure. Do not poll browser_list. timeout_millis is 1000-3600000 (default 900000); explicit short waits remain available for scripts/tests. Set wait=false only for an explicitly nonblocking script."
    )]
    async fn browser_handoff(
        &self,
        Parameters(input): Parameters<HandoffInput>,
    ) -> Result<Json<HandoffOutput>, String> {
        let receipt = self
            .controller
            .request_handoff(
                &input.panel_id,
                &input.reason,
                input.wait.unwrap_or(true),
                input.timeout_millis,
                input.resume_request_id.as_deref(),
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(Json(HandoffOutput {
            panel_id: receipt.panel_id,
            request_id: receipt.request_id,
            handoff_pending: receipt.handoff_pending,
            elapsed_millis: receipt.elapsed_millis,
        }))
    }

    #[tool(
        name = "browser_audit",
        description = "Read a bounded page of redacted audit entries for a browser panel. The default page is the newest matching records (limit 1-500, default 100). Set from_start=true to iterate from the oldest retained match; reuse next_event_id as after_event_id until has_more is false. Optional action_id filter. The response includes pagination and loss metadata: records_returned, records_retained, malformed_records, older_records_dropped, and cursor_lost when after_event_id is no longer retained."
    )]
    fn browser_audit(&self, Parameters(input): Parameters<AuditInput>) -> Result<Json<AuditOutput>, String> {
        let page = self
            .controller
            .read_audit_page(
                &input.panel_id,
                &AuditPageRequest::new(
                    input.after_event_id,
                    input.from_start.unwrap_or(false),
                    input.action_id,
                    input.limit,
                ),
            )
            .map_err(|error| error.to_string())?;
        let entries = page
            .entries
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("could not encode browser audit: {error}"))?;
        Ok(Json(AuditOutput {
            panel_id: input.panel_id,
            entries,
            next_event_id: page.next_event_id,
            has_more: page.has_more,
            records_returned: page.records_returned,
            records_retained: page.records_retained,
            malformed_records: page.malformed_records,
            older_records_dropped: page.older_records_dropped,
            cursor_lost: page.cursor_lost,
        }))
    }

    #[tool(
        name = "browser_panel",
        description = "Read one live Horizon browser panel's safe state, including the profile that it uses: the shared profile name, or `private`. For a Horizon-injected agent identity, panel ids outside the calling agent's workspace are rejected. Use browser_list when the panel id is unknown."
    )]
    fn browser_panel(
        &self,
        Parameters(input): Parameters<PanelInput>,
    ) -> Result<Json<crate::model::BrowserPanel>, String> {
        self.controller
            .panel(&input.panel_id)
            .map(Json)
            .map_err(|error| error.to_string())
    }
}

impl ServerHandler for HorizonBrowserMcp {
    fn get_info(&self) -> ServerConfig {
        let info = ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("horizon-browser", env!("CARGO_PKG_VERSION")));
        if self.controller.cards_only() {
            return info.with_instructions(CARDS_INSTRUCTIONS);
        }
        info.with_instructions(
                "MCP is the sole agent control contract for Horizon browser panels. For agent identities injected by Horizon, discovery and control are scoped to the workspace that contains the calling agent panel: browser_list shows only that workspace's panels, every other tool rejects panel ids outside it, and a panel's visible field is host presentation state rather than proof that it is in your workspace; identities from outside Horizon keep unscoped discovery. Start with browser_list; if it is empty, call browser_create in your current Horizon workspace, and to run a check in another engine beside your panel, call browser_create with that engine as backend. Reuse an existing panel for iframe, popup, dialog, and consent interactions: never create or reveal a helper panel as a workaround. If the current top-level semantic tools cannot reach embedded frame content, call browser_handoff on the original panel only when its capabilities include handoff. Remote sessions do not support manual steering and return unsupported_backend; report that limitation instead of requesting a handoff. Use the default human-wait timeout on supported local sessions. Keep the turn active and await any yielded invocation until Done — hand back to agent; a final waiting message cannot resume on hand-back. On handoff timeout, inspect browser_panel: renew the blocking wait with resume_request_id from the timeout and the default timeout while handoff_pending is true; otherwise resume from a fresh snapshot if handoff completed, or report closure, lost ownership, cancellation, or connection failure. To compare engines, browser_create with an explicit backend opens one panel per engine (chromium, firefox, safari) beside your existing panel. Only set browser_create allow_additional=true when the user explicitly requests an independent browser session, which a second panel of the same engine, a create without backend, or another remote target needs. Use browser_provider_devices with a provider name and search words to discover any available device/OS/browser combination; pass its returned target to browser_create. Configured target names also remain supported; never supply endpoints, capabilities or credentials yourself. Use visible=false for a live background panel and browser_visibility to show or hide it later without stopping automation or capture. Call browser_close when the user is done with a panel you own or a remote device session must be released; it stops the session and the panel leaves browser_list. Use browser_provider_usage to inspect shared usage for all configured provider profiles or one named provider, without allocating a session. Usage is informational and does not reserve capacity. If remote capacity remains held after a panel disappears, use browser_remote_allocations list and then reconcile its safe reference. Reconciliation checks only the exact retired allocation at the original provider; uncertain results retain the hold. Inspect each panel's advertised capabilities before requesting navigation, DOM actions, handoff, audit, network, or video capture. Use browser_resize width and height to pin a supported local content viewport in CSS pixels; reset=true resumes panel sizing. Check its measured applied dimensions. Navigation preserves the pin; video max_width does not change it. For page zoom checks (for example 200 percent reflow), call browser_zoom with percent 25-500 or reset=true on local Chromium or Firefox; its css_viewport is the panel or pinned size divided by the zoom, and Safari and remote sessions return unsupported_backend. If you encounter HTTP Basic or Digest authentication, call browser_http_auth set with the username and password the user provided (optional origin) before browser_navigate, or set then reload if a protected page was already opened; Safari and remote sessions return unsupported_backend. For WebSocket or HTTP observation, call browser_network start before browser_navigate; opt into native bounded response bodies with include_http_bodies. Prefer browser_network_watch with its returned capture_id and next_sequence for bounded event-driven monitoring, or tail the exact private NDJSON path returned by browser_network with ordinary read-only Unix tools; call browser_network stop to flush. For page-pixel recording, call browser_video start, optionally pause/resume, then stop to finalize a private WebM path; quality, compression_level, fps, max_width, and max_file_bytes are start-only. To upload a file, call browser_act set_files on the input[type=file] element itself (browser_query finds it even when a styled button hides it) with absolute host paths under your work root; the result reports the attached names and sizes, BrowserStack desktop and Android sessions transfer them before selection; iOS uploads remain unsupported. For a local Chromium or Firefox drop zone, browser_act drop_files uses the same authorized files and staging, with the drop-zone ref or selector. Remote sessions and Safari do not support drop_files. For keyboard checks, browser_act press sends trusted key presses (Tab, Shift+Tab, Enter, Escape, arrows, or a chord such as Control+a, with an optional count) to the focused element; then read document.activeElement with browser_evaluate or take a screenshot. Take a fresh semantic snapshot or query before acting through refs, and verify afterward. Never use raw browser endpoints or Horizon's private runtime files.",
            )
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let screenshot = self.inline_screenshots && request.name == "browser_screenshot";
        let fetch = requested_download(&request);
        let response = self
            .tool_router
            .call(ToolCallContext::new(self, request, context))
            .await?;
        if let Some(file_name) = fetch {
            return Ok(crate::download::attach(response, &file_name));
        }
        if screenshot {
            let root = horizon_browser_control::BrowserRuntimePaths::resolve();
            return Ok(crate::screenshot::inline(response, root.root()));
        }
        Ok(response)
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> {
        std::future::ready(Ok(ListToolsResult {
            tools: self.tool_router.list_all(),
            ttl_ms: Some(0),
            cache_scope: Some(rmcp::model::CacheScope::Private),
            ..Default::default()
        }))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }
}

/// Shortest navigation bound this server accepts: the engine reports its
/// typed `timed_out` outcome on a 250 ms coordination tick, so smaller
/// bounds could only end in this server's generic timeout.
const MIN_NAVIGATION_TIMEOUT_MILLIS: u64 = 1_000;

/// The bound the engine enforces for a navigation action; the controller
/// waits `RESULT_DELIVERY_HEADROOM_MILLIS` longer for the typed report.
/// The file name that a request asks the answer of `browser_downloads` to carry.
/// The handler states the name it resolved in its own result; this rewrite adds
/// the bytes of that one file.
fn requested_download(request: &rmcp::model::CallToolRequestParams) -> Option<String> {
    if request.name != "browser_downloads" {
        return None;
    }
    request
        .arguments
        .as_ref()?
        .get("fetch")?
        .as_str()
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

fn bounded_navigation_timeout(timeout_millis: Option<u64>) -> u64 {
    crate::controller::bounded_action_timeout(timeout_millis).max(MIN_NAVIGATION_TIMEOUT_MILLIS)
}

fn bounded_evaluation_timeout(timeout_millis: Option<u64>) -> u64 {
    timeout_millis
        .unwrap_or(BrowserControlAction::DEFAULT_EVALUATION_TIMEOUT_MILLIS)
        .clamp(1, BrowserControlAction::MAX_EVALUATION_TIMEOUT_MILLIS)
}

fn require_action_completed(action: ActKind, value: &BrowserControlValue) -> Result<(), String> {
    if matches!(value, BrowserControlValue::Accepted)
        || matches!(
            (action, value),
            (ActKind::Scroll, BrowserControlValue::Json { .. })
                | (ActKind::SetFiles, BrowserControlValue::Files { .. })
                | (ActKind::DropFiles, BrowserControlValue::Accepted)
        )
    {
        Ok(())
    } else {
        Err("browser returned an unexpected action result".to_string())
    }
}

/// Shortest wait bound this server accepts; the engine observes on a 100 ms
/// cadence and reports its typed timeout on a 250 ms coordination tick.
const MIN_WAIT_TIMEOUT_MILLIS: u64 = 1_000;

/// The wait this server applies to a `WaitForSelector` action.
fn bounded_wait_timeout(timeout_millis: Option<u64>) -> u64 {
    timeout_millis
        .unwrap_or(horizon_browser::DEFAULT_WAIT_TIMEOUT_MILLIS)
        .clamp(MIN_WAIT_TIMEOUT_MILLIS, MAX_ACTION_TIMEOUT_MILLIS)
}

/// One audited engine-side wait: the browser driver observes the selector
/// itself and reports the matched nodes, the elapsed time, or a typed
/// failure (`wait_timeout`, `wait_navigation_invalidated`,
/// `wait_ownership_lost`, `wait_handoff_pending`, `wait_superseded`,
/// `browser_unavailable`).
async fn wait_for_selector(controller: &BrowserController, input: WaitInput) -> Result<WaitOutput, String> {
    let timeout_millis = bounded_wait_timeout(input.timeout_millis);
    if let Some(poll_millis) = input.poll_millis {
        tracing::debug!(target: "browser_mcp", poll_millis, "browser_wait poll_millis is ignored; the engine observes the page itself");
    }
    let receipt = controller
        .execute_engine_bounded(
            &input.panel_id,
            BrowserControlAction::WaitForSelector {
                selector: input.selector,
                state: input.state.into(),
                timeout_millis: Some(timeout_millis),
            },
            timeout_millis,
        )
        .await
        .map_err(|error| error.to_string())?;
    let BrowserControlValue::Wait { wait } = receipt.value else {
        return Err("browser returned an unexpected wait result".to_string());
    };
    Ok(WaitOutput::from_outcome(input.panel_id, receipt.action_id, wait))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WaitState;

    #[test]
    fn public_device_input_rejects_internal_browser_commands() {
        use horizon_browser_control::manifest::device::{DevicePanelInput, Operation};
        let internal = serde_json::json!({"operation":"browser_screenshot", "panel_id":"panel"});
        assert!(serde_json::from_value::<DevicePanelInput>(internal.clone()).is_err());
        // The private host queue can still deserialize its own command.
        assert!(serde_json::from_value::<Operation>(internal).is_ok());
        let native = serde_json::json!({"operation":"screenshot", "panel_id":"panel", "copy_to_clipboard":true});
        assert!(matches!(
            serde_json::from_value::<DevicePanelInput>(native).unwrap().into_operation(),
            Operation::Screenshot { input } if input.copy_to_clipboard
        ));
    }

    #[test]
    fn evaluation_bounds_follow_the_protocol_contract() {
        assert_eq!(
            bounded_evaluation_timeout(None),
            BrowserControlAction::DEFAULT_EVALUATION_TIMEOUT_MILLIS
        );
        assert_eq!(bounded_evaluation_timeout(Some(0)), 1);
        assert_eq!(bounded_evaluation_timeout(Some(12_000)), 12_000);
        assert_eq!(
            bounded_evaluation_timeout(Some(u64::MAX)),
            BrowserControlAction::MAX_EVALUATION_TIMEOUT_MILLIS
        );
    }

    #[test]
    fn every_advertised_input_schema_is_a_plain_object() {
        // Model providers refuse a tool whose input schema has `oneOf`, `anyOf`
        // or `allOf` at the top level, and the client then drops the tool from
        // the agent's list. Every tool must describe one object.
        let server = HorizonBrowserMcp::new(BrowserController::with_actor("agent"));
        let mut tools = server.tool_router.list_all();
        tools.sort_by(|left, right| left.name.cmp(&right.name));
        assert!(!tools.is_empty(), "the server advertises tools");
        for tool in tools {
            let schema = serde_json::to_value(tool.input_schema.as_ref()).expect("an input schema");
            let keys = schema
                .as_object()
                .map(|object| object.keys().cloned().collect::<Vec<_>>());
            assert_eq!(
                schema["type"],
                serde_json::json!("object"),
                "{} must describe a JSON object, keys {keys:?}",
                tool.name
            );
            for combinator in ["oneOf", "anyOf", "allOf"] {
                assert!(
                    schema.get(combinator).is_none_or(serde_json::Value::is_null),
                    "{} has a top-level {combinator}, which providers refuse: keys {keys:?}",
                    tool.name
                );
            }
        }
    }

    #[test]
    fn block_panel_takes_the_operation_and_never_a_wrapper_field() {
        let input = serde_json::json!({"operation": "next_click", "panel_id": "card", "timeout_millis": 5000});
        let parsed = serde_json::from_value::<horizon_browser_control::manifest::block::BlockPanelInput>(input)
            .expect("a card operation is the tool input");
        assert!(matches!(
            parsed.into_operation(),
            horizon_browser_control::manifest::block::Operation::NextClick {
                timeout_millis: Some(5000),
                ..
            }
        ));
        assert!(
            serde_json::from_value::<horizon_browser_control::manifest::block::BlockPanelInput>(
                serde_json::json!({"operation": "inspect", "panel_id": "card", "wrapped": {"operation": "list"}})
            )
            .is_err(),
            "an unknown field beside an operation is still refused"
        );
    }

    #[test]
    fn the_block_panel_schema_advertises_the_terminal_operation() {
        let server = HorizonBrowserMcp::new(BrowserController::with_actor("agent"));
        let tool = server
            .tool_router
            .list_all()
            .into_iter()
            .find(|tool| tool.name == "block_panel")
            .expect("the block_panel tool");
        let schema = serde_json::to_string(tool.input_schema.as_ref()).expect("an input schema");
        for field in [
            "open_terminal",
            "remote_command",
            "ssh",
            "machine",
            "machine_set",
            "machine_delete",
            "machines",
            "machine_key",
            "machine_keys",
            "delete_machine_key",
            "key",
        ] {
            assert!(schema.contains(field), "the schema omits {field}: {schema}");
        }
        for code in [
            "invalid_key_name",
            "too_many_keys",
            "key_unavailable",
            "invalid_machine_name",
            "invalid_session_name",
            "machine_unavailable",
            "too_many_machines",
            "invalid_request",
        ] {
            assert!(
                tool.description
                    .as_deref()
                    .is_some_and(|description| description.contains(code)),
                "the description names {code}"
            );
        }
        assert!(
            tool.description
                .as_deref()
                .is_some_and(|description| description.contains("open_terminal")),
            "the description names the operation"
        );
    }

    #[test]
    fn the_block_panel_schema_advertises_the_panel_of_a_preset() {
        let server = HorizonBrowserMcp::new(BrowserController::with_actor("agent"));
        let tool = server
            .tool_router
            .list_all()
            .into_iter()
            .find(|tool| tool.name == "block_panel")
            .expect("the block_panel tool");
        let schema = serde_json::to_string(tool.input_schema.as_ref()).expect("an input schema");
        for field in ["open_panel", "preset", "directory"] {
            assert!(schema.contains(field), "the schema omits {field}: {schema}");
        }
        let description = tool.description.as_deref().expect("a description");
        for code in [
            "unknown_preset",
            "directory_missing",
            "rate_limited",
            "panel_unavailable",
        ] {
            assert!(description.contains(code), "the description names {code}");
        }
        assert!(
            description.contains("open_panel(preset,directory,title?)"),
            "the description names the fields of the operation"
        );
    }

    #[test]
    fn server_exposes_only_the_mcp_contract() {
        let server = HorizonBrowserMcp::new(BrowserController::with_actor("agent"));
        let mut names = server
            .tool_router
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            [
                "block_panel",
                "browser_act",
                "browser_audit",
                "browser_close",
                "browser_create",
                "browser_downloads",
                "browser_duplicate",
                "browser_evaluate",
                "browser_handoff",
                "browser_http_auth",
                "browser_list",
                "browser_navigate",
                "browser_network",
                "browser_network_watch",
                "browser_orientation",
                "browser_panel",
                "browser_provider_devices",
                "browser_provider_usage",
                "browser_query",
                "browser_remote_allocations",
                "browser_resize",
                "browser_screenshot",
                "browser_snapshot",
                "browser_video",
                "browser_visibility",
                "browser_wait",
                "browser_zoom",
                "device_panel",
            ]
        );
        let schemas = serde_json::to_string(&server.tool_router.list_all()).unwrap_or_default();
        assert!(schemas.contains("browser_unavailable"));
        assert!(schemas.contains("after_event_id"));
        assert!(schemas.contains("from_start"));
        assert!(schemas.contains("next_event_id"));
        assert!(schemas.contains("older_records_dropped"));
        assert!(schemas.contains("cursor_lost"));
        assert!(schemas.contains("Done — hand back to agent"));
        assert!(schemas.contains("wait"));
        assert!(!schemas.contains("browser_ws"));
        assert!(!schemas.contains("manifest_path"));
        assert!(!schemas.contains("cdp_endpoint"));
    }

    #[test]
    fn navigation_outcomes_map_to_typed_tool_results() {
        let outcome = |state| horizon_browser::NavigationOutcome {
            requested_url: "https://example.test/".to_string(),
            wait: horizon_browser::NavigationWait::Commit,
            state,
            committed_url: Some("https://example.test/landing".to_string()),
            title: None,
            loading: true,
            redirected: true,
            elapsed_millis: 12,
        };
        let committed = NavigateOutput::from_outcome(
            "panel".to_string(),
            "action".to_string(),
            outcome(horizon_browser::NavigationState::Committed),
        );
        assert!(committed.completed);
        assert_eq!(committed.state, crate::model::NavigateState::Committed);
        assert_eq!(committed.wait, crate::model::NavigateWait::Commit);
        assert!(committed.redirected);
        assert_eq!(committed.committed_url.as_deref(), Some("https://example.test/landing"));
        for state in [
            horizon_browser::NavigationState::TimedOut,
            horizon_browser::NavigationState::Superseded,
        ] {
            let output = NavigateOutput::from_outcome("panel".to_string(), "action".to_string(), outcome(state));
            assert!(!output.completed, "{state:?} never counts as completed");
            assert_eq!(output.elapsed_millis, 12);
        }
        let dispatched = NavigateOutput::from_outcome(
            "panel".to_string(),
            "action".to_string(),
            outcome(horizon_browser::NavigationState::Dispatched),
        );
        assert!(dispatched.completed);

        assert_eq!(bounded_navigation_timeout(None), 15_000);
        assert_eq!(
            bounded_navigation_timeout(Some(100)),
            1_000,
            "bounds below 1 s are raised"
        );
        assert_eq!(
            bounded_navigation_timeout(Some(600_000)),
            60_000,
            "the engine gets the full documented bound; the controller adds delivery headroom"
        );
        let encoded = serde_json::to_value(crate::model::NavigateWait::DomContentLoaded).expect("encode");
        assert_eq!(encoded, serde_json::json!("dom_content_loaded"));
        let aliased: crate::model::NavigateWait =
            serde_json::from_value(serde_json::json!("domcontentloaded")).expect("alias decodes");
        assert_eq!(aliased, crate::model::NavigateWait::DomContentLoaded);
    }

    #[test]
    fn wait_inputs_map_to_one_bounded_engine_action() {
        assert_eq!(bounded_wait_timeout(None), 10_000);
        assert_eq!(bounded_wait_timeout(Some(10)), 1_000, "bounds below 1 s are raised");
        assert_eq!(bounded_wait_timeout(Some(600_000)), 60_000);
        let state: horizon_browser::SelectorState = WaitState::Hidden.into();
        assert_eq!(state, horizon_browser::SelectorState::Hidden);
        let output = WaitOutput::from_outcome(
            "panel".to_string(),
            "action".to_string(),
            horizon_browser::WaitOutcome {
                state: horizon_browser::SelectorState::Visible,
                generation: 2,
                revision: 3,
                nodes: Vec::new(),
                elapsed_millis: 1_480,
                polls: 15,
            },
        );
        assert_eq!(output.state, "visible");
        assert_eq!(output.elapsed_millis, 1_480);
        assert_eq!(output.polls, 15);
        assert_eq!((output.generation, output.revision), (2, 3));
    }

    #[test]
    fn scroll_accepts_the_engines_scroll_state_completion() {
        assert!(require_action_completed(ActKind::Click, &BrowserControlValue::Accepted).is_ok());
        assert!(
            require_action_completed(
                ActKind::Scroll,
                &BrowserControlValue::Json {
                    value: serde_json::json!({ "scrollY": 480 }),
                },
            )
            .is_ok()
        );
        assert!(
            require_action_completed(
                ActKind::Click,
                &BrowserControlValue::Json {
                    value: serde_json::Value::Null,
                },
            )
            .is_err()
        );
    }
}
