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

When the user must steer, call `browser_handoff` with a concise reason and stop
issuing actions. Poll `browser_list` until `handoff_pending` becomes false,
then take a fresh snapshot before continuing. Use `browser_audit` to review the
redacted ordered action history or to verify a specific action id.
