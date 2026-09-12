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

## Follow-up implementation

- Add bounded, exact-allocation record/load operations and a frozen version-one
  profile digest representation. First recording must precede key preparation
  and provider dispatch; no late backfill or replacement is permitted.
- Wire configured preview/consent/setup/check to this binding and the shared
  complete Azure deployment-target validator. Reject current-profile mismatch
  before credential acquisition or provider requests.
- Prove no-handle lost-response recovery, profile/subscription drift, competing
  controllers, missing metadata, retained SSH pins and unchanged dirty data.

The digest is a consistency binding, not a signature or provider attestation.
Direct malicious rewriting of the private database is not prevented by SQLite
triggers; readers must validate complete metadata and runtime identity. No token,
credential, registry password, allocation or cloud acceptance is implied here.
