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
Settings > Tailnets displays nonsecret API credential status and supplies masked
owner entry, replacement and removal. Saved values never fill a draft.
Read `cloud_companions.tailnet_api_credentials` for cached ready, pending or
unavailable status without Settings. The CLI is
`cloud_deploy tailnets STATE_ROOT [TAILNET_ID]`. API status does not qualify an
older worker image. Cached status is not an OS-store watcher; use Tailnets Refresh
for an explicit fresh read. Never pass an API secret to MCP.
Use `tailnet_api_credential` to request `set_personal_token`, `set_oauth` or `remove`
for an exact saved tailnet ID and caller-chosen UUID request ID. Set requests carry
kind metadata only; the person enters values and selects Save in Settings. The person fulfills
it in Settings > Tailnets; requests never open Settings or authorize a write.
Read `status` with the same IDs after a lost answer. `cancel` cancels only a pending
request; a running transaction remains uncertain until its result arrives.
Credentials are desktop-global, so a workspace agent cannot approve its own request.
Status qualification uses the production UI, actual CLI and normal AGENT public
MCP responses; baseline id/name evidence alone does not qualify API status.

Machine-local destination preference helpers retain a checkout/profile scope and
nonsecret account or exact registry grant metadata. The staged launch engine uses
that choice before image work; new automatic launches stay off until the selector is complete. A saved preference is not proof of authentication or registry
permission. Keep these settings outside repository files and use the shared
transactional save helper to preserve other windows' choices.

Image publication credentials belong on the host. An operation preparation helper
retains the authenticated provider account ID and owns a separate temporary Docker
login. The engine retains its account and renews the operation login before push.
Prebuilt profiles still require an explicit image; build-profile omission is staged. Never copy publication credentials to a worker or repository.


The staged publication helper can recover a retained account even when another
account is cached: it requests device sign-in for the recorded immutable identity
and checks it before saving a new credential chain. A wrong account preserves
the previous chain and private operation login. A renamed matching account is
refused before a futile prompt because its recorded namespace cannot change.
Explicit account selection starts
a new device sign-in. Launch preparation uses retained-account recovery; a visible
selector and account switch remain separate from this staged API.

Discovery and price results do not authorize spending or lifecycle changes.
Get explicit authorization for the exact resource before starting or stopping a worker.
Keep credentials in Horizon. Do not send keys, tokens, endpoints, or raw provider
capabilities as tool arguments. Preserve an uncertain operation's identity.

A CPU cloud with an exact Container registry binding uses that read-only grant instead
of the legacy provider pull login. Allocation requires verification of its immutable
image digest. The host keeps the verified material; a file change cannot replace it.
No publishing credential reaches the worker. Unbound images keep their legacy behavior.
Initial image and rebuild preparation select a host-only journal for direct hosts,
without a RunPod key. Provider registry operations keep their original account scope.
See `docs/testing/procedures/bound-worker-pull.md` for synthetic checks.

Failed cloud cards diagnose output from the failed step's current visit only.
A Hetzner image rebuild replaces the server and keeps its workspace volume.
`error during placement` reports a provider placement refusal after the image
steps. A prior `invoke-rc.d: policy-rc.d denied execution of start.` notice
reports a skipped service action during package installation, not that refusal.
Retrying the live worker can allocate a server and needs lifecycle authorization.

The cloud header's × asks to delete provider resources before removing the cloud.
A current empty record offers Remove cloud, even after a prior failure. After
deletion fails, or when a deployed cloud's resource record is missing or unreadable, Remove from Horizon anyway leaves provider
resources and saved records in place; they can still cost money. While a local
or another controller's operation runs, the dialog offers only Cancel.
Saved-record checks run in the background. While the dialog shows Checking, wait
for its result or use Cancel. A final removal checks the record again and keeps
its operation lock until Horizon applies the result. Cancel preserves the cloud;
a blocked filesystem read can finish later but cannot remove a newer cloud.
Cancel clears automatic close intent and restores its panels. A provider deletion
that already started can still finish. A Ready connection watch permits explicit
Remove from Horizon anyway after deletion fails; a busy operation still refuses it.
This UI close dialog does not grant lifecycle authorization through MCP.

