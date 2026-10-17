---
name: horizon-cloud
description: Inspect Horizon cloud offers and companions, read or act on the cloud list of your workspace, see or propose changes to its Upkeep of Dependabot pull requests, control explicitly authorized companion workers, use a worker Local Network Bridge, or ask for GitHub access from a cloud worker through public MCP tools.
---

# Horizon cloud MCP

Use this skill for cloud offers, the cloud list of your workspace, Upkeep, companion workers,
a worker's Local Network Bridge, and GitHub access on a cloud worker.
Read [the cloud reference](references/cloud.md) for the server and operation you need.
Use tools from the connected server and their current schemas. Do not substitute
private Horizon state for MCP results. Cloud-list `worker_image` and
`image_publication` are retained metadata from the matching cloud runtime, or
`null` when unavailable or the image is unresolved. Registry publication includes
an opaque nonsecret grant ID, never a credential value or path. These fields do
not prove current registry access.

Hetzner offers include all current x86 types in permitted locations. The configured
server types are fallback preferences; they do not restrict explicit worker choices.

Cloud cards show the last-read tailnet device name under Connections > Tailnet.
Copy includes the MagicDNS domain when the worker reports it. A stable-name
fallback accepts `horizon-tailnet-contract=2` or `horizon-tailnet-contract=4`;
it does not supply an unknown domain. Marker 2 qualifies this display fallback
only; selected-tailnet operations require marker 4. Connect again to refresh
names after an administrator rename.

The worker source recipes include pinned Unix PROXY1 and explicit flow
cancellation. `--horizon-flow-cancel` / JSON `HorizonFlowCancel` defaults false;
only Linux raw Unix PROXY1 without TLS supports it. The root controller must own
the original private listener and exact daemon pair; agents must not substitute
routes or infer identity from a header/config/descriptor. Payload write EOF keeps
reverse flow open. Control `DRAINED=0x01` then EOF confirms daemon copying, not
remote application delivery. This transport does not activate M2; actual Kernel
and userspace Worker consumer qualification remains separate. Same-PID TCP and
generic-proxy output are insufficient. Published pins stay unchanged until actual
publication. See `references/cloud.md` and
`docs/testing/procedures/tailscale-unix-proxy.md` for current native-image gates.

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
For an authorized new cloud, the development CLI accepts
`cloud_deploy select-tailnet CATALOG_ROOT CLOUD_ID TAILNET_ID` before allocation.
Use the original saved catalog root and an existing saved ID. Deploy with
`STATE_ROOT` equal to `CATALOG_ROOT/CLOUD_ID`. This writes only the selected
network through the existing lifecycle lock; it reads no secret and starts no worker.
A provisioned cloud keeps its network. Omission or an empty ID refuses instead of
clearing it. Windows refuses cloud-state mutation before creating paths.
See docs/testing/procedures/cloud-tailnet-selection.md.

Use `tailnet_api_credential` to request `set_personal_token`, `set_oauth` or `remove`
for an exact saved tailnet ID and caller-chosen UUID request ID. Set requests carry
kind metadata only; the person enters values and selects Save in Settings. The person fulfills
it in Settings > Tailnets; requests never open Settings or authorize a write.
Read `status` with the same IDs after a lost answer. `cancel` cancels only a pending
request; a running transaction remains uncertain until its result arrives.
Credentials are desktop-global, so a workspace agent cannot approve its own request.
Status qualification uses the production UI, actual CLI and normal AGENT public
MCP responses; baseline id/name evidence alone does not qualify API status.

The Linux Worker `protected-ingress` feature is off by default and has no public
command. Its live reader checks the original fixed read-only host receipt,
file metadata and current kernel boundary, retaining the host allocation. Serialized facts do
not recreate custody or prove source preservation. Rootless/remapped workers,
Gate activation and full M2 qualification remain unsupported by this component.
Read docs/architecture/protected-ingress-reader.md before its owned-VM test.

The internal `one-service-gateway` feature stays off by default. It supplies an
exact LAN/tailnet endpoint engine and an internal pinned Unix SSH attachment.
No UI, CLI or MCP operation enables it. Use the existing owner-enabled Local
Network Bridge tools for current operations. Private attachment needs the trusted
non-symlink `/tmp` policy; non-Unix attachment is unsupported. M1-1 still needs owner
controls, saved membership, interface parity and actual native
LAN/tailnet qualification. Read docs/architecture/pinned-unix-ssh-attachment.md.

