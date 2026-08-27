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
```

Released Horizon builds start the same server through private transport
bootstrap plumbing, so bundled integrations do not need a second installed
binary.

## Tool contract

- `browser_list` and `browser_panel` discover safe live-panel state.
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
  navigation for a complete socket lifecycle; while active, agents may use
  ordinary read-only tools such as `tail -f`, `jq`, or `rg` on that exact path.
- `browser_handoff` pauses automation so the user can steer.
- `browser_audit` returns redacted ordered action records.

The server automatically claims and heartbeats a panel using
`HORIZON_BROWSER_ACTOR`. Tool schemas and results never expose raw CDP, BiDi,
WebDriver, manifest, audit-file, or result-file locations. Horizon's locked
manifest queue remains private implementation plumbing, not a second API.

Chromium WebSocket frames come directly from CDP. Standard WebDriver BiDi
currently exposes HTTP lifecycle events but not WebSocket frames, so Firefox
uses an explicitly advertised, pre-document page bridge that batches frames
before crossing BiDi. This bridge is observable by page code and is not an
undetectability feature. Safari network capture is reported as unsupported.

## Dependency boundary

`horizon-browser-mcp` is Horizon-specific and depends on `horizon-core` for
host coordination. The reusable browser engine stays in the separately
publishable `horizon-browser` crate, which does not depend on Horizon, egui,
Tokio, or MCP. A future standalone application can implement
`BrowserCoordination` with its own storage and expose this MCP shape or another
host adapter without pulling Horizon's UI into the engine.