An Upkeep is a cloud that Menu > New Upkeep starts for the person: an always-on
Hetzner worker on which one Codex loop reviews, fixes and merges the pull requests of
chosen bots in chosen repositories, with a board beside its card. It costs money while
its worker runs and needs a GitHub sign-in, a Hetzner project and a Codex login in Cloud
settings. Only the person starts one in the dialog. Horizon allows the loop's GitHub
access for the chosen repositories without asking; every other request still waits on
the card. On a current worker image, the worker also does this by itself while no Horizon
runs, and starts the loop again after a container restart. Once a day, after the repositories' agents, the loop runs one daily maintenance
agent for the whole Upkeep that does the "Daily maintenance" tasks of the shared
instructions the repositories' AGENTS.md link; its read request for that shared
repository waits on the card too. See `docs/upkeep.md` for its limits and trust
boundaries. From an agent panel in its workspace, `cloud_upkeep` reads the Upkeep,
proposes repositories or a new Upkeep for the person to confirm, and removes, pauses,
resumes or runs it (see the Upkeep section of [the cloud reference](references/cloud.md)).

The board of an Upkeep has **Run now**, **Pause** / **Resume** and **Repositories**.
They are the person's controls in the Horizon UI: Run now and Pause go to the worker
with the next exchange, and Repositories saves only the repositories that the person
added or removed. A board that a config file opens has no buttons. Agents reach the same
actions through `cloud_upkeep`; the board shows each agent change below its title.

For a UI check that a repository's rules require, an Upkeep cloud declares one companion
for each of its repositories whose checkout on this computer commits a `ui-acceptance`
profile. The person creates that cloud with New cloud in the Upkeep's workspace and
checks it on the Upkeep card. The Upkeep agent starts and stops it through its worker's
exchange, not through the companion tools, and Horizon stops it when the run ends; it
needs the person's Horizon running. See "UI acceptance on companion clouds" in
`docs/upkeep.md`.

Browser provider catalogs and browser sessions use `horizon-browser`.
This skill does not install servers, start resources, or enable network access on load.

### Retained publication record prerequisite

Cloud persistence can retain a recipe, resolved destination and nonsecret publisher
identity without activating a publishing flow. Records contain no credentials and
prove no current registry permission. Deployment format 5 and paired allocation
format 3 prevent older readers from erasing that identity; migration retains the
original enrollment and worker ownership evidence. Invalid or downgraded records
are refused. New staged API launches retain their choice before image work.
A retry or rebuild must authorize that same account or publication grant. Reusing
an existing image, including reconnect and resize, needs no publishing login.
See docs/testing/procedures/publication-records.md.


### Retained enrollment evidence

Cloud records can retain exact worker device IDs and unresolved capture attempts across stop, deletion and redeploy. They bind saved tailnet, provider, worker and allocation request identity in the owning locked cloud record. Selected deployments and reconnects capture the private root daemon Self.ID before readiness. This adds no node deletion operation. Typed reasons for the later cleanup integration distinguish missing device identity, missing desktop API access and offline access; keep uncertain records and never delete by hostname/tag guesses. API credentials stay on the desktop.

The receipt-capture adapter reads only the worker's actual tagged Self.ID via pinned root SSH and the existing userspace daemon socket, persists it before readiness and records uncertain attempts before enrollment. It binds the existing allocation request and saved tailnet ID to the owning cloud root. Worker agents cannot query this private root lane. No API secret is sent to a worker and no API editor or node-delete operation is activated by this adapter.

Saved cloud launch confirmations retain their image intent in a conditional wire
format. Existing launches with no reviewed choice keep their flat encoding.
An explicit recipe-image confirmation also stays flat and must match the exact
profile image. Publisher choices use a version-2 envelope that older readers
refuse. Compatible readers retain the full choice, and desktop preparation forwards
that choice and its named profile to the shared engine. This bridge activates no
selector or automatic publication.
An unstarted legacy launch with no publisher choice keeps its explicit recipe target.
An image-omitting build keeps automatic intent.
A durable cloud record keeps its own publishing identity.
Ordinary unknown legacy fields remain compatible, but mixed or downgraded
publisher envelopes and reserved null keys are refused. Publisher choices require
a build recipe; prebuilt launches keep their flat legacy or recipe confirmation.

