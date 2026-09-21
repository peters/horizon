# horizon-browser-mcp

`horizon-browser-mcp` is the sole agent-facing control adapter for live Horizon
browser panels. It serves newline-delimited MCP JSON-RPC over stdio and exposes
the same semantic actions for Chromium CDP, Firefox WebDriver BiDi, and Safari
WebDriver panels.

The crate is not published. Its executable can be configured as a local stdio
MCP server during development:

```toml
[mcp_servers.horizon-browser]
command = "/path/to/horizon-browser-mcp"
env_vars = ["HORIZON_BROWSER_ACTOR", "HORIZON_BROWSER_HOST_INSTANCE"]
default_tools_approval_mode = "approve"
```

The workspace's [`horizon-browser-cli`](../horizon-browser-cli) crate provides a
second launcher for this exact server (`horizon-browser mcp`) plus a JSON
plan runner that calls these MCP tools without defining another action API.

Released Horizon builds start the same server through private transport
bootstrap plumbing and apply that server-scoped approval mode automatically,
so bundled integrations neither need a second installed binary nor interrupt
each browser action with an approval prompt. This does not change approvals for
shell commands, files, or other MCP servers.

## Tool contract

- `browser_list` and `browser_panel` discover safe live-panel state. For an
  agent launched inside Horizon, discovery and every control tool are scoped
  to the workspace that contains the agent panel: panels in other workspaces
  are never listed and their ids are rejected even when they are unowned.
  A panel's `visible` field is host presentation state, not proof that it is
  in the caller's workspace.
- `browser_create` opens a panel in the calling agent's Horizon workspace and
  returns only after its backend is ready, it is owned by that agent, and,
  when a `url` was given, that page has committed (`navigation: committed`,
  with `panel.url` authoritative; the title is read after commit and may still
  be empty or change). If the first page has not
  committed within a bounded startup wait, the controllable panel is still
  returned with `navigation: pending` instead of an empty URL presented as
  ready; if that page failed to load, it is returned with `navigation: failed`
  and the browser's `navigation_error` (an explicit `about:blank` counts as
  committed); if the user navigated the panel before that page committed, it
  is returned with `navigation: superseded` at the user's page. `startup_millis` reports the latency from the host accepting the
  request until the panel was reported. It uses the configured
  backend unless explicitly overridden. Set `visible: false` to start a live
  background panel. If the same agent already owns a panel, creation is rejected
  unless the user explicitly requested another independent session and the call
  sets `allow_additional: true`. Reuse the original panel for iframe, popup,
  dialog, and consent flows instead of creating a helper panel.
- `browser_create` with `target: <name>` runs the session at a configured
  remote target (`browser.remote.targets` in Horizon's configuration) instead
  of a local browser. The agent names the target only; Horizon resolves the
  provider, capabilities and credentials and refuses with a typed reason
  (`target_unknown`, `target_invalid`, `credentials_not_ready`,
  `credentials_invalid`, `remote_session_limit_reached`, the last also when
  other Horizon instances on the same computer hold the provider's sessions,
  and `remote_quota_contended` when another instance was checking that quota
  at the same moment, which is simply retried) that never carries a
  value. After allocation the device is verified against the target's
  requirement from the provider's own evidence (a hosted grid's session
  record, or the capabilities a standard endpoint echoes); a physical
  requirement the evidence does not confirm fails as
  `remote_device_rejected` after Horizon attempts to release the session
  (the panel note says whether the provider confirmed it); a session that
  could not be allocated or safely started fails as
  `remote_allocation_failed` (nothing held; a rejected credential is
  `remote_authentication_failed`, an account without automation access is
  `remote_not_entitled`, and no matching or free device is
  `remote_device_unavailable`, all with nothing held), and one whose allocation or
  cleanup got no trustworthy answer fails as `remote_allocation_unknown` (a
  device may still be held; check the provider before creating again). Such a panel reports `remote_target`,
  `remote_device` (model, OS version, hardware evidence), classic
  `WebDriver` and no network capture.
- `browser_visibility` shows or hides an existing panel without stopping its
  browser, ownership lease, network capture, or MCP control.
- `browser_duplicate` opens the current URL in another panel sharing the source
  panel's cookies and persistent website storage. It supports ready local Chromium
  and Firefox panels in the caller's workspace. Each panel keeps independent
  navigation, input, and automation; logout affects its siblings. Closing a sibling preserves the
  remaining pages, and saved panels restore their shared profile. Unsaved forms,
  history, and live JavaScript state are not copied.
