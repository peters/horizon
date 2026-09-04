# ADR-383: Session-owned remote records in the control-plane database

Status: Proposed implementation boundary for issue #383.
Date: 2026-09-05 (Europe/Oslo; 2026-09-04 UTC)
Deciders: Repository maintainer through the issue and reviewed implementation PRs.

## Context

Board snapshots are rebuilt from live panels during normal saves. The local
restore factory starts terminal commands, and session duplication retains most
workspace identities. None of those paths currently understands remote worker
ownership. Remote cleanup records must also survive removal of a workspace or
session until exact provider cleanup completes.

## Decision

Persist the complete validated remote aggregate in the existing private SQLite
control-plane database. Use a globally unique logical workspace identity and an
immutable owning session identity. Every replacement compares the exact stored
revision and snapshot. Keep remote records out of board snapshot replacement and
session-file deletion. Later runtime snapshots reference session-owned records;
they do not copy live provider handles.

The initial storage PR adds schema migration, validated record operations,
bounded session recovery, and ownership/revision regressions. It performs no
provider calls and adds no UI consumer. Generation/workflow allocation will be
one transaction in a subsequent coordinator slice using this same database.
The record store is not permission to create, attach, or delete a worker.

Each snapshot is limited to 4 MiB, with session recovery capped at 512 records
and 64 MiB of serialized snapshots. Reads are bounded before materialization;
creates and replacements enforce the same per-session limits in their write
transaction so accepted writes cannot make recovery exceed its budget.
All record operations run synchronously off the render thread. This storage
boundary exposes no automatic retention or record-deletion API.

Once recorded, the exact cleanup reason and request timestamp remain immutable
while that runtime exists. Later cleanup observations cannot replace the original
intent or reset its age; only verified runtime disposal can retire it.

Domain-valid in-memory state can exceed the storage limits. Callers must persist
intent successfully before provider side effects and surface storage-capacity
errors without discarding the last saved runtime or cleanup record.

## Options considered

### Embed the whole aggregate in runtime YAML

Low initial complexity and no dependency cost; familiar serialization. It makes
session-file inspection easy. However, normal board saves can drop remote data,
copying sessions can duplicate cleanup identity, and ordinary session deletion
can erase unresolved cleanup intent. Multi-controller creation would still need
a separate transactional ownership store.

### Use the existing SQLite control-plane store

Moderate implementation complexity; no new dependencies or hosted-service cost.
Uses existing transaction, privacy, and compatibility policy. Indexed, bounded
recovery fits local workspace counts. Shared transactions can later bind runtime
generations to single-worker workflows. Requires explicit UI references and
eventual cleanup-aware record retirement.

### Separate remote SQLite database

Similar storage cost and familiarity, but introduces a cross-database crash
boundary between generation ownership and the existing workflow creation fence.
No MVP benefit justifies that extra recovery protocol.

## Trade-off and consequences

Choose one durable control-plane database and explicit session ownership. This
delays UI restore until it can safely resolve references, but avoids pretending
that YAML metadata alone makes remote startup or deletion safe. Existing local
panels, SSH restoration, browser/session bindings, and runtime YAML stay unchanged
in the storage-only slice. It does not solve session duplication or UI migration
by itself; those remain explicit acceptance criteria.

## Action items

1. Extract database policy mechanically without changing behavior.
2. Add session-owned aggregate records and exact-revision storage tests.
3. Atomically bind one runtime generation to one single-worker workflow.
4. Add copy-safe runtime references and deferred remote panel restoration.
5. Retire records only through cleanup-aware coordinator transitions.
