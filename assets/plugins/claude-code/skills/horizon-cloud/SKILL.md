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
fallback accepts `horizon-tailnet-contract=2` or `horizon-tailnet-contract=4`;
it does not supply an unknown domain. Marker 2 qualifies this display fallback
only; selected-tailnet operations require marker 4. Connect again to refresh
names after an administrator rename.

A selected tailnet requires `horizon-tailnet-contract=4` and an auth key with
only `tag:horizon-worker`. The tailnet policy must define this tag in `tagOwners`.
The worker checks `Self.Tags` after enrollment and once at each resume.
An untagged node or a node with extra tags is refused and logged out.
There is no continuous tag monitoring or separate tag for each workspace.
The tailnet policy controls reachability. The current quick-start pin predates
marker 4 and refuses a selected tailnet until the image and pin update.

Personal API token and OAuth client bindings stay in the desktop OS store,
independently of enrollment auth keys. Stored presence reports kind only; it
does not prove API validity, scope or network access. Saving does not remove
nodes. Never pass an API secret to MCP or a worker.
This foundation supplies host storage APIs; an API editor and UI/CLI/MCP status
adapters follow separately. API storage does not qualify an older worker image.

Discovery and price results do not authorize spending or lifecycle changes.
Get explicit authorization for the exact resource before starting or stopping a worker.
Keep credentials in Horizon. Do not send keys, tokens, endpoints, or raw provider
capabilities as tool arguments. Preserve an uncertain operation's identity.

The cloud header's × asks to delete provider resources before removing the cloud.
A current empty record offers Remove cloud, even after a prior failure. After
deletion fails, or when a deployed cloud's resource record is missing or unreadable, Remove from Horizon anyway leaves provider
resources and saved records in place; they can still cost money. While a local
or another controller's operation runs, the dialog offers only Cancel.
This UI close dialog does not grant lifecycle authorization through MCP.

An Upkeep is a cloud that Menu > New Upkeep starts for the person: an always-on
Hetzner worker on which one Codex loop reviews, fixes and merges the pull requests of
chosen bots in chosen repositories, with a board beside its card. It costs money while
its worker runs and needs a GitHub sign-in, a Hetzner project and a Codex login in Cloud
settings. Only the person starts one in the dialog. Horizon allows the loop's GitHub
access for the chosen repositories without asking; every other request still waits on
the card. See `docs/upkeep.md` for its limits and trust boundaries.

Browser provider catalogs and browser sessions use `horizon-browser`.
This skill does not install servers, start resources, or enable network access on load.