- `browser_close` closes a panel the caller owns in its workspace and stops
  its session; a remote device allocation is released by that teardown. The
  panel leaves `browser_list`; read what you need from `browser_audit` before
  closing, because the journal file is retained on disk but `browser_audit`
  answers only for a live panel.
- `browser_navigate` changes the top-level page and reports a typed outcome.
  By default it returns once the document committed (`wait: commit`);
  `wait: dispatched` returns as soon as the engine handed the command to the
  backend without awaiting the browser's acceptance (a rejection after that
  point shows as a failed page state, not as an action error), and
  `wait: dom_content_loaded` after `DOMContentLoaded`. The result carries
  `requested_url`, `committed_url`, `title`, `loading`, `redirected`,
  `elapsed_millis`, and `state`. Check `completed`: a wait that exceeds
  `timeout_millis` returns `state: timed_out` with the latest page state so the
  caller can inspect or retry, while unreachable destinations and rejected
  commands are errors audited as failed. `timeout_millis` accepts 1000-60000
  ms; smaller values are raised to 1000 so the typed outcome can be reported.
  Safari's classic WebDriver has no dispatch-only navigation, so there every
  wait returns once the page loaded or the bound elapsed.
- `browser_snapshot` and `browser_query` return bounded semantic nodes with
  short-lived refs. Snapshots keep iframe boundaries discoverable even when
  cross-origin policy prevents inspecting the frame document.
- `browser_act` clicks, fills, scrolls, reloads, or traverses history. Set
  `count: 2` on a click for a backend-native trusted double-click.
  Remote `fill` uses native Element Clear/Send Keys and reads the live field
  back before reporting completion. An empty, reverted, missing, or otherwise
  unequal value returns `input_failed`, even when the driver accepted the
  commands. Clearing an input is verified too. This is an immediate value
  postcondition, not a promise against later application changes or proof of
  native-picker interaction. No JavaScript value-assignment fallback is used
  for remote fill. Native date/time controls may reject a fill; inspect the
  resulting field after a failure because clearing may already have occurred.
  `set_files` attaches host files to an `input[type=file]`: target the input
  itself (query it by selector when a styled button hides it) and pass
  `files` as absolute paths, at most 32. Each path must resolve to a regular
  file under the agent work root (`HORIZON_WORK_ROOT`, else the server's
  working directory) or a root listed in `HORIZON_BROWSER_ATTACHMENT_ROOTS`;
  symlinks are judged by where they resolve, and the queue stages a private
  copy of each checked file (at most 512 MiB each) for the browser to read,
  so a pathname re-pointed after the check cannot reach the browser. The
  page reads an attached file lazily, often only on submit, so the copies
  stay for the panel: each panel keeps its attachment actions for 24 hours,
  at most 32 actions and 4 GiB, pruned when the next attachment is staged
  and swept when an MCP server starts, every hour while it runs, when it
  lists panels or closes one, and at every attachment, which also drops all
  staging of panels whose manifest is gone however they closed;
  staging for actions whose result has not been consumed yet (queued,
  dispatched or in flight) is never pruned for room, and when those alone
  leave no room the new request is refused (would block) until they settle. One request whose files alone exceed that 4 GiB
  is refused before any copy is made. Path validation, authorization and
  staging run on a blocking thread and count against the call's
  `timeout_millis` while the ownership lease is kept alive; a call that
  times out while staging reports so, and the staging may still finish and
  queue the action afterwards. `accept` extension tokens match the end of the file name,
  so compound tokens such as `.tar.gz` and dotfiles such as `.env` work. Copies live under the runtime root, or on
  Linux under `~/Horizon/browser-attachments/<digest of the runtime root>` when the runtime root is a
  hidden directory beneath the home directory, which a Snap-confined browser
  cannot open; that visible directory is namespaced by the runtime root so
  two hidden roots under one home never share it. The readback opens every attached file's first and last byte
  in the page, so a browser that lists a file it cannot read fails with
  `attachment_unreadable` instead of uploading nothing. An attachment
  replaces the input's current selection on every backend. The input's
  `multiple` and
  `accept` attributes are checked first (`multiple_not_allowed`,
  `accept_mismatch`; an `accept` attribute longer than 8192 characters is
  refused as `accept_unsupported` rather than judged in part), a target
  that is not a file input returns
  `not_file_input`, a path outside the roots returns `attachment_policy`, a
  malformed path `invalid_input`, a missing path `invalid_input` as well,
  and a file over the limit `file_too_large`, all with no side effects. The
  request's shape (count, absolute, UTF-8, length, control characters) is
  checked before any backend or panel state, so it fails the same way on
  every backend; an unsupported remote platform is then refused with `unsupported_backend`
  before the paths are opened, so a missing path on an unsupported remote panel reports
  `unsupported_backend`.
  BrowserStack desktop sessions transfer a single-file ZIP through the Selenium
  upload endpoint before selection; BrowserStack Android sessions use Appium Push File.
  Each remote request is limited to 16 MiB per file and 32 MiB total.
  Generic WebDriver grids and unsupported platforms (including iOS native pickers) omit `set_files` from
  capabilities and return `unsupported_backend`. Transfer failures return
  `attachment_transfer_failed` without including provider responses or file
  contents. Transferred files remain in the provider session for lazy page
  reads and are released with that session. `accept`
  tokens that are neither an extension nor a MIME type are ignored, as
  browsers ignore them. Chromium attaches through `DOM.setFileInputFiles`, which
  fires the page's `input` and `change` handlers; local Firefox and Safari
  use Element Send Keys with the paths. The result's `files` lists the
  names, sizes, and MIME types read back from the input, and a readback that
  does not hold exactly the requested files by name and size fails with
  `attachment_mismatch`.
  Every audit record for the action keeps the authorized source
  paths, never the private staged copies or the contents. Snapshot
  and query nodes carry `file_input` (`accept`, `accept_truncated`, `multiple`, `files`) for
  file inputs so the attachment can be verified without `browser_evaluate`.