The desktop NodeApi SDK uses exact recorded IDs and desktop-only API credentials. Core integration is behind the default-off `tailnet-node-cleanup` feature and requires cloud-workspaces. Default UI and CLI builds do not automatically remove nodes. With the feature enabled, the original owner Delete controller calls the SDK only after provider deletion; missing, denied, offline and unverified404 results retain exact IDs. Unknown attempts remain manual. Redeploy refuses unresolved node cleanup before releasing storage journals or resetting prior-worker history. Confirmed receipts remain recorded; fresh clouds and None without enrollment can reopen. Stop and companion MCP grants remain unchanged; no MCP cloud-delete operation is added.

Enabled backend cleanup also refuses normal card removal while its locked record remains pending. Dedicated retry presentation, precise close errors, the explicit owner discard warning and default activation belong to the final UI outcome; they are not supplied by this backend prerequisite.

See docs/architecture/tailnet-node-cleanup.md and docs/testing/procedures/tailnet-node-cleanup.md.

### Staged portable build engine

The development `cloud_deploy` helper prints destination/account metadata in its
plan and retains it in the confirmation token. Retry and rebuild keep the original
publisher; a changed preference cannot redirect a cloud. Personal write credentials
stay host-only and are renewed before push. A private image needs a separate verified
read-only pull grant. New automatic launches remain off until the complete UI and
public destination controls are delivered. Do not enable a
partial user flow or remove explicit development image pins in this stage.
See docs/testing/procedures/portable-build-engine.md.

A retained publishing record also fixes the rebuild profile name. A request for
another profile is refused even when that profile names the same destination or
omits its image. Recipe edits within the recorded profile still rebuild to the
recorded destination.

A new desktop record requires its saved publisher choice to match current local
intent, including when an override was removed. Existing records retain their own
publication; automatic intent without an override remains valid. A legacy
image-omitting launch refuses a later scoped override until it is reviewed.
An automatic record that still has no publisher needs the same review on retry.
With the staging setting off, saved non-recipe publisher choices refuse before
creating any record; explicit legacy recipe images continue to work.

Desktop preparation now preserves saved publisher choices, and rebuild previews
use the cloud's retained publication and profile. New automatic desktop launches
remain disabled until the destination selector and public destination controls
are complete. Keep the setting off for ordinary desktop use; metadata forwarding
is not authorization to publish or proof of a qualified new-cloud selector.

A rebuild confirmation also snapshots the exact retained publication. Its token
covers the publisher account or grant even when the resolved image and files are
identical. The locked execution compares that identity before reading credentials;
a changed or missing publisher confirmation requires a fresh plan. Legacy plans
without a retained publication keep their existing token format. The development
CLI displays the recorded destination and account with the rebuild plan. A retained
publisher requires that printed token even when the working copy is clean.

### Host-only pull preparation prerequisite

The provider-aware registry helper can prepare a direct-host pull grant without a
RunPod key. It stores host records separately from provider registry records and
refuses provider registry creation from a host grant. Both journal paths must stay
outside the source. New private directories sync locked pending intents and their directory entries
before mkdir, with parent syncs before a
child or pull identity is pinned. Interrupted creation is recovered first; existing
roots with no pending intent do not require a readable parent. Both scopes pin the same
read-only identity for each generation. Rotate the generation when that identity
changes. Initial image and rebuild callers choose the helper for their provider.
This helper does not prove a live registry pull or revoke an installed
host credential. See docs/testing/procedures/host-pull-journal.md.

On Unix, private registry credentials preserve the machine's Docker context and plugin
locations. They copy no unrelated registry login, credential helper or proxy setting.
Explicit host settings keep their existing precedence.

Host-only registry checks use `verify_pull` with an immutable image and `pull_status`
with an exact repository and generation. They require no compute account, create no
provider binding and return `scope: host`. Status reads saved validation only; it
reads no pull or compute credential and proves no current issuer permission.
The Cloud settings card uses these checks directly. Its separate provider section
keeps legacy verify/status/reconcile/revoke behavior and requires the compute account.
Revocation there removes provider access, not issuer tokens or installed host access.
The CLI is `cloud_deploy registry SETTINGS ACTION_JSON_FILE`; the optional stdio
`cloud_registry` tool uses the same operations and machine-bound settings file.

