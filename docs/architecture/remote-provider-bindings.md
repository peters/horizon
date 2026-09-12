# Retained provider selection

Status: Proposed for #474. Date: 2026-09-12.

## Context

A saved worker target identifies a named profile. Before an Azure resource ID is
retained, recovery cannot determine the original subscription from that name if
the configuration has changed. Reconciliation must not inspect a different
subscription or infer permission to create a replacement.

## Decision

Store a versioned, immutable provider binding beside the exact runtime allocation
in the same private SQLite database. Bind workspace, owner, generation, workflow
and job identities to the explicit subscription and a digest of the complete
approved non-secret profile. Missing metadata is missing provenance, never a
default subscription or an instruction to repair it from current configuration.

Schema seven introduces only the table, collision/update/delete protections and
schema validation. It creates no binding rows, rewrites no runtime snapshots and
does not enable Azure setup. Valid schema-four/five/six stores remain readable
without migration; partial or malformed metadata fails closed. Existing schema
six network-volume metadata remains validated on legacy reads.

## Alternatives and trade-offs

- Re-reading the current profile by name is smaller but loses original placement
  provenance after an ambiguous create, so it is rejected.
- Separate sidecar files avoid a database migration but cannot share the existing
  allocation transaction and corruption checks, so they are rejected.
- A database binding adds a migration and explicitly refuses recovery when the
  original profile is unavailable. It reuses the existing ownership boundary and
  avoids duplicating orchestration or introducing a new provider framework.

## Immutable record/load API

`RemoteCpuProfileBinding` freezes all eight approved CPU-profile fields under an
explicit version-one domain and byte encoding; changing a field requires a
deliberate digest-version decision. The store records it only for an exact,
unclaimed allocation before key reservation or first-pin intent. Exact repeats
and read-only loads remain available after setup expiry or management intent;
neither rewrites snapshots nor consumes a creation grant. Readers validate
bounded row contents and workspace/workflow/job collisions, not only the schema.
The caller must record before preparing a private key; database state alone
cannot prove that a key file does not exist.

## Configured Azure setup

The configured setup preview exposes the complete non-secret Azure profile;
consent binds that exact profile and immutable image before saving. Its Azure
leaf allocates through the existing store, records the immutable binding, then
assembles the lazy CLI credential/provider with that same store's creation fence.
The shared setup coordinator alone prepares/reserves the key and dispatches
ensure. No second allocator, key implementation or creation fence is introduced.

Manual Check requires the original binding and an unchanged complete profile
before key or provider access, including when no worker handle was returned.
Missing provenance remains interrupted, never backfilled. Existing recovery
inspects or reconciles without ensure; it can retain observations and attest a
host key through the provider's fixed guest command. Errors preserve the original
locator and do not authorize another create, task, attachment or cleanup.
Synthetic tests exercise this ordering; no live Azure acceptance is claimed.

## Follow-up implementation

- Expose the configured API through explicit UI confirmation, without enabling
  unsupported attachment, repository or task actions.
- Prove live no-handle recovery, competing controllers, retained SSH pins and
  unchanged dirty data on the exact integrated candidate.

The digest is a consistency binding, not a signature or provider attestation.
Direct malicious rewriting of the private database is not prevented by SQLite
triggers; readers must validate complete metadata and runtime identity. No token,
credential, registry password, allocation or cloud acceptance is implied here.