The same internal feature now includes a trusted Core owner controller. It saves
Armed before private listener/SSH dispatch and retains the original Store, worker
and pinned route until physical cleanup finishes. Decoded owner scope or retained
metadata grants no approval. No UI/CLI/MCP route invokes it yet; complete M1 still
needs actual owner controls and parity. Read docs/architecture/one-device-owner-admission.md
and its component procedure before using the controller.

The Linux Worker `trusted-tcp-producer` feature is off by default. It checks the
original root daemon through a root-private Unix Serve backend before bounded
header and WhoIs comparisons. Generic SOCKS/HTTP proxy dials cannot reach that
backend. It requires the pinned Unix Serve PROXY-v1 runtime with explicit flow cancellation; qualification
is pending. An opaque AcceptedStream retains the original payload and private control.
Write EOF keeps control alive; whole cancel closes both immediately. Normal completion requires
daemon DRAINED0x01 plus EOF, bounded by the caller's captured deadline and cancellation. It adds no Gate, keeper,
Serve/Funnel mapping or public operation. Read
docs/architecture/trusted-tcp-producer.md and its component procedure.

Producer acceptance is opaque and bound to its exact original backend instance;
a successor at the same path cannot adopt it. Retain only the weak pending
cancellation handle before identification. It keeps no queued descriptor alive
and continues to cancel the original payload/control after acceptance. Serialized
facts, path equality and a retained bare socket clone are not this authority.

The Linux worker `exposure-gate` feature is also off by default. It provides a
bounded actual-peer CIDR/Public TCP component only; no public operation starts
it. The direct constructor refuses DNS/tag and TailnetOnly before binding.
The separate default-off `trusted-exposure-gate` adapter consumes an original
Producer and opaque accepted Unix stream for Restricted DNS/tag/CIDR rules.
Pending streams expose no raw FD; Gate keeps only a weak cancellation handle and
compares the private original-accept cookie before named admission.
The original root-private Unix backend has no TCP bridge; direct public sockets
retain their actual-peer CIDR behavior. The shared relay preserves byte budgets,
half-close and physical cancellation on both transports. It owns no
Serve mapping, protected runtime, keeper or public operation; TailnetOnly still
refuses. Actual named allow/deny in both daemon modes remains a required gate.
Follow the worker Gate sections of the cloud reference and their procedures.

Retained exposure records store bounded comparison data and service intent.
The `cloud-exposure` feature enables record transitions and SDK availability for
an ignored qualification test. It stays off by default and adds no public exposure operation. Present exposure selects Deployment 7 and paired Allocation 5;
absent exposure keeps the earlier formats. A decoded record supplies no live
approval, transport or cleanup proof. Read the retained exposure metadata
section of the cloud reference.

The default-off SDK `exposure-firewall` management controller retains its original
client, sibling rules and foreign policies. It needs the actual owner's durable
journal, pinned SSH management policy and physical Gate teardown; no public tool
starts it. Read the exposure management section of the cloud reference.

The Protocol `exposure-control` feature is off by default. Its bounded request,
reply, member and Worker observations are passive comparison facts. They grant
no held stream, owner approval, live membership or runtime readiness. There is no
public activation. See docs/architecture/exposure-control.md and its procedure.

Machine-local destination preference helpers retain a checkout/profile scope and
nonsecret account or exact registry grant metadata. The launch engine retains
that choice before image work. A saved preference is not proof of authentication or registry
permission. Keep these settings outside repository files and use the shared
transactional save helper to preserve other windows' choices.

Image publication credentials belong on the host. An operation preparation helper
retains the authenticated provider account ID and owns a separate temporary Docker
login. The engine retains its account and renews the operation login before push.
Prebuilt profiles still require an explicit image; build profiles may omit it. Never copy publication credentials to a worker or repository.


