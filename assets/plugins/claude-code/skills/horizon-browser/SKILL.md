---
name: horizon-browser
description: Control, inspect, or audit a live browser panel inside Horizon through the Horizon browser MCP tools.
---

# Horizon browser control

Use the `browser_*` MCP tools as the only agent-facing browser contract. Do
not inspect Horizon runtime files, connect to raw CDP/BiDi/WebDriver endpoints,
or invoke a browser-control CLI. If the MCP tools are unavailable, report that
the Horizon browser MCP server is not connected.

Start with `browser_list` when the panel id is unknown. Use `browser_panel` for
a known panel. Before interacting, call `browser_snapshot` or `browser_query`
and prefer its short-lived `ref` in `browser_act`. Navigation, another snapshot
or query, and `browser_wait` can invalidate earlier refs, so reacquire a ref
immediately before an action when the page may have changed.

After navigation or interaction, verify the visible outcome with
`browser_wait`, `browser_query`, or a new snapshot. Use `browser_evaluate` only
when the semantic tools cannot answer the question.

For HTTP or WebSocket observation, first inspect the panel's
`network_capture` field from `browser_list` or `browser_panel`. When supported,
call `browser_network` with `operation: start` **before navigation** so open,
frames, errors, and close are all observed. Use URL filters and payload/file
limits for busy streams. The result returns live connection counters and one
private NDJSON export path; it is explicitly safe to inspect that exact path
with read-only tools such as `tail -f`, `jq`, or `rg`. Poll `operation: status`
for open/closed state and drop/truncation counters, then call `operation: stop`
to flush the file. Do not infer or inspect any other Horizon runtime path.

Chromium WebSocket frames are protocol-native. Firefox WebSocket frames use
page instrumentation because standard WebDriver BiDi does not expose them;
the panel advertises this distinction. Safari network capture is currently
unsupported. Do not describe Firefox instrumentation as undetectable.

When the user must steer, call `browser_handoff` with a concise reason and stop
issuing actions. Poll `browser_list` until `handoff_pending` becomes false,
then take a fresh snapshot before continuing. Use `browser_audit` to review the
redacted ordered action history or to verify a specific action id.
