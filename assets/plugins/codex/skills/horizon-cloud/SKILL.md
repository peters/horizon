---
name: horizon-cloud
description: Inspect Horizon cloud offers and companions, read or act on the cloud list of your workspace, control explicitly authorized companion workers, use a worker Local Network Bridge, or ask for GitHub access from a cloud worker through public MCP tools.
---

# Horizon cloud MCP

Use this skill for cloud offers, the cloud list of your workspace, companion workers,
a worker's Local Network Bridge, and GitHub access on a cloud worker.
Read [the cloud reference](references/cloud.md) for the server and operation you need.
Use tools from the connected server and their current schemas. Do not substitute
private Horizon state for MCP results.

Hetzner offers include all current x86 types in permitted locations. The configured
server types are fallback preferences; they do not restrict explicit worker choices.

Cloud cards show the last-read tailnet device name under Connections > Tailnet.
Copy includes the MagicDNS domain when the worker reports it. A stable-name
fallback requires `horizon-tailnet-contract=2`; it does not supply an unknown
domain. Connect again to refresh names after an administrator rename.

Discovery and price results do not authorize spending or lifecycle changes.
Get explicit authorization for the exact resource before starting or stopping a worker.
Keep credentials in Horizon. Do not send keys, tokens, endpoints, or raw provider
capabilities as tool arguments. Preserve an uncertain operation's identity.

Browser provider catalogs and browser sessions use `horizon-browser`.
This skill does not install servers, start resources, or enable network access on load.
