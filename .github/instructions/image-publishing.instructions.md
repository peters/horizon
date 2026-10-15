---
applyTo: "crates/horizon-cloud-worker/src/remote.rs,crates/horizon-core/examples/cloud_deploy.rs,crates/horizon-core/examples/cloud_deploy/publishing/**,crates/horizon-core/src/cloud_panel.rs,crates/horizon-core/src/cloud_panel/launch_serde/**,crates/horizon-core/src/cloud_panel/launch_serde.rs,crates/horizon-cloud/src/image_publishing.rs,crates/horizon-cloud/src/image_publishing/**,crates/horizon-core/src/cloud_runtime/github/publish/publication.rs,crates/horizon-core/src/cloud_runtime/github/publish/publication/**,crates/horizon-core/src/cloud_runtime/publishing.rs,crates/horizon-core/src/cloud_runtime/publishing/**,crates/horizon-core/src/cloud_runtime/setup/publishing.rs,crates/horizon-core/src/cloud_runtime/setup/publishing/**,crates/horizon-core/src/cloud_runtime/settings.rs,crates/horizon-core/src/cloud_runtime/registry/**,crates/horizon-browser-control/src/manifest/image_destination.rs,crates/horizon-browser-control/src/manifest/provider_usage.rs,crates/horizon-browser-mcp/src/controller/image_destination.rs,crates/horizon-browser-mcp/src/server.rs,crates/horizon-ui/src/app/cloud_panel/image_destinations/**,crates/horizon-ui/src/app/cloud_panel/image_destinations.rs,crates/horizon-ui/src/app/browser_provider_usage.rs"
---

# Image destination boundary

Read `docs/architecture/image-publishing.md` for the guarantees and limits.
Repository names, profile names and stored selections are untrusted.
Only host account bindings can authorize publication credentials.

Report these failures:

- A source repository selects a publishing account or credential reference.
- An automatic destination uses the source owner's account instead of the host's publishing account.
- A saved destination accepts another publisher when the host checks its identity.
- A registry destination contains credentials, a tag, a digest or an implicit registry host.
- Equal repository names from different source identities or profiles produce the same automatic package name.
- Unbounded or malformed source names escape the destination checks.
- Secret material enters serialized destination metadata.
- A prepared publication accepts a different immutable provider account ID.
- One operation replaces another operation's Docker credentials.
- A temporary publishing login changes the host's shared Docker configuration.

- A local save overwrites another window's account at the same destination URI.
- A selection accepts a changed publication grant while it still names the same repository.
- Package naming sends URL credentials to metadata or output events.
- Pull rotation changes the publication grant identity.
- A preference for one checkout or profile changes another scope.

Retained-account recovery must prompt for the original immutable account when
another account is cached. Check the device result before saving its credential
chain; a wrong account must preserve the prior stored chain. Preparation and
renewal share this guard. A renamed matching account must fail before a futile
device prompt, with a specific rename explanation. Explicit selection must not silently reuse a cached
account. The engine calls preparation and renewal. Visible account selection follows separately.

These limits are deliberate:

- This prerequisite supplies a reusable policy contract. It does not enable automatic launches.
- The policy performs no authentication or network operation and proves no registry permission.
- Host adapters supply a source identity without credentials and check the publisher before each publication.
- Explicitly authorized registry destinations can use another service, a nested project path or a registry port.
- GitLab account connection and project discovery are outside this task.

Do not request the deferred launch or UI integration as fixes to this policy prerequisite.

For CPU host grants, require current verification of the exact immutable digest.
Reject a reread of mutable credential files that substitutes unverified material in user data.
Reject a publishing credential sent to a worker or an allocation after missing verification.

The local preference helpers do not authenticate metadata, change profile parsing or activate automatic launches.

Runtime constructors keep None until the selector is delivered. The version-2
reader and actual UI preparation forward the full choice and profile atomically. Reject a
reader that silently discards reviewed publishing intent. Refuse publisher envelopes
without a build recipe, including serialization before any partial output. Preserve legacy flat
encodings and exact-image recipe confirmations, including JSON/YAML compatibility.
Unknown ordinary legacy keys stay compatible; reserved envelope keys may not hide
inside a flat body, even when null. No publication is activated by this prerequisite.

Recipe/profile equality is enforced before serialization as well as during
deserialization. An inconsistent in-memory recipe emits no standalone launch
JSON, and JSON/YAML serialization fail before it can reach an older reader.

Host-only pull preparation must not read a compute API key or create provider registry IDs.
Existing provider journals retain their original path and account identity. Direct-host and
provider journals have separate operation locks, but the first material pin is one
generation-wide transaction. Report in-place pull changes accepted through another scope,
provider mutation state accepted in a host journal, or publishing secrets sent to a worker.
Initial-build and rebuild callers choose the helper from their actual provider kind. Local checks cannot revoke a credential already installed on a host.

