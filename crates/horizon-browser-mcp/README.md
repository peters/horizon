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
default_tools_approval_mode = "approve"
```

Released Horizon builds start the same server through private transport
bootstrap plumbing and apply that server-scoped approval mode automatically,
so bundled integrations neither need a second installed binary nor interrupt
each browser action with an approval prompt. This does not change approvals for
shell commands, files, or other MCP servers.

## Tool contract

- `browser_list` and `browser_panel` discover safe live-panel state.
- `browser_create` opens a normal visible panel in the calling agent's Horizon
  workspace and returns only after it is ready and owned by that agent. It uses
  the configured backend unless explicitly overridden.
- `browser_navigate` changes the top-level page.
- `browser_snapshot` and `browser_query` return bounded semantic nodes with
  short-lived refs.
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
- `browser_handoff` pauses automation so the user can steer.
- `browser_audit` returns redacted ordered action records.

The server automatically claims and heartbeats a panel using
`HORIZON_BROWSER_ACTOR`. Creation requests are accepted only from an identity
injected into a live Horizon agent panel and are routed to that panel's
workspace. Panel creation and later actions share the redacted audit identity.
Tool schemas and results never expose raw CDP, BiDi, WebDriver, manifest,
audit-file, or result-file locations. Horizon's locked manifest queue remains
private implementation plumbing, not a second API.

Chromium HTTP metadata, response bodies, and WebSocket frames come directly
from CDP. Firefox HTTP metadata and response bodies use native WebDriver BiDi;
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
