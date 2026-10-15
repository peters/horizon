# Worker image destinations

This document defines the image destination boundary for epic #1597.
The destination policy is a reusable module in `horizon-cloud`.
It does not authenticate, send network requests or publish an image.
The launch pipeline does not use automatic destinations yet.

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
This helper has no launch caller yet. The launch integration follows separately.

## Retained publication record prerequisite

A cloud record can retain its original recipe image, named profile, resolved image
repository and nonsecret publisher identity. The record contains no credential and
is not proof that a publisher is authenticated or permitted to push. Existing
launch callers do not create this record yet; automatic publishing remains inactive.
The two legacy image-preparation factories refuse a retained publisher before loading
old publishing credentials. Initial preparation is fenced only when no image specification
exists; rebuild preparation is fenced when it will build. Reconnect, resize and other
operations that reuse an existing image are not subjected to a publishing-credential fence.
The portable engine must replace these two fences with retained-account authorization
before it can build or rebuild these images. Reading and migration preserve metadata.

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

These helpers have no launch or UI caller yet. They do not change profile parsing or enable image omission.
The launch integration must authenticate the account and retain the destination before a build.
It must compare the shown selection again before it uses credentials or paid resources.

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

This is a preparation prerequisite. Active deployment callers still use the existing provider journal;
the portable launch integration must select the provider-aware helper before activation.
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
