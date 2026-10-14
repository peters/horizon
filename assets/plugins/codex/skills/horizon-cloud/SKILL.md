---
name: horizon-cloud
description: Inspect Horizon cloud offers and companions, read or act on the cloud list of your workspace, see or propose changes to its Upkeep of Dependabot pull requests, control explicitly authorized companion workers, use a worker Local Network Bridge, or ask for GitHub access from a cloud worker through public MCP tools.
---

# Horizon cloud MCP

Use this skill for cloud offers, the cloud list of your workspace, Upkeep, companion workers,
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
The tailnet policy controls reachability. The quick-start pin contains marker 4;
selected-tailnet startup still requires storage that enforces private root modes.
The tested RunPod persistent storage does not and is unsupported. Never weaken
these checks; **None** does not bypass isolation. CPU evidence is not GPU qualification.

Personal API token and OAuth client bindings stay in the desktop OS store,
independently of enrollment auth keys. Stored presence reports kind only; it
does not prove API validity, scope or network access. Saving does not remove
nodes. Never pass an API secret to MCP or a worker.
Settings > Tailnets displays nonsecret API credential status only; no API editor
ships in this milestone. Read `cloud_companions.tailnet_api_credentials` for
cached ready, pending or unavailable status without Settings. The CLI is
`cloud_deploy tailnets STATE_ROOT [TAILNET_ID]`. API status does not qualify an
older worker image. Cached status is not an OS-store watcher; use Tailnets Refresh
for an explicit fresh read. Never pass an API secret to MCP.
Status qualification uses the production UI, actual CLI and normal AGENT public
MCP responses; baseline id/name evidence alone does not qualify API status.

Image publication credentials belong on the host. An operation preparation helper
retains the authenticated provider account ID and owns a separate temporary Docker
login. It has no launch caller yet; existing launch behavior and explicit `image`
requirements remain. Never copy publication credentials to a worker or repository.

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
the card. See `docs/upkeep.md` for its limits and trust boundaries. From an agent
panel in its workspace, `cloud_upkeep` reads the Upkeep, proposes repositories or a new
Upkeep for the person to confirm, and removes, pauses, resumes or runs it (see the
Upkeep section of [the cloud reference](references/cloud.md)).

The board of an Upkeep has **Run now**, **Pause** / **Resume** and **Repositories**.
They are the person's controls in the Horizon UI: Run now and Pause go to the worker
with the next exchange, and Repositories saves only the repositories that the person
added or removed. A board that a config file opens has no buttons. Agents reach the same
actions through `cloud_upkeep`; the board shows each agent change below its title.

Browser provider catalogs and browser sessions use `horizon-browser`.
This skill does not install servers, start resources, or enable network access on load.