Check the actual host journal directory against the canonical source, not only its
parent registry root. Refuse source aliases before creating any journal. Missing
private ancestors sync a locked pending intent and its parent directory entry before creation and
must be durable before their children or generation pins. Existing ancestors with
an unfinished intent must wait or finish the parent sync before proceeding.
Coordination files retain their inode, resolve parent aliases, contain no secrets
and refuse symlinks, wrong owners, broad permissions and extra hard links.
Existing roots with no pending intent must not require a readable parent.

The staged engine records new destinations before building, authenticates the
retained account or exact publication grant on each new build, and renews personal
operation credentials immediately before push. Reusing a worker image requires no
publisher authentication. Report a retry/rebuild that adopts a new host preference,
loads another publishing grant, or accepts a confirmation token for another selection.
New automatic launches stay behind the default-off stage. Saved envelope reads,
actual UI preparation forwarding and record-aware rebuild previews form one bridge.
Reject a preview that drops the retained publisher or accepts another profile name.
Do not activate the selector or update development image pins in this bridge.

Registry credential isolation must preserve the selected Unix Docker context and
plugin locations without copying another registry login, a credential helper or
proxy credentials. Resolve relative machine configuration paths before linking
contexts or plugins; an empty `DOCKER_CONFIG` uses the home fallback. Apply the same
safe skeleton during preflight login. Keep explicit daemon-host precedence. Temporary config creation
must not read a stored personal publication chain to prepare a registry-bound login.

Host-only management must not load a compute key or create provider registry IDs.
Keep host pull checks and provider reconciliation/revocation explicit in every
interface. Metadata-only status must not load auth files or claim current issuer
permission. Provider revocation must not claim that a host or issuer token was revoked.

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

Actions on previous registry generations must not clear or restore the current
generation's readiness hint. Apply the same current-generation check before
starting the action and before accepting its result.

Settings verification must retain the selected saved binding identity independently
of the editable image. Malformed images and images from another repository must
clear that binding's hint without validating another one. A retired request must
not alter current proof. Refusals survive settings closure and board-session
switches until fresh successful verification, within the current app process.

The staged destination manager and receipt owner are a shared host API, not yet
public controls. Reject caller-supplied actor/host/root authority, cross-owner
receipt access, ID reuse for another request, unbounded work/history, registry-secret
or compute-key reads during listing, or a mutation that skips the exact shown-override
fence. Listing may inspect cached sign-in metadata for identity, but must not return
tokens, authenticate or resolve registry credentials.
Cancellation does not prove rollback; expired receipts require listing saved state.

A destination receipt ID must never become a cloud sign-in Skip key. Authentication
attempts use independent private keys, and host-owned cancellation remains scoped
to the receipt rather than a caller-selected cloud identifier.

Receipt-backed destination operations require the host's actual canonical Git root.
Refuse a receipt bound to a subdirectory before host policy or authentication;
resolve agent working directories to the root in host adapters. Preserve direct
native helper normalization of checkout subdirectories. A cancelled metadata selection must
remain cancellation, not successful Info with only a selection_error.

Public requests cannot supply source or settings authority. The host binds a typed
LaunchOwner to the live local panel registered directory. Shared worker preparation
resolves its root off the render thread, including subdirectories. Keep direct-root
receipt fencing intact. Status and cancel retain the original actor, host, path and
root. Poll active operations without user input so observation expiry and lost
scope request cancellation. Remote/SSH panels and expired requests refuse.
Keep CLI plan and stdio MCP behavior, both bundled guides and coverage in sync.

The desktop destination adapter refuses any agent panel owned by a cloud group,
including worker and Upkeep panels whose local metadata has no SSH connection.
Admission and retained-operation polling must check both saved board membership
and live cloud groups. Cover the session-restore interval before live membership
is copied. A local launch directory alone does not establish desktop ownership.

Unstarted legacy launches without a publisher choice retain explicit recipe intent.
Automatic profiles keep automatic intent, not an empty recipe confirmation.
Reject silently adopting a new local override before their first durable record;
an existing record must keep its own publisher when the saved choice is absent.

Before a new desktop deployment record, confirm the saved choice through shared
selection policy. Cover a removed override as well as a replacement, while keeping
automatic intent without an override valid. Existing records remain authoritative.

Before a first record, cover a saved non-recipe choice while the setting is off,
even with an explicit recipe image. Refuse before persistence instead of creating
a legacy record without that publisher. A legacy automatic launch with no shown
choice must reject a scoped override while still accepting no-override automatic
intent. Existing durable records remain authoritative.

Also cover a retry whose existing automatic record still has no publication, such
as after missing sign-in. That record has no publisher to retain and must not
adopt a later scoped override without a reviewed choice. Already bound records
retain their original publisher regardless of current preferences.
