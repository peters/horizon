---
name: horizon-cast
description: Cast Horizon panels, workspaces, or its main window to Apple TV through the public cast MCP tool. Use for receiver discovery, pairing, casting status, and stopping casts.
---

# Horizon casting

Use the public `cast` tool on the Horizon browser MCP server (`--browser-mcp`).
The calling agent must run in a Horizon workspace on Linux.
Read [the casting reference](references/casting.md) before receiver or session operations.
Use the current tool schema for arguments. If the tool or supporting host is absent,
report the missing capability. Do not use private runtime files or another capture path.

A task to examine casting does not authorize a cast to a real TV.
Start, pair, stop, or forget only within the user's requested receiver and task.
Application capture needs the person's permission in the Cast picker.
An agent cannot grant that permission.