The publication helper can recover a retained account even when another
account is cached: it requests device sign-in for the recorded immutable identity
and checks it before saving a new credential chain. A wrong account preserves
the previous chain and private operation login. A renamed matching account is
refused before a futile prompt because its recorded namespace cannot change.
Explicit account selection starts
a new device sign-in. Launch preparation uses retained-account recovery; a visible
selector and public destination controls use explicit account selection;
existing-cloud recovery still requires its recorded identity.

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
repository waits on the card too. Pull requests that Upkeep agents open use an
`upkeep/` branch and the `upkeep` label; the loop finds them and carries them through
the same gates. The default instructions ask for the checks that each repository's
`AGENTS.md` makes apply to a change, and no more. Instructions that the person kept
as the default follow a newer default of Horizon with the next exchange; changed
instructions stay as written. See `docs/upkeep.md` for its limits
and trust boundaries. From an agent panel in its workspace, `cloud_upkeep` reads the
Upkeep, proposes repositories or a new Upkeep for the person to confirm, and removes,
pauses, resumes or runs it (see the Upkeep section of [the cloud reference](references/cloud.md)).

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

### Retained publication records

Cloud persistence can retain a recipe, resolved destination and nonsecret publisher
identity. Records contain no credentials and
prove no current registry permission. Deployment format 5 and paired allocation
format 3 prevent older readers from erasing that identity; migration retains the
original enrollment and worker ownership evidence. Invalid or downgraded records
are refused. New launches retain their choice before image work.
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
that choice and its named profile to the shared engine. New cloud supplies the reviewed destination and publishing identity.
Build profiles may omit the image; explicit images remain supported.
An unstarted legacy launch with no publisher choice keeps its explicit recipe target.
An image-omitting build keeps automatic intent.
A durable cloud record keeps its own publishing identity. An omitted-image launch
also keeps an explicit image already retained by a legacy record, even after a
local destination override changes.
Ordinary unknown legacy fields remain compatible, but mixed or downgraded
publisher envelopes and reserved null keys are refused. Publisher choices require
a build recipe; prebuilt launches keep their flat legacy or recipe confirmation.

The desktop NodeApi SDK uses exact recorded IDs and desktop-only API credentials. The existing owner cloud-delete controller now calls it after original provider deletion. Missing, denied, offline and unverified404 results retain the exact IDs. Unresolved capture attempts receive no API request and stay manual, even after another receipt succeeds. Different saved-tailnet bindings stay independent. Stop and companion MCP grants remain unchanged; no MCP cloud-delete operation is added. Redeploy refuses unresolved node cleanup before releasing storage journals or resetting prior-worker history. Confirmed receipts remain recorded; fresh clouds and None without enrollment can reopen.

Manage shows pending node cleanup and offers Retry node cleanup through the existing owner Delete controller. Normal removal and automatic close keep the pending card after a locked record check. Explicit native Remove from Horizon anyway preserves its record and warns that retry controls disappear. It does not confirm node removal.

See docs/architecture/tailnet-node-cleanup.md and docs/testing/procedures/tailnet-node-cleanup.md.

### Portable build engine

The development `cloud_deploy` helper prints destination/account metadata in its
plan and retains it in the confirmation token. Retry and rebuild keep the original
publisher; a changed preference cannot redirect a cloud. Personal write credentials
stay host-only and are renewed before push. A private image needs a separate verified
read-only pull grant. Build profiles may omit image and use the reviewed
machine-local destination. A prebuilt profile still requires an explicit image.
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
Saved publisher choices must match current local intent before creating any
record. Retiring an old staging value does not authorize another destination;
explicit legacy recipe images remain compatible.

Desktop preparation now preserves saved publisher choices, and rebuild previews
use the cloud's retained publication and profile. Automatic desktop launches
review the destination before Start. Metadata forwarding alone is not
authorization to publish.

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

### Destination management policy

The shared core destination manager reads metadata without opening the settings
editor or loading provider credentials. It lists choices, selects an existing
registry grant, resets a shown override or explicitly signs in to a derived
personal destination. Every mutation compares the exact shown local override.
Host-owned, bounded operation receipts retain the actor, host, repository and
settings root through polling or cancellation. Cancellation is a request, not
proof that a write did not commit; list saved state after an expired receipt.
The public host adapter below uses this policy.
See docs/testing/procedures/image-destination-management.md.

Direct-root receipts require the exact canonical Git root. Public host adapters
use the distinct launch-directory entry point: the shared worker normalizes the
registered local directory, while receipt ownership retains that exact path.
Cancellation during metadata selection is reported as cancellation, never as a
successful listing with only an inline selection error.