A refused worker pull or failed/cancelled validation keeps that repository
unverified across settings reopenings and board-session switches in the current
app session. Reading saved
status does not clear this failure; a new successful verification is required.
After an app restart, saved successful history is available again. A verification displays its
own image result rather than substituting history from the other scope.
The settings card retains its saved repository and grant identity separately
from the editable image. Malformed or other-repository images invalidate that
card's hint without validating another binding; stale grants leave current proof alone.
Worker pull refusals are retained before settings load, including a failed or
replaced load.

Saved host pull history never opens the provider journal or reads its compute key,
even when a newer provider validation exists. Provider controls keep their own scoped
status message. A fresh verification displays its own result.

An explicit provider action can retain its own active provider proof when host
history was read successfully and is empty. A busy or unreadable host journal
never permits that fallback. Reopening settings reads host history only; it does
not load compute credentials to recover provider history.

### Staged destination management policy

The shared core destination manager reads metadata without opening the settings
editor or loading provider credentials. It lists choices, selects an existing
registry grant, resets a shown override or explicitly signs in to a derived
personal destination. Every mutation compares the exact shown local override.
Host-owned, bounded operation receipts retain the actor, host, repository and
settings root through polling or cancellation. Cancellation is a request, not
proof that a write did not commit; list saved state after an expired receipt.
The public host adapter below uses this policy. Automatic publication remains
disabled until final integration. See docs/testing/procedures/image-destination-management.md.

Direct-root receipts require the exact canonical Git root. Public host adapters
use the distinct launch-directory entry point: the shared worker normalizes the
registered local directory, while receipt ownership retains that exact path.
Cancellation during metadata selection is reported as cancellation, never as a
successful listing with only an inline selection error.

Saved pull metadata compares the registry repository without a tag or digest.
It retains the registry port and exact path; it is not a live access check.

### Public image destination management

Use `cloud_image_destination` from a live local agent panel. It uses the panel's
registered launch directory and resolves its checkout in the shared background
worker, including subdirectories. The host supplies the settings root and source;
never pass a root, source path, token or credential value. Remote worker and SSH
panels cannot change desktop publishing choices. Admission and retained-operation
polling check both saved and live cloud membership, including session restoration.
No Settings window is required.

Supply a fresh lowercase UUID `operation_id` and a named build `profile` for a new
action. Start with `action: {"operation":"list"}`. The result gives the selected
account/destination, exact `shown_override` and existing registry choices. For
`select`, pass a listed `destination` and the exact `shown` snapshot. For `reset`,
pass that same `shown`. A stale snapshot refuses without replacing another choice.
The preference stays machine-local; source YAML remains unchanged.

Every action can return a pending receipt. Poll `status` with the same operation
ID and no profile until `done` is true, then read its nested result or error.
A receipt alone does not prove that listing or a mutation completed.

Only `authenticate` starts personal publishing sign-in. Pass `shown` and set
`switch: true` only after the person asks to change account. Show the device code
and verification URI returned by the operation. Never collect a token yourself.
Poll `status` with the same ID and no profile at least every ten seconds while
sign-in runs. Thirty seconds without readback requests cancellation. `cancel`
requests cancellation only for that owned operation. After a timeout, read status
with the same ID; do not create another mutation. Receipts expire and disappear on
host restart. List saved state and review it before retrying an expired operation.

The normal CLI plan runner exposes the same tool and schema through
`horizon-browser run PLAN.json`; its stdio MCP server exposes the same operation.
Existing clouds keep their recorded publisher. Retained publication status is a
separate follow-up outcome.
The staging setting still defaults to false, so mutations remain disabled until
final integration. See docs/testing/procedures/image-destination-public.md.

Management rechecks the staging setting in the same transaction as the exact
override comparison. If it is disabled while sign-in waits, no destination
preference commits. This does not revoke authorization already completed at the
issuer or prove that a credential-chain write did not occur.
