---
applyTo: "crates/horizon-core/src/cloud_runtime/deployment.rs,crates/horizon-core/src/cloud_runtime/deployment/**,crates/horizon-core/src/cloud_runtime/state.rs,crates/horizon-core/src/cloud_runtime/state/**,crates/horizon-core/src/cloud_runtime/allocation/legacy.rs,crates/horizon-core/src/cloud_runtime/allocation/legacy/**,crates/horizon-core/src/cloud_runtime/publishing/publication.rs"
---

# Retained publisher persistence boundary

Read the retained publication record section in `docs/architecture/image-publishing.md`.
Records are untrusted metadata, not authentication or proof of registry permission.

Report these failures:

- A resolved image repository or named profile differs from its retained record.
- A registry publisher carries a personal account.
- A personal publisher lacks its matching nonsecret account metadata and namespace.
- Invalid metadata replaces the previous saved record.
- A legacy image-preparation factory uses a retained publisher through the older
  credential path when an initial image or rebuild will be built.
- The image-build compatibility fence blocks reconnect, resize or another operation
  that only reuses an existing image.
- Deployment format 5 or paired allocation format 3 drops publisher or enrollment evidence.
- A downgraded version or a version missing its required evidence is accepted.
- Migration bypasses a neighboring duplicate-worker fence.

The record prerequisite does not activate launch, image omission or a UI flow.
Host publication authorization must still authenticate and compare the retained identity.
Do not request the deferred engine or selector as a fix to this persistence prerequisite.
