# Combined retained repository intake

`repository_overlay::intake::{receive, observe}` is the worker core API. The
repository binary supplies its framing commands, and the Linux `intake::controller`
module supplies explicit export approval and pinned handoff. These do not create a
provider allocation, execute setup or start a task. Constructing a request, capturing
an overlay, computing a digest or recovering an allocation is not export approval.

The caller approves the **complete** exact base closure, including raw commit
metadata and files that overlays will delete, and both index/worktree layers of
one existing `RepositoryOverlayBundle`. It keeps source bytes stable and exclusively
controlled during any later transfer; an open file handle alone is not immutable.

## Request and input

Version 1 binds workspace local ID, workflow/job IDs, runtime generation, worker
resource ID, client-key SHA-256, complete `GitSource`, pack SHA-256/encoded length,
and overlay manifest/encoded length. Pack base is `source.commit`. Overlay identity
is the existing complete bundle manifest, not a second encoding or wire digest.
Canonical request JSON is capped at 32 KiB and its SHA-256 identifies the claim.
Unknown/duplicate fields, malformed IDs, zero generation/commit and excessive
lengths are rejected before storage access. Worker-supplied labels do not themselves
authenticate provider ownership; the pinned controller checks the retained allocation.

The core receiver consumes exactly the declared pack bytes, then exactly the declared
canonical overlay bytes, then EOF. Its bounded pack reader exposes local EOF without
consuming the overlay. The existing native receiver independently verifies the pack,
index and exact shallow commit closure. The overlay codec verifies all payload hashes,
manifest and source before either input is published. The pack ceiling is the shared
256 MiB default; existing bundle inner/encoded limits and native process ceilings apply.
Pack copying is chunked. Overlay decoding retains bounded complete-buffer copies,
not constant total memory. No original source repository is read on the worker.

## Worker command framing

`intake` reads a four-byte little-endian request length in the range 1–32 KiB,
then that exact JSON header, and delegates the remaining pack/overlay stream to
the core receiver. A matching existing claim can reply without consuming payload.
`intake-status` instead accepts only bounded request JSON followed by EOF; it has
no length prefix or payload. Invalid framing is rejected before storage access.

Both commands emit bounded JSON responses using the shared 128 KiB response limit.
Exit codes are 0 for acknowledgement/observation, 2 for rejected framing/identity,
4 for a claimed-unknown result, and 1 for other unsuccessful outcomes. Exit code 3
means the response could not be written; it does not undo a completed intake.
Neither command grants export approval or setup/task authority.

## Fixed roots and replay barrier

Production uses only the existing `/workspace/.horizon-worker` parent. It must be
owned by the executing UID, mode 0700, with stable private ancestry/mounts and
healthy journaled ext4 storage accepted by the existing storage qualifier. Git,
prlimit and kernel metadata must be trusted. There is no path override, chmod,
repair, filesystem fallback, or claim of hostile same-user race confinement.

The fixed `repository-intake.claim` stores the complete canonical request. Only a
fresh create-new winner that writes and synchronizes both claim and parent may
initialize `repository-inputs/{packs,bundles}` and `repository-setup`. These roots
are mode 0700, created by single-component writes under held parent descriptors,
synchronized child-before-parent, and checked against their live names. An existing
child without a claim is refused; existing, partial or conflicting claims cannot
grant another receiver or replace data. The setup root remains empty and confers
no permission to start setup or a task.

A matching existing receive request performs observation **without reading its
payload**, synchronizing, repairing or recreating roots. `observe` never creates a
claim or infers an absent/replayable operation from missing data. It rechecks the
published `packs/base` and stored bundle through the existing read-only validators.
Successful observation is current logical verification, not a new durability proof.

## Retained results and interruption

Responses distinguish acknowledged, observed, claimed-unknown, unconfirmed,
rejected, error and unsupported outcomes. Known pack progress preserves unpublished,
published-unsynchronized and rename-unconfirmed states, including both candidate
names for an uncertain rename. Bundle writes have an explicit unconfirmed state.
Roots and progress are historical candidates, not assertions that all names exist
or authority to clean them up. Absent optional fields serialize as explicit null.

Cancellation is checked before admission, directory operations, bounded input reads
and final acknowledgement, and delegated to native pack operations. Blocking reader,
storage, spawn and reap calls are not covered by a hard operation deadline. Failure,
drop or a lost outer response never deletes state or authorizes a resend. A later
caller can inspect the same intent, but cannot manufacture a fresh claim after loss.
Synchronization acknowledgement is conditional on the documented storage premises;
it is not power-loss testing, cloud retention proof, backup or repository readiness.

Unit tests cover strict metadata, unsafe/missing roots, every root-sync failure,
partial/conflicting claims, concurrent claim writers, existing-child refusal,
directory replacement, cancellation, exact payload framing and qualified observation.
The qualified positive unit test reports an explicit skip when capability is absent;
delivery additionally requires a real qualified positive smoke of this exact API.

## Pinned controller

`RepositoryIntakeProposal::new` binds a prepared pack and existing bundle to the
complete retained allocation and source without filesystem, provider or SSH access.
Retain its canonical `request()` before calling `approve_export` after a positive
caller export decision. Approval covers the entire base closure and both overlay
layers; the consumed approval type is not a human signature or persisted permission.

`send_approved_repository_intake` validates current ownership before and after
handoff, including workspaces with no attached panels. It hashes and rewinds one
held no-follow regular pack file, checks its private parent/file identity, and streams
from that same handle. The caller must keep its bytes and ancestry exclusively
stable: post-transfer detection cannot retract bytes already disclosed. Run this
off the UI thread. The ten-minute pipe budget does not bound blocking local reads,
process spawning or reaping.

The controller invokes the fixed [worker commands](#worker-command-framing) using
the retained SSH identity and pin, never an interactive trust prompt. The selected
worker binary must contain these commands; an older local runtime image alone is
not evidence that they are deployed.

`observe_repository_intake` uses the saved request without approval, local source
or pack reads, initialization, automatic resend or cleanup. Decode checks response
identity, fixed paths, state/progress combinations and process status. A validated
response can still describe uncertainty rather than acknowledgement. Failures keep
the request and any already-validated response for observation; neither cancellation
nor lost acknowledgement grants another upload, setup or task.
