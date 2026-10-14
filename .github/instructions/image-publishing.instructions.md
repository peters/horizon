---
applyTo: "crates/horizon-cloud/src/image_publishing.rs,crates/horizon-cloud/src/image_publishing/**,crates/horizon-core/src/cloud_runtime/image_publishing.rs,crates/horizon-core/src/cloud_runtime/image_publishing/**"
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

These limits are deliberate:

- This prerequisite supplies a reusable policy contract. It does not enable automatic launches.
- The policy performs no authentication or network operation and proves no registry permission.
- Host adapters supply a source identity without credentials and check the publisher before each publication.
- Explicitly authorized registry destinations can use another service, a nested project path or a registry port.
- GitLab account connection and project discovery are outside this task.

Do not request the deferred launch or UI integration as fixes to this policy prerequisite.