Saved pull metadata compares the registry repository without a tag or digest.
It retains the registry port and exact path; it is not a live access check.

### Image destination selector

New cloud shows Publish images, the selected repository and publishing account.
Automatic uses the image-publishing account, independently of the Git host or
source author. Choose an authorized account or registry before starting. Change selects an existing
authorized registry, uses the profile default or explicitly signs in to another
account. The preference stays on this computer for the canonical checkout and
named profile; repository YAML stays unchanged. UI reads and changes use the same
management policy as host adapters. Cancel sign-in cancels only its owned attempt.
Metadata is refreshed in the background every five seconds. Start can queue an
already shown valid choice during refresh; dispatch waits for fresh matching
metadata. A first read cannot queue an unknown choice. Refresh opens no sign-in.

A personal image needs separate verified read-only worker access before Start.
The setup action targets that exact repository. Settings describe the resolved
repository without attributing a local choice to YAML. Cached sign-in metadata
is not confirmation of the publishing account; worker pull access remains separate.
Opening registry setup stays in settings even when compute and SSH are ready.
Save and return or Cancel goes back to New Cloud for review without starting. Registry edits use the machine's agent defaults and preserve saved bindings; the next Start checks the selected profile's agent credentials again, even if an earlier profile read reported readiness.
Destination metadata is historical,
not proof of authentication; the engine confirms intent and access before a build.
The normal CLI plan runner and public MCP tool use the same policy. Build
profiles can omit image; prebuilt profiles still require it. Publishing choices
stay on this computer rather than in repository YAML.
See docs/testing/procedures/image-destination-selector.md.

A queued Start or stock watch retains the full reviewed profile and publisher.
Transient metadata refresh waits without consuming the request. Changed identity,
profile or destination requires a fresh review; cached metadata never transfers to
another profile or a prebuilt launch. Choice controls remain visible and disabled
while metadata is loading.

The loading indicator keeps the same space before, during and after a refresh.
A stopped stock watch keeps its reason across background refreshes and shows
metadata errors separately. Review the current choice before Start or a new watch.

An interactive sign-in blocks Start until it ends. Its code, link and Cancel
controls stay usable. Only an already shown choice can queue during metadata reads.

Opening the sign-in page keeps the dialog responsive. If the browser cannot
start, copy the code and retry the link; the metadata error stays separate.

A destination save or sign-in blocks new Start and watch intent until fresh
metadata returns after completion. Account changes stay disabled after a failed
first metadata read; use Reload first. Ordinary
metadata reloads can queue the already shown choice; saves cannot queue the old
choice. Storage-only edits keep a pending destination read or sign-in alive,
but a queued Start still requires its complete confirmed profile. An explicit image stays visible during refresh; automatic metadata does not
replace its reviewed target.

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
Existing clouds keep their recorded publisher. cloud_list reports the retained
worker image and publisher metadata when that cloud's matching runtime is loaded.
See docs/testing/procedures/image-destination-public.md.

Management compares the exact shown override inside the save transaction.
A cancelled or stale operation cannot silently replace another preference.
Cancellation does not revoke authorization already completed at the issuer or
prove that an in-flight write was rolled back. List saved state after uncertainty.

Destination inspection, selection, reset and account sign-in use the same profile
prepared by the trusted launch adapter. Selecting a historical revision must not
reload today's profile for a mutation: that profile may be absent or prebuilt now.
The public headless request still resolves its own working-copy context; callers
cannot supply a trusted profile through the wire schema.

## Remote transport development

The independent `remote-session` crate provides non-PTY Rust SSH command
execution with Ed25519 keys, an exact caller-supplied host-key pin and a numeric
`SocketAddr`. It reads no saved credentials and starts no SSH client executable.
Current Horizon cloud connections and visible UI behavior are unchanged. The
crate supplies no public cloud operation or Mosh route. Do not infer RunPod UDP
or history qualification from its library tests. Genuine OpenSSH tests
are opt-in on Unix and use synthetic loopback fixtures; they are separate from
Rust-server tests and the explicit check with no SSH program on PATH. Follow
`docs/architecture/remote-session.md` and
`docs/testing/procedures/remote-session-ssh.md` for setup and package checks.