- `browser_http_auth` is how a user supplies a username and password for HTTP
  Basic or Digest (MCP, CLI `run` plans, and prompt jobs all call this tool).
  Call `operation: set` with the credentials the user provided. Pass `origin`
  as `http://host[:port]` or `https://host[:port]` when known; if omitted, the
  current page origin is used, and set fails when the page has none. The engine
  provides those credentials only to matching server challenges for that origin. Passwords never enter the action audit or
  Horizon config. Safari and remote device sessions return
  `unsupported_backend`. `operation: clear` drops live-session credentials for
  later intercepted challenges; it does not revoke Authorization values the
  browser already cached, so open a new panel for a clean unauthenticated
  session.
- `browser_wait` verifies present, visible, or hidden selector state as one
  audited engine-side action: the browser driver observes the page itself at
  a fixed cadence (no repeated query actions), evaluates the condition over
  every element the selector matches (the page scan counts matches and
  visible matches beyond the nodes it returns) and returns at most 20 of
  them with `elapsed_millis` and `polls`, and fails with a typed code when the bound
  elapses (`wait_timeout`), the page navigates (`wait_navigation_invalidated`),
  the lease is lost (`wait_ownership_lost`), or a handoff is pending
  (`wait_handoff_pending`), or a later wait on the same panel replaces it
  (`wait_superseded`), or the browser backend stops while waiting
  (`browser_unavailable`). `timeout_millis` accepts 1000-60000 ms;
  `poll_millis` is accepted for compatibility and ignored.
- `browser_evaluate` evaluates an explicit size-bounded expression.
- `browser_network` starts, inspects, or stops a bounded HTTP/WebSocket
  capture and returns an explicit private NDJSON path plus connection state,
  frame/byte counts, drops, truncation, and writer health. Start it before
  navigation for a complete request or socket lifecycle. Set
  `include_http_bodies: true` together with `include_http: true` to add native,
  size-bounded `http_response_body` records. While active, agents may use
  ordinary read-only tools such as `tail -f`, `jq`, or `rg` on that exact path.
- `browser_network_watch` long-polls that Horizon-owned capture for a bounded,
  filtered batch. Reuse its `capture_id` and `next_sequence` to avoid duplicate
  delivery. It accepts no file path, excludes payloads by default, reports
  sequence gaps and capture health explicitly, and wakes on a matching record,
  capture stop, capture replacement, or timeout.
- `browser_handoff` pauses automation so the user can steer, and waits until
  they hand the panel back (`handoff_pending: false`) unless `wait` is false.
  Remote sessions do not advertise handoff and return `unsupported_backend`
  before claiming ownership, creating/resuming a handoff, or starting a wait.
  Manual remote steering is not supported; showing the panel is not proof of
  usable native input. This applies to MCP and CLI plans using the same tool.
  Keep the agent turn active, including while its client yields a running tool.
  Omit `timeout_millis` for the default 15-minute human wait. After a timeout,
  inspect `browser_panel`; while still pending, retry with the
  timeout's `resume_request_id` and the default timeout. This waits on the
  existing request without undoing a concurrent Done click. A stale id is
  rejected, and `resume_request_id` requires `wait: true` (the default).
  If handoff completed, take a fresh snapshot and continue; stop on cancellation,
  closure, lost ownership, or an unrecoverable connection failure.
