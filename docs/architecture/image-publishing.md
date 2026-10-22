# Worker image destinations

This document defines the image destination boundary for epic #1597.
The destination policy is a reusable module in `horizon-cloud`.
It does not authenticate, send network requests or publish an image.
The shared launch engine resolves and retains destinations. New cloud and public
CLI/MCP controls review the selected publisher before ordinary automatic launches.

## Host publication preparation

The host has an operation preparation helper in `github::publish::publication`.
It authenticates the publishing account and returns non-secret account metadata with a private Docker configuration.
The account metadata contains the provider's immutable user ID and its login name.
The helper rejects another user ID, including a reused login. Letter case does not change the account.
An account rename is refused because it could change the selected namespace.
A cached matching account ID with a changed name fails before prompting again;
signing in cannot recover the recorded namespace. A new cloud needs a newly
reviewed destination.

The operation owns its temporary Docker configuration until it finishes.
It writes only its authenticated publishing login there, not into the host's shared Docker configuration.
A later sign-in or another publication cannot replace that operation's credentials.
The temporary configuration is removed when the operation is dropped.
On Unix, the directory is created with mode 0700 and its login file uses mode 0600.
Cancellation is checked before local work, after the token lookup and before the result is returned.
Credentials are neither printable nor serializable through this helper.
Expected account metadata is untrusted and must pass validation before the login is prepared.
The helper can renew the same account's operation login before a push. A long build must
call this renewal checkpoint; preparation alone does not guarantee token validity through
a later push. When the cached host account differs from a retained publisher, preparation and
renewal ask for a new device sign-in for the recorded immutable identity. The
helper checks that identity before saving the returned credential chain. A wrong
account leaves the prior stored chain and private operation login unchanged.
A separate explicit account-selection helper always starts device sign-in rather
than accepting the cached account. The native selector and public destination
controls call this helper for an explicit account switch. It is not a per-account
credential store.
The engine uses this helper before a new personal build and renews its private login
immediately before push. Registry-bound publication uses its exact saved grant instead.

## Retained publication records and preparation

A cloud record retains its original recipe image, named profile, resolved image
repository and nonsecret publisher identity. The record contains no credential and
is not proof of current registry permission. A new API launch records its authorized
local choice before image work. A retry or rebuild keeps that destination and compares
its immutable account or publication grant again before loading image credentials.
An existing image specification requires no publishing login. Reconnect and resize
retain their existing image and do not start a publication.

Store reads and writes validate the record against the resolved build profile.
Personal destinations require matching immutable account metadata and namespace;
registry destinations cannot carry a personal account. An inconsistent record
cannot replace the previous saved cloud. Host authorization must still compare
the recorded identity with the authenticated account on every publication attempt.

A publication record uses deployment format 5 and paired allocation format 3.
The versions fence older readers that could otherwise erase the publisher. Removing
the record without changing its version, or lowering the version while keeping it,
is refused. Existing deployment formats and enrollment evidence remain supported.
Legacy migration preserves publication metadata and the duplicate-worker guard,
including when a neighboring cloud already has a publication record.


Reconnect and resize accept an omitted image, the original recipe image or the
resolved image after a rebuild. All three refer to the same retained destination;
none selects a new publishing account. A different explicit image or named profile
is refused before credentials load, including after an explicit recipe becomes automatic.

## Machine-local destination preferences

The host can store one destination preference for each canonical checkout and named profile.
The preference stays in machine-local cloud settings. It does not enter repository YAML.
A personal preference contains the immutable account ID, account name and its own GHCR namespace.
A registry preference retains the exact configured publication grant identity across reloads.
The grant identity contains a hash of its username and immutable credential-file reference.
Pull-credential rotation does not change the publication grant identity.
Configured preferences use metadata only and never serialize token values.
An automatic preview uses the existing private sign-in chain reader to get its cached account name.
This cached name does not prove authentication. The caller runs this read on a worker thread.
These metadata helpers do not authenticate an account or prove registry permission.

