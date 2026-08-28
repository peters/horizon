---
name: horizon-browser
description: Control, inspect, or audit a live browser panel inside Horizon through the Horizon browser MCP tools.
---

# Horizon browser control

Use the `browser_*` MCP tools as the only agent-facing browser contract. Do
not inspect Horizon runtime files, connect to raw CDP/BiDi/WebDriver endpoints,
or invoke a browser-control CLI. If the MCP tools are unavailable, report that
the Horizon browser MCP server is not connected.

Start with `browser_list` when the panel id is unknown. If it returns no panels,
call `browser_create`; this opens a normal visible panel in the current agent's
Horizon workspace and returns its ready panel id. Omit `backend` to use
Horizon's configured browser, or select `chromium`, `firefox`, or `safari` when
the platform supports it. An optional bare-host `url` defaults to HTTPS while
explicit HTTP remains available. Use `browser_panel` for a known panel. Before
interacting, call `browser_snapshot` or `browser_query` and prefer its
short-lived `ref` in `browser_act`. Navigation, another snapshot or query, and
`browser_wait` can invalidate earlier refs, so reacquire a ref immediately
before an action when the page may have changed.

After navigation or interaction, verify the visible outcome with
`browser_wait`, `browser_query`, or a new snapshot. Use `browser_evaluate` only
when the semantic tools cannot answer the question.

For HTTP or WebSocket observation, first inspect the panel's
`network_capture` field from `browser_list` or `browser_panel`. When supported,
call `browser_network` with `operation: start` **before navigation** so open,
frames, errors, and close are all observed. Use URL filters and payload/file
limits for busy streams. To capture HTTP response content, set both
`include_http: true` and `include_http_bodies: true`, and check
`http_response_body_transport` first. Bodies appear as bounded
`http_response_body` records; they may contain sensitive page data and never
belong in the action audit. The result returns live connection counters and
one private NDJSON export path; it is explicitly safe to inspect that exact
path with read-only tools such as `tail -f`, `jq`, or `rg`. Prefer filtered,
incremental processing over copying a raw high-rate stream into the agent
conversation. Poll `operation: status` for open/closed state and
drop/truncation counters, then call `operation: stop` to flush the file. Do not
infer or inspect any other Horizon runtime path.

Chromium HTTP bodies and WebSocket frames are protocol-native. Firefox HTTP
bodies are native WebDriver BiDi, while WebSocket frames use page
instrumentation because standard BiDi does not expose them; the panel
advertises both distinctions. Safari network capture is currently unsupported.
Do not describe Firefox WebSocket instrumentation as undetectable.

When the user must steer, call `browser_handoff` with a concise reason and stop
issuing actions. Poll `browser_list` until `handoff_pending` becomes false,
then take a fresh snapshot before continuing. Use `browser_audit` to review the
redacted ordered action history or to verify a specific action id.