- `browser_audit` returns a bounded page of redacted ordered action records.
  The default page is the newest matching entries (`limit` 1-500, default 100).
  Set `from_start` to iterate from the oldest retained match, then reuse
  `next_event_id` as `after_event_id` until `has_more` is false. The response
  reports `records_returned`, `records_retained`, `malformed_records`,
  `older_records_dropped`, and `cursor_lost` when the resume cursor is no
  longer retained.

The server automatically claims and heartbeats a panel using
`HORIZON_BROWSER_ACTOR`. Horizon injects that identity, together with the
`HORIZON_BROWSER_HOST_INSTANCE` of the host process that launched the agent,
into the agent process and explicitly forwards both to the bundled stdio MCP
subprocess. A Horizon identity that reaches the server without its host
instance receives an explicit error instead of an empty workspace. Creation and
visibility requests are accepted only from an identity belonging to a live
Horizon agent panel and are routed to that Horizon instance. Panel lifecycle
and later actions share the redacted audit identity.
The Horizon host stamps every live browser manifest with its own host
instance and the agent identities that currently share the panel's workspace,
and refreshes the stamp when either panel moves; the driver records its host
at start and only that host may stamp the manifest, so a second host whose
board carries the same persisted panel id cannot rewrite it. The stamp and
the requested owner of a newly created panel are written in one locked
transaction, so authorization follows
workspace membership without restarting the browser session or the agent.
Membership is evaluated inside the same locked manifest transaction as each
claim, heartbeat, queued action, handoff, and audit read, so a move cannot
race past the boundary. Because the stamp names the host and every agent
carries the host that launched it, separate Horizon processes sharing one
home never authorize each other's panels, even when duplicated or copied
sessions reuse the same persisted panel ids or their workspace ids collide;
create and visibility requests are likewise claimed only by the launching
host. Manifests without a stamp (older hosts, or a panel whose host has not
stamped it yet) fail closed for Horizon agents.
Identities from outside Horizon (a standalone host or the process-local
fallback) are not placed in any workspace and keep unscoped discovery.
When no valid actor is injected, the server uses a process-local identity and
releases only that identity's claims on clean shutdown. A crash retains the
heartbeat TTL fallback, while a normal reconnect can claim the panel
immediately.
When embedded frame content cannot be controlled by the current top-level
semantic tools, the supported fallback is a user handoff on the original panel,
not a separate helper panel.
Tool schemas and results never expose raw CDP, BiDi, WebDriver, manifest,
audit-file, or result-file locations. Horizon's locked manifest queue remains
private implementation plumbing, not a second API.

Chromium HTTP metadata, response bodies, and WebSocket frames come directly
from CDP. CDP cannot return a `fetch()` body that the page drained straight into
a `Blob` (`response.blob()`); Horizon records that body as an
`http_response_body` record with an `error` and no `payload` instead of a
zero-byte success, so read `text()` or `arrayBuffer()`, or leave the body
unread, when the bytes matter. Separately, a top-level navigation to a PDF
captures the PDF viewer's HTML shell as a normal successful body, not the PDF
bytes. Firefox HTTP metadata and response bodies use native WebDriver BiDi;
standard BiDi does not currently expose WebSocket frames, so Firefox uses an
explicitly advertised, pre-document page bridge that batches only socket
frames before crossing BiDi. This bridge is observable by page code and is not
an undetectability feature. Safari network capture is reported as unsupported.
Response bodies are opt-in because they may contain sensitive page data; the
export is private, bounded, filtered, and excluded from the action audit.

## Dependency boundary

`horizon-browser-mcp` uses `horizon-browser-control` for filesystem discovery,
workspace authorization, ownership, handoff, action queues, results, and audit.
It has no dependency on Horizon core, terminal state, or UI. The same stdio
server can control compatible standalone browser hosts and embedded Horizon
panels, with the existing actor/host scoping rules preserved.

The browser engine remains in `horizon-browser`; it does not depend on this
server or an async runtime. The MCP adapter owns Tokio and protocol transport.
The default coordination root remains `HOME/.horizon`; configurable runtime
roots are a separate follow-up in #693.

## Shared provider usage

`browser_provider_usage` reads shared remote-provider capacity without allocating
a browser session or requiring the settings UI to be open. Omit `provider` for
all configured profiles, or pass `{"provider":"team_cloud"}` for one profile.
The host resolves each profile's own credential bindings; callers never pass keys
or endpoints. Multiple accounts on one provider need distinct bindings.

