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
- Initial image or rebuild preparation bypasses the retained account or publication grant.
- Publisher authentication blocks reconnect, resize or another operation that only reuses an existing image.
- Deployment format 5 or paired allocation format 3 drops publisher or enrollment evidence.
- A downgraded version or a version missing its required evidence is accepted.
- Migration bypasses a neighboring duplicate-worker fence.

The engine authenticates and compares the retained identity before each new build.
A default-off gate keeps new automatic launches inactive until the selector and
interface controls are complete. Publisher envelopes remain fenced until UI forwarding
is integrated atomically. Do not request activation as a fix to this API prerequisite.

Rebuild plans and preparation must reject a profile name different from the retained
publication profile, including another profile with the same image or no image.

Rebuild tokens must distinguish different retained publishers or grants with the
same resolved image, profile and files. The locked deployment must match its
confirmed publishing identity before credentials load or a replacement is journaled.

Image-only retries must restore the saved working-copy confirmation as well as the
profile before overlay validation and image-label calculation. A historical CLI
request with no working-copy field must not discard the locked record's confirmation.

Reconnect and resize normalize an omitted image or the original recipe image to
the retained destination, even after a rebuild changes the recipe from explicit
to automatic. A different explicit image must still refuse before credentials.
