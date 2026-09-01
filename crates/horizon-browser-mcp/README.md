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
env_vars = ["HORIZON_BROWSER_ACTOR"]
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
  returns only after it is ready and owned by that agent. It uses the configured
  backend unless explicitly overridden. Set `visible: false` to start a live
  background panel. If the same agent already owns a panel, creation is rejected
  unless the user explicitly requested another independent session and the call
  sets `allow_additional: true`. Reuse the original panel for iframe, popup,
  dialog, and consent flows instead of creating a helper panel.
- `browser_visibility` shows or hides an existing panel without stopping its
  browser, ownership lease, network capture, or MCP control.
- `browser_navigate` changes the top-level page.
- `browser_snapshot` and `browser_query` return bounded semantic nodes with
  short-lived refs. Snapshots keep iframe boundaries discoverable even when
  cross-origin policy prevents inspecting the frame document.
- `browser_act` clicks, fills, scrolls, reloads, or traverses history. Set
  `count: 2` on a click for a backend-native trusted double-click.
- `browser_wait` verifies present, visible, or hidden selector state.
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
- `browser_handoff` pauses automation so the user can steer.
- `browser_audit` returns redacted ordered action records.

The server automatically claims and heartbeats a panel using
`HORIZON_BROWSER_ACTOR`. Horizon injects that identity into the agent process
and explicitly forwards it to the bundled stdio MCP subprocess. Creation and
visibility requests are accepted only from an identity belonging to a live
Horizon agent panel and are routed to that Horizon instance. Panel lifecycle
and later actions share the redacted audit identity.
The Horizon host stamps every live browser manifest with the agent identities
that currently share the panel's workspace and refreshes the stamp when either
panel moves, so authorization follows workspace membership without restarting
the browser session or the agent. Membership is evaluated inside the same
locked manifest transaction as each claim, heartbeat, queued action, handoff,
and audit read, so a move cannot race past the boundary. Identities are per-panel UUIDs, so separate
Horizon processes sharing one home never authorize each other's panels even
when their workspace ids collide, and manifests without a stamp (older hosts,
or a panel whose host has not stamped it yet) fail closed for Horizon agents.
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

`horizon-browser-mcp` is Horizon-specific and depends on `horizon-core` for
host coordination. The reusable browser engine stays in the separately
publishable `horizon-browser` crate, which does not depend on Horizon, egui,
Tokio, or MCP. A future standalone application can implement
`BrowserCoordination` with its own storage and expose this MCP shape or another
host adapter without pulling Horizon's UI into the engine.