The result is `{"providers":[...]}`. Each row contains `provider`, `supported`,
`local_session_limit`, optional `running`, `allowed`, `queued`,
`sampled_at_millis` (Unix milliseconds), and `error`. Missing credentials or API
failures leave counts null and report an error for that row; unsupported adapters
are explicitly marked. This snapshot includes competing clients and is not a
reservation. Hosted capacity remains provider-managed.

This tool requires a live Horizon-launched agent identity, as configured remote
session creation does. A local standalone browser host has no remote-provider
configuration. CLI plans invoke the same tool; see the
[CLI usage example](../horizon-browser-cli/README.md#shared-remote-provider-usage).

## Native Device viewer lifecycle

The compact viewer summary selects the supplied machine name, hostname,
Tailscale name, server-advertised VNC name, then local endpoint. Expand
**Connection details** for selectable supplied labels, the local endpoint,
server name and desktop resolution. Unknown fields are omitted.

Inspect/list expose the same observed information in `server.name` and
`server.desktop_size` (`[width, height]`). Server labels are bounded to 256
characters and control/bidi-formatting characters are flattened. Observations remain labelled as last
connection data after disconnect, clear on reconnect and are never persisted.
Supplied labels persist but are not verified by the server; loopback does not
identify the remote host and no SSH/Tailscale discovery or tunnel is created.


Creation accepts optional creator-supplied host labels:

```json
{"operation":"create","endpoint":"127.0.0.1:5900","identity":{"machine_name":"Lab workstation","hostname":"lab-host","ip_addresses":["192.0.2.10","2001:db8::10"],"tailscale_name":"lab-host.example.ts.net"}}
```

Labels are plain text, trimmed, and limited to 256 characters each. Control and
Unicode bidi-formatting characters are rejected; up to 16
numeric IPv4/IPv6 addresses are accepted. Empty labels become absent values. Create,
list and inspect return the supplied identity separately from the local VNC
endpoint. These labels are not verified by VNC and never change routing or
initiate host discovery. Existing endpoint-only callers remain supported.
Supplied identity persists with the panel. Restored viewers remain stopped until
explicitly reconnected; saved labels do not verify the machine now behind a reused port.
The standalone device CLI controls an explicitly configured target; viewer
creation and identity are exposed through `device_panel` on the browser MCP.


`device_panel` manages read-only native VNC viewers in the caller's current
Horizon workspace. It does not forward input or provision a desktop. Use the
standalone device CLI/MCP with an explicitly configured isolated target for input.

```json
{"operation":"create","endpoint":"127.0.0.1:5900"}
{"operation":"list"}
{"operation":"inspect","panel_id":"<returned id>"}
{"operation":"visibility","panel_id":"<returned id>","visible":false}
{"operation":"reconnect","panel_id":"<returned id>"}
{"operation":"close","panel_id":"<returned id>"}
```

Creation returns a stable id immediately. Inspect `connection`,
`image_received`, `image_displayed`, and `frame_sequence` before claiming live
image evidence. `visible` is the panel's presentation setting; the image can
still be off canvas or clipped. `image_displayed` describes the last completed
UI frame and requires a connected image intersecting the drawing clip. An old
texture after a disconnect is not a live image. The frame sequence counts
uploaded images in the current connection and resets on explicit reconnect.

Hidden and off-canvas viewers continue receiving at their configured refresh rate
without frame-triggered repaints or off-screen GPU uploads. `received_frame_sequence`
counts worker-published image updates independently of `frame_sequence` uploads.
It advances without UI rendering and resets on reconnect; inspection does not
consume the pending image. Revealing a viewer displays the latest retained image,
even if the desktop has since become static. Older hosts omit this field, which
new clients deserialize as zero: that alone does not establish failed reception.
Neither counter is a connection heartbeat. An unchanged desktop can keep both
stationary; use connection errors to identify failures, and require
`image_displayed` separately when proving a live visible view.


All operations require a Horizon-injected caller and host identity. Discovery
and inspection are workspace-scoped. Only the creator/owner may change visibility
or close a viewer. Explicit reconnect can acquire an unowned viewer, including a
restored one; it never steals another agent's viewer. Restore remains stopped.
Closing releases the viewer connection without terminating its target. Ownership
is session-local and is not persisted across application restarts.

Endpoints accept numeric loopback addresses and nonzero ports only, including
`[::1]:5900`; hostnames, remote addresses, URLs and commands are refused. Host
requests expire before dispatch after 10 seconds; the MCP wait is bounded to 15
seconds. On `host_timeout`, list before retrying a mutation because it may have
completed without a delivered result. Hosts predating this API return that
bounded timeout and require a normal application upgrade to gain the capability.
