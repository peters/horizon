# Worker image destinations

This document defines the image destination boundary for epic #1597.
The destination policy is a reusable module in `horizon-cloud`.
It does not authenticate, send network requests or publish an image.
The shared launch engine resolves and retains destinations. New automatic launches
remain behind a machine-local staging gate, off until the complete selector is delivered.

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
than accepting the cached account. It is not an account-switch UI or per-account
store; the launch and selector integrations still follow separately.
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

A build profile can omit `image`; a prebuilt profile still requires one. The staged
engine consumes these preferences, authenticates the selected account and records
its destination before building. It checks the shown selection again before using
credentials. Existing explicit-image records retain their original image and do not
inherit a later preference. A new automatic launch remains refused while the staging
gate is off. The visible selector, public destination controls and saved-launch bridge
are delivered separately before activation.

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
refuse. This prerequisite also refuses those envelopes until the launch engine
can consume the retained choice; it activates no selector or image publication.
Ordinary unknown legacy fields remain compatible, but mixed or downgraded
publisher envelopes and reserved null keys are refused.

Cloud-launch JSON and YAML are untrusted. A parser must not silently lose a
reviewed publisher and then launch the recipe's original target. The envelope
requires a compatible consumer; this prerequisite fails closed for publisher
choices until engine forwarding is wired in the same change that enables reads.
Recipe confirmations carry no alternative publisher and retain identical image
semantics for old readers. Runtime constructors initialize the new field to None,
so ordinary saved launches remain unchanged. The reviewed choice adds no credential or source-repository URL; it carries
only destination and publisher metadata, not authorization.

Malformed launch bodies deliberately return a generic decode error rather than
repeating rejected field values. Saved state is untrusted and an invalid field
can contain accidentally pasted private text. This trades field-level diagnostics
for a redacted error while retaining compatibility for valid legacy documents.

Recipe/profile equality is enforced before serialization as well as during
deserialization. An inconsistent in-memory recipe emits no standalone launch
JSON, and JSON/YAML serialization fail before it can reach an older reader.

## Staged engine and development CLI

The engine keeps a false-by-default `portable_image_publishing` setting until the
complete user flow is ready. Existing explicit-image launches remain supported.
The development `cloud_deploy` helper accepts a profile without an image only in an
explicitly enabled owned test fixture. Its plan prints the selected repository and
account metadata, includes that choice in the confirmation token, and reads a saved
cloud without creating state. Execution rechecks the choice. A changed preference
cannot redirect a retained cloud; a missing original grant stops before building.

The default-off stage does not enable saved publisher envelopes in the desktop.
Version-2 reads and actual UI request forwarding remain one later atomic change.
Git hosting and registry hosting remain independent; no GitLab sign-in or project
discovery is added. A missing read-only grant for a private personal package stops
before build and names the recorded destination. A publisher's write token never
becomes worker pull access. Legacy deployment and allocation versions remain fenced.

The first and final reviewable candidates require an owned real registry build,
push and immutable read-only check, plus the read-only real-checkout identity gate.
These prove local engine behavior, not a live issuer account or paid allocation.
See [the engine procedure](../testing/procedures/portable-build-engine.md).

A retained publishing record also fixes the rebuild profile name. A request for
another profile is refused even when that profile names the same destination or
omits its image. Recipe edits within the recorded profile still rebuild to the
recorded destination.

This staged engine is supported through the development CLI only. Desktop
creation, saved publisher-envelope restoration and retained-publication rebuild
previews wait for the atomic UI bridge. Keep the setting off for desktop use;
a staged CLI record is not evidence that the desktop path is qualified.

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
Direct native management can normalize a checkout subdirectory. Receipt-backed
operations instead require the host's actual Git root: the worker compares the
prepared root with that owned scope before reading host policy or authenticating.
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
The default-off setting still gates mutations. No public adapter is wired by
this prerequisite; later UI, CLI and MCP controls must use this shared policy.
