---
applyTo: "crates/horizon-core/src/cloud_panel.rs,crates/horizon-core/src/cloud_panel/launch_serde/**,crates/horizon-core/src/cloud_panel/launch_serde.rs,crates/horizon-cloud/src/image_publishing.rs,crates/horizon-cloud/src/image_publishing/**,crates/horizon-core/src/cloud_runtime/github/publish/publication.rs,crates/horizon-core/src/cloud_runtime/github/publish/publication/**,crates/horizon-core/src/cloud_runtime/publishing.rs,crates/horizon-core/src/cloud_runtime/publishing/**,crates/horizon-core/src/cloud_runtime/setup/publishing.rs,crates/horizon-core/src/cloud_runtime/setup/publishing/**,crates/horizon-core/src/cloud_runtime/settings.rs"
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
account. This prerequisite does not wire these helpers into launches or UI.

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

The launch-format prerequisite keeps every runtime constructor at None and refuses
publisher envelopes until launch forwards their choice to the engine. Reject a
reader that silently discards reviewed publishing intent. Preserve legacy flat
encodings and exact-image recipe confirmations, including JSON/YAML compatibility.
Unknown ordinary legacy keys stay compatible; reserved envelope keys may not hide
inside a flat body, even when null. No publication is activated by this prerequisite.

Recipe/profile equality is enforced before serialization as well as during
deserialization. An inconsistent in-memory recipe emits no standalone launch
JSON, and JSON/YAML serialization fail before it can reach an older reader.