The source adapter reads the origin, including included local configuration files, with bounded, cancellable Git commands on a worker thread.
It removes URL credentials before package naming and sends no remote text to output events.
Without a supported remote, it uses the canonical local checkout path.
Malformed or unreadable Git configuration stops selection; it is not treated as an absent origin.
The source transport, host, complete path and explicit port distinguish source identities.
Origin normalization removes at most one final `.git` suffix and does not interpret browser routes.
Relative local origins use the canonical checkout path. Non-UTF-8 fallback paths are refused.
Home-relative SCP and SSH URI origins retain an opaque SSH-user scope; an implicit user uses the local fallback.
SSH URI paths and usernames decode percent escapes before home-scope selection. Named homes
retain the named user; an absolute SCP path remains absolute.
Automatic naming refuses query parameters, fragments and multiple origin URLs.
The source text cannot choose the publishing account.

A save requires an existing trusted cloud root outside the source repository.
It holds the shared transaction lock and reads the current settings again.
If the settings file does not exist, the save starts from the standard machine defaults.
It compares the complete previous preference, including the immutable account ID.
It also checks that the selected registry publication grant still matches its shown identity.
A stale window cannot replace another window's choice, including another account at the same URI.
The save preserves unrelated settings and preferences. Cancellation prevents the commit.
The save refuses settings inside the canonical source repository before it creates a lock.
Malformed scopes and duplicate checkout/profile pairs are rejected when settings load.
A removed registry grant does not prevent unrelated clouds from loading settings.
Selection of that removed grant fails when the host tries to use it.

A build profile can omit `image`; a prebuilt profile still requires one. The
engine consumes these preferences, authenticates the selected account and records
its destination before building. It checks the shown selection again before using
credentials. Existing explicit-image records retain their original image and do not
inherit a later preference. New cloud and the public destination controls use the
same machine-local policy. An absent or unauthorized destination refuses before
the expensive build and offers the specific account or registry setup action.

## Destination identity

