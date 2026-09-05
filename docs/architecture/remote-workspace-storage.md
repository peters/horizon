# ADR-383: Session-owned remote records in the control-plane database

Status: Accepted record-storage boundary; persistent-lifetime integration pending.
Date: 2026-09-05 (Europe/Oslo; 2026-09-04 UTC)
Deciders: Repository maintainer through the issue and reviewed implementation PRs.

## Context

Board snapshots are rebuilt from live panels during normal saves. The local
restore factory starts terminal commands, and session duplication retains most
workspace identities. None of those paths currently understands remote worker
ownership. Remote cleanup records must also survive removal of a workspace or
session until exact provider cleanup completes.

## Decision

### Client references are not worker lifetime

The corrected product contract in [issue #383](https://github.com/peters/horizon/issues/383)
requires remote development to keep running while Horizon is closed or the PC is
off. Closing the final local panel, removing a local session/reference, or losing
a client connection must not create stop/delete intent. Reopening reconnects to
the existing worker and sessions; it does not allocate a new generation merely
because the client restarted.
This is the required integration contract, not behavior implemented by the
record-storage API alone.

The owning local session below scopes client records and prevents accidental
copy/adoption. It is not a compute lifetime lease. Remote task supervision,
repository durability, and checkpointing must not rely on that PC's database or
event loop remaining available. A dedicated Remote Environments overview will
expose reconnect and explicit stop/kill or manual cleanup/delete actions.

The image permits an unset expiry for persistent execution and retains a bounded
watchdog only for explicitly time-limited jobs. Provider-neutral targets and
worker handles distinguish persistent execution from explicit time limits; a
worker observation must match its target's lifetime policy before attachment.
The local container adapter supports both explicit policies, rejects expiry
metadata on a persistent worker, and preserves the same worker across controller
drop/reopen. Inspection never creates a missing worker, and finding a stopped
worker does not restart or replace it. If a newly created persistent worker
has conflicting image-supplied expiry metadata, its exact identity is returned
for manual inspection; metadata drift never authorizes automatic deletion.
After an ambiguous persistent create response, a recovered worker is retained
if inspection fails: another controller may already be using it. The returned
exact identity permits reconciliation, not an unverified cleanup or replacement.
The remote GPU adapter still rejects
unsupported persistent requests/handles before I/O; product profiles and the
remaining remote implementations still need persistent execution support.
Neither record storage nor these contracts alone complete persistent cloud workspaces.

The product policy defaults to persistent, but saved records never infer that
policy from missing or malformed data. Persistent targets contain the explicit
`"lifetime":"persistent"` marker; legacy timed targets retain `lease_seconds`
and their exact v1 wire representation. Observed worker lifetimes keep the v1
`lease` field name: a timed observation contains `terminate_after`, while a
persistent observation contains only `"lifetime":"persistent"`. Missing, null,
conflicting, and unknown policies fail closed. Older clients reject the new
marker instead of silently treating a persistent environment as temporary.
Existing time-limited jobs are never converted into unbounded compute.

### Durable local identity records

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

Generic creation accepts only dormant records, and generic replacement cannot
introduce a runtime into a dormant record, including a previously retired legacy
record. New runtime identity requires the dedicated atomic allocator. Its private
prepared replacement may stage identity inside the caller's immediate transaction;
the allocator must commit the matching workflow and ownership binding with it.
There is no public import shortcut for active runtimes. Already-persisted legacy
snapshots remain readable and updateable with their original identity.
Generic replacements cannot advance the specification's generation; that is also
allocator-owned, and the shared replacement validation retains its allocation path.

Once an existing generation leaves provisioning, a replacement cannot move it
back to provisioning, including from reconciling or failed with no observed
worker. This rule also applies to legacy snapshots and survives reopening the
store. Reconciliation and attachment may still move between non-creating phases;
ordering among them remains coordinator policy. Local panel intent remains
independent of the runtime's lifetime.

Each snapshot is limited to 4 MiB, with session recovery capped at 512 records
and 64 MiB of serialized snapshots. Reads are bounded before materialization;
creates and replacements enforce the same per-session limits in their write
transaction so accepted writes cannot make recovery exceed its budget.
All record operations run synchronously off the render thread. This storage
boundary exposes no automatic retention or record-deletion API.

Once recorded, the exact cleanup reason and request timestamp remain immutable
while that runtime exists. Later cleanup observations cannot replace the original
intent or reset its age; only verified runtime disposal can retire it.
The first trusted SSH endpoint may be recorded once while the runtime exists. Once pinned,
its host, port, user, and host key cannot be replaced or dropped within that
runtime generation, even while reconnecting or reconciling.

Domain-valid in-memory state can exceed the storage limits. Callers must persist
intent successfully before provider side effects and surface storage-capacity
errors without discarding the last saved runtime or cleanup record.

### Reserved allocation identity schema

Schema 3 reserves `remote_runtime_allocations`: one primary workspace identity,
unique workflow/job indexes, and foreign keys to the workspace and workflow
snapshots. Migration from schema 2 preserves the existing snapshots, revisions,
and consumed creation claims without manufacturing allocation bindings from
legacy runtime references. The new table starts empty.

Older clients reject the newer database version. Missing or partial allocation
tables/indexes and ambiguous interrupted-version fixtures are rejected rather
than silently repaired or adopted. Before migration commit and on every operation,
validate the owned table and index definitions against their exact, versioned
`sqlite_schema.sql` representation. This includes uniqueness, indexed columns,
the primary key, positive generation, foreign keys and strict typing; equivalent
but unexpected definitions are not adopted. Keep these CREATE definitions stable
until an explicit schema migration replaces them, including parent-table renames.
A frozen format fixture prevents accidental definition drift; unexpected extra
indexes or triggers are also rejected. The foreign keys prevent deleting a
parent snapshot while an allocation binding still references it; there is no
implicit ownership cascade or provider cleanup.

Generation zero describes a specification that has never allocated a runtime;
bindings require a positive generation. A future allocation increments the
specification generation and commits that same value in the runtime and binding.
The allocator must validate bounded metadata and all cross-record identities;
the schema alone does not establish session ownership or job membership.
Retention expiry cannot remove a bound workflow or its consumed creation claim.
Any future retirement operation must explicitly retire the binding in the same
transaction, after its separate cleanup/recovery contract permits retirement.

This schema prerequisite exposes no allocation read/write API and introduces no
new workflow node kind, worker creation, adoption, lifetime policy, or UI behavior.
The future allocator must commit and validate both snapshots with their binding
in one transaction before the existing creation fence can grant provider creation.

### Durable runtime creation denials

Schema 4 adds `remote_runtime_creation_fences`. During the immediate migration
transaction, stream every existing remote snapshot across all sessions, bounded
to one 4 MiB snapshot at a time. Reuse the validated aggregate decoder to obtain
canonical workflow/job UUIDs; do not extract identity using unvalidated JSON or
infer ownership from old runtime references. Invalid or oversized snapshots roll
back the entire upgrade, including its new schema objects and version change.
The one-time migration is linear in the stored records and must run off the
render thread; it does not impose a new global session/record recovery limit.

Each saved runtime contributes an append-only creation denial, including when
its ordinary workflow is missing, expired, or has never consumed a claim. The
denial is not an allocation binding, consumed claim, or ownership grant. Preserve
all existing snapshot bytes, revisions, and creation claims. Generic record APIs
cannot introduce another runtime or change an active runtime's identity.
The private prepared replacement also records the canonical workflow/job denial
in the same transaction as every nonempty runtime snapshot. Future allocation
therefore publishes the denial before committing its runtime/workflow/binding;
a later failure rolls back the snapshot and denial together. Repeated writes
preserve the same append-only identity. No public runtime-import path is added.
Neither runtime retirement nor workflow deletion removes these denials; future
allocations must use fresh identity. No provider resource is touched.

Generic worker creation checks both workflow and job denials inside its existing
immediate transaction using two bounded indexed existence queries. Unrelated
ordinary workflows remain usable. A future atomic allocator may bypass this
negative fence only through its separately verified, complete current
allocation binding; absence of a binding can never be sufficient authority.
Copying a saved job ID into another ordinary workflow cannot obtain another
grant, even after losing the original binding or retiring its runtime snapshot.
Reopening a store or recreating an ordinary workflow with a saved runtime's IDs
cannot mint a replacement worker. The two identity columns are the physical row
key, with no implicit rowid that could redirect a replacement. Update/delete
triggers protect the append-only rows, and exact versioned table/index/trigger
validation runs on open and every operation. Missing or altered metadata is
rejected, not repaired or adopted.

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
