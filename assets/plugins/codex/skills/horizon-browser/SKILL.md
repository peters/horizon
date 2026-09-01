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
call `browser_create`; this opens a panel in the current agent's Horizon
workspace and returns its ready panel id. If it returns a usable panel, reuse
that panel for iframe, popup, dialog, and consent interactions. Never create or
reveal a helper panel as a workaround. Only when the user explicitly requests
another independent browser session may you call `browser_create` with
`allow_additional: true`. Omit `backend` to use Horizon's
configured browser, or select `chromium`, `firefox`, or `safari` when the
platform supports it. Set `visible: false` for background automation; use
`browser_visibility` to show or hide the live panel later without stopping its
session, capture, ownership, or MCP control. An optional bare-host `url`
defaults to HTTPS while explicit HTTP remains available. Use `browser_panel`
for a known panel. Before
interacting, call `browser_snapshot` or `browser_query` and prefer its
short-lived `ref` in `browser_act`. Navigation, another snapshot or query, and
`browser_wait` can invalidate earlier refs, so reacquire a ref immediately
before an action when the page may have changed.

Snapshots expose iframe boundaries as `iframe` nodes. If the current top-level
semantic tools cannot reach the embedded frame content, call `browser_handoff`
on the original panel so the user can complete the interaction; do not open a
separate panel for the frame.

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
belong in the action audit. The result returns live connection counters and one
private NDJSON export path. Prefer `browser_network_watch` for event-driven
monitoring: filter by URL and event kind, leave payloads excluded unless needed,
then pass the returned `capture_id` and `next_sequence` into the next call. It
reports timeout, capture stop/replacement, gaps, drops, truncation, file limits,
and writer failure explicitly. For sustained local analysis, it is also safe to
inspect the exact path returned by `browser_network` with read-only tools such
as `tail -f`, `jq`, or `rg`; never infer or inspect another Horizon runtime
path. Call `operation: stop` to flush the capture.

Chromium HTTP bodies and WebSocket frames are protocol-native, but CDP cannot
return a `fetch()` body the page drained with `response.blob()`, and a
top-level navigation to a PDF yields only the viewer's HTML shell. Such
`http_response_body` records carry an `error` and no `payload`; when the bytes
matter, read `text()` or `arrayBuffer()` or leave the body unread. Firefox HTTP
bodies are native WebDriver BiDi, while WebSocket frames use page
instrumentation because standard BiDi does not expose them; the panel
advertises both distinctions. Safari network capture is currently unsupported.
Do not describe Firefox WebSocket instrumentation as undetectable.

When the user must steer, call `browser_handoff` with a concise reason and stop
issuing actions. Poll `browser_list` until `handoff_pending` becomes false,
then take a fresh snapshot before continuing. Use `browser_audit` to review the
redacted ordered action history or to verify a specific action id.