A destination contains an explicit registry repository and a non-secret publisher identity.
An automatic GHCR destination uses the host's image-publishing account.
The source repository cannot select that account.
Personal destinations retain a lowercase account name and that account's own namespace.
An observed login with different letter case still names the same account.
A machine-local registry destination names the exact credential reference that permits publication.
Organization destinations use a machine-local registry binding.
Enterprise Managed Users require an organization destination because GitHub does not provide personal package storage for those accounts.
See [the GitHub registry documentation](https://docs.github.com/en/enterprise-cloud@latest/packages/working-with-a-github-packages-registry/working-with-the-container-registry).

The package name contains the repository name, profile name and a short hash of the source identity.
The hash separates equal names from different source hosts and owners.
Normalization cannot turn repository text into a registry address or an account selection.
The source identity must not contain credentials. Host adapters remove credentials before they call the policy.

## Trust boundary

Repository names, profile names and persisted records are untrusted input.
The policy rejects malformed accounts, credential references and registry repositories.
Destinations omit tags, digests and credential material.
The policy describes a selection. It does not prove that the account has permission to publish.
The host must check permission and retain the selected account before a build.

Hosts must check the recorded publisher before each publication attempt.
A reconnect, retry or account change must not select another destination.
Repository configuration cannot name credential references or change an existing cloud's publisher.
Workers receive only their authorized pull access. Publication credentials stay on the host.

## Registry independence

A bound destination can use a different registry from the source Git service.
The contract accepts nested project paths and explicit registry ports.
GitLab account connection and project discovery are future work.
The current task does not add those operations.

## Bound CPU worker pulls

An exact Container registry binding takes precedence over the legacy CPU-provider login.
Before allocation, the read-only grant must verify the worker's immutable image digest.
The provider adapter retains that image identity and the verified material in memory.
A local credential-file change cannot substitute another login in host user data.
Missing verification or a different immutable digest stops allocation before provider contact.
Unbound images retain the existing legacy-provider behavior.
Temporary registry Docker directories are private at creation on Unix.
These checks prove the immutable image read at that moment; they cannot guarantee future issuer availability.

The provider-aware preparation helper stores direct-host pull records under the private registry root's `hosts` directory.
It does not read a RunPod key or create provider registry IDs. Existing RunPod records keep their original path and account binding.
Both the registry root and its actual host journal directory are checked against the canonical source before credential preparation or journal creation.
This includes an existing `hosts` checkout and a host directory alias into the source.
Private directory creation uses a stable per-user coordination file in the parent directory.
It holds that file's lock and syncs the pending bit and its parent directory entry before creating a directory. It syncs the
new directory and its parent before clearing the bit or allowing a child or generation pin.
An existing ancestor with a pending intent waits for the creator or completes the parent sync
after an interruption. Ancestor aliases resolve before the shared coordination file is selected.
The files stay in place to preserve lock identity; they contain only a zero- or one-byte intent,
not credentials. No-follow opens, ownership, private permissions and link-count checks protect them.
Existing roots with no pending intent do not require read permission on their parent.
The two journals retain separate operation locks, but one generation-wide first-use transaction pins their shared pull identity.
A changed pull username or secret requires a new generation, including when the other journal was used first.
Malformed records and a direct-host record with provider mutation state are refused.

The shared initial-build and rebuild paths choose the provider-aware helper.
Direct hosts use the host journal; provider-managed registry access retains its original journal.
Local validation does not revoke a pull credential already installed on a host.

The verified CPU pull material retains the earliest configured or issuer-observed expiry.
An elapsed known expiry stops allocation before provider contact.
This checkpoint does not prove that an issuer will keep an unexpired grant valid through the host pull.

Preference saves validate the complete reloaded settings before looking up or replacing
a scope. Duplicate checkout/profile entries are refused without discarding either choice.

## Saved launch confirmation boundary

Saved cloud launch confirmations retain their image intent in a conditional wire
format. Existing launches with no reviewed choice keep their flat encoding.
An explicit recipe-image confirmation also stays flat and must match the exact
profile image. Publisher choices use a version-2 envelope that older readers
refuse. Compatible readers retain the full choice, and desktop preparation forwards
that choice and its named profile to the shared engine. New cloud supplies the reviewed destination and publishing identity.
Build profiles may omit the image; explicit images remain supported.
Ordinary unknown legacy fields remain compatible, but mixed or downgraded
publisher envelopes and reserved null keys are refused. A publisher choice without
a build recipe is invalid and cannot be saved or restored as a prebuilt launch.

Cloud-launch JSON and YAML are untrusted. A parser must not silently lose a
reviewed publisher and then launch the recipe's original target. The envelope
requires a compatible consumer; reads and actual UI request forwarding are wired
atomically. The engine rechecks the shown choice against machine-local policy.
Recipe confirmations carry no alternative publisher and retain identical image
semantics for old readers. New cloud forwards the selector's confirmed choice;
other runtime constructors keep None and existing records remain authoritative. The reviewed choice adds no credential or source-repository URL; it carries
only destination and publisher metadata, not authorization.

Malformed launch bodies deliberately return a generic decode error rather than
repeating rejected field values. Saved state is untrusted and an invalid field
can contain accidentally pasted private text. This trades field-level diagnostics
for a redacted error while retaining compatibility for valid legacy documents.

Recipe/profile equality is enforced before serialization as well as during
deserialization. An inconsistent in-memory recipe emits no standalone launch
JSON, and JSON/YAML serialization fail before it can reach an older reader.

An unstarted legacy launch without a publisher choice forwards its explicit recipe
image as its intent until a durable deployment exists. An image-omitting build keeps
automatic intent; it must not forward an empty recipe confirmation. A changed local override cannot
silently replace that image or create a record for the replacement. Existing
records remain authoritative; a missing launch choice does not replace their
retained publishing identity with a recipe confirmation.
A legacy automatic launch with no saved confirmation must still be automatic under
shared selection policy: a scoped registry or personal override requires a new
review before any record. An automatic record without a publisher remains
unbound: retry requires the same review of a later override and preserves the
existing record on refusal. Retiring a saved staging value does not authorize a
stale non-recipe choice: it must still match current local intent before a first
record. Explicit recipe images remain compatible.

## Portable engine and development CLI

Ordinary machine settings enable portable publishing. Build profiles may omit
image; explicit-image and prebuilt launches remain supported. The development
`cloud_deploy` helper prints the selected repository and account in its plan,
includes that choice in its confirmation token, and reads a retained cloud
without creating state. Execution rechecks the choice. A changed preference
cannot redirect a retained cloud; a missing original grant stops before building.

Saved `portable_image_publishing` values are a retired staging detail. Missing,
false and true all load as enabled. Reading settings never rewrites their bytes;
the next ordinary save omits the obsolete key while retaining all credential
references. Invalid non-boolean legacy values still refuse parsing. The default
is centralized in Settings and used by setup drafts. Explicit lower-level
in-process legacy requests may still construct the conservative disabled path;
ordinary persisted settings cannot leave the completed user flow hidden.

Version-2 launch reads, UI preparation and retained-publication rebuild previews
share the same metadata contract. Every Start entry point reviews the current
publisher, including Enter, stock watches and collected creation jobs. Git hosting
and image hosting remain independent; this adds no GitLab sign-in or project
discovery. A private personal image needs a separate verified read-only pull
grant before building. A publisher's write token never becomes worker pull access.
Legacy deployment and allocation versions remain fenced.

A retained publishing record also fixes the rebuild profile name. A request for
another profile is refused even when that profile names the same destination or
omits its image. Recipe edits within the recorded profile still rebuild to the
recorded destination.

A rebuild confirmation also snapshots the exact retained publication. Its token
covers the publisher account or grant even when the resolved image and files are
identical. The locked execution compares that identity before reading credentials;
a changed or missing publisher confirmation requires a fresh plan. Legacy plans
without a retained publication keep their existing token format. The development
CLI displays the recorded destination and account with the rebuild plan. A retained
publisher requires that printed token even when the working copy is clean.

Image-only retries also restore the locked record's working-copy confirmation.
Advancing HEAD does not turn an older-revision retry into a commit-only build:
the overlay and image label retain the original revision and files hash. Later
unconfirmed file edits refuse before building.

The first and final reviewable candidates require an owned real registry build,
push and immutable read-only check, plus the read-only real-checkout identity and
setup gates. These prove local engine behavior, not a live issuer account or paid
allocation. See [the final integration procedure](../testing/procedures/portable-image-publishing.md).

## Docker context and registry isolation

On Unix, private registry configurations preserve the machine's selected daemon
context and CLI plugin locations through the same safe builder as personal publication.
They contain only their selected registry login. Other registry authentication,
credential helpers and proxy settings stay out. A private configuration must not
silently switch rootless or remote Docker to the default socket. Explicit host
settings keep their existing precedence. The machine configuration stays unchanged.

## Host-only registry management

Machine-local bindings have two independent management scopes. `verify_pull` checks
the immutable image using the saved read-only grant and a host journal; `pull_status`
reads that journal without loading any credential. Both return `scope: host` and
create no provider access. Legacy verify/status/reconcile/revoke remain provider
operations. Their credential/account fences and resource IDs are unchanged.

The settings card, CLI and stdio registry tool share this dispatch. The settings
card puts provider lifecycle controls in a separate section. Its cached pull hint
can retain a host check when provider access is revoked. A failed host-journal read
does not silently fall back to another scope's successful read. A cached validation
is history, not a current issuer-permission check; deployment verifies its exact
immutable digest and current material again. Host checks do not revoke credentials
already installed on a worker. No new operation allocates compute or publishes.

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

If a host journal is busy or unreadable, aggregate saved history stays unknown
and does not fall back to a provider success. This can temporarily withhold the
settings readiness hint; the action's own scoped status remains visible. Fresh
verification results bypass this historical aggregate lookup.

Saved host pull history never opens the provider journal or reads its compute key,
even when a newer provider validation exists. Provider controls keep their own scoped
status message. A fresh verification displays its own result.

An explicit provider action can retain its own active provider proof when host
history was read successfully and is empty. A busy or unreadable host journal
never permits that fallback. Reopening settings reads host history only; it does
not load compute credentials to recover provider history.

## Shared destination management boundary

The shared manager accepts a local repository and named working-copy profile.
Direct native management can normalize a checkout subdirectory. Direct-root receipt
operations require the host's actual Git root: the worker compares the prepared
root with that scope before reading host policy or authenticating. Public host
adapters instead use a typed `LaunchOwner` from the live panel's registered start
directory. The shared worker resolves that directory to its Git root before
policy. This accepts subdirectories without filesystem work on the render thread.
Canonical aliases pass that root check, while receipt identity remains the exact
host-supplied owner. Cancelled metadata reads propagate cancellation, including a
lookup that finishes successfully after its cancellation was requested.
Repository YAML and origin metadata are untrusted and cannot choose credentials
or an authentication endpoint. Listing reads settings and the cached sign-in store to obtain nonsecret
account identity; it never returns stored tokens and does not authenticate, read registry secrets, allocate compute
or mutate a preference. Selection accepts only an already configured exact
registry grant. Personal targets are derived by the explicit authentication path,
which rejects ambiguous origins and unsafe credential storage before sign-in.

The trusted host supplies operation ownership (actor, host, settings root and
repository), never the caller's payload. Receipts are in-memory and bounded by
count, lifetime and retention. IDs cannot be reused for another owner or request. Each authentication attempt
uses a separate private Skip key outside the valid cloud-ID namespace; a caller
receipt ID cannot clear or consume a cloud card's Skip request.
Scope disappearance, lack of observation and lifetime limits request cancellation.
A receipt is not durable proof of whether a write committed; settings mutations
use the existing compare-and-swap fence, and callers list state after expiry.
The public host adapter, CLI plan runner and stdio MCP tool use this shared policy.

## Public destination operations

Use `cloud_image_destination` from a live local agent panel. It uses the panel's
registered launch directory and resolves its checkout in the shared background
worker, including subdirectories. The host supplies the settings root and source;
never pass a root, source path, token or credential value. Remote worker and SSH
panels cannot change desktop publishing choices. No Settings window is required.

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

The desktop destination adapter refuses any agent panel owned by a cloud group,
including worker panels whose local metadata has no SSH connection.
Both admission and retained-operation polling check saved board membership as
well as live cloud groups: session restoration can expose the saved membership
before live cloud state is copied. A local launch directory alone does not
establish desktop ownership.

## Image destination selector

New cloud shows Publish images, the selected repository and publishing account.
Automatic uses the image-publishing account, independently of the Git host or
source author. Choose an authorized account or registry before starting. Change selects an existing
authorized registry, uses the profile default or explicitly signs in to another
account. The preference stays on this computer for the canonical checkout and
named profile; repository YAML stays unchanged. UI reads and changes use the same
management policy as host adapters. Cancel sign-in cancels only its owned attempt.
A bounded five-second metadata refresh runs off the render thread. Start can queue
an already shown valid choice while metadata loads. First reads cannot queue an
unknown choice. Dispatch waits for fresh matching metadata from shared policy.
The engine still compares the reviewed choice at first preparation.

A personal image needs separate verified read-only worker access before Start.
The setup action targets that exact repository. Settings describe the resolved
repository without attributing a local choice to YAML. Cached sign-in metadata
is not confirmation of the publishing account; worker pull access remains separate.
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

An owned sign-in blocks queueing from its start, before its prompt arrives.
Copy, Open and Cancel must remain usable until the sign-in ends. Background
metadata reads can retain shown intent; interactive authentication cannot.

A destination save or sign-in blocks new Start and watch intent until fresh
metadata returns after completion. Account changes stay disabled after a failed
first metadata read; use Reload first. Ordinary
metadata reloads can queue the already shown choice; saves cannot queue the old
choice. An explicit image stays visible during refresh; automatic metadata does not
replace its reviewed target.

Storage edits do not affect destination-management inputs. Preserve the owned
read or sign-in and its prompt across those edits, without launching another
metadata thread. Confirmation still snapshots the complete profile, including
storage, so an earlier queued Start or watch cannot adopt the changed sizing.
All other profile fields and source/owner identity remain part of operation scope.

Destination inspection, selection, reset and account sign-in use the same profile
prepared by the trusted launch adapter. Selecting a historical revision must not
reload today's profile for a mutation: that profile may be absent or prebuilt now.
The public headless request still resolves its own working-copy context; callers
cannot supply a trusted profile through the wire schema.
