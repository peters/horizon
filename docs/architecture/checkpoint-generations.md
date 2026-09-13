# ADR: Worker-owned repository checkpoint generations

**Status:** One-shot operation implemented and locally validated; broader acceptance remains open
**Date:** 2026-09-13
**Deciders:** Repository maintainers under the approved #383/#471 delivery scope

## Context

The existing independent worker service protects selected staged and working
bytes, including across commits. It does not retain the complete committed base
needed to reconstruct a checkout after losing the source. Named bundle and pack
primitives provide explicit ownership and uncertainty handling, not a complete
checkpoint. The next outcome must connect those primitives to worker execution.

## Decision

Add one explicit `checkpoint-once` operation using existing core capture, seed,
pack, named receive/publication and bundle verification. Keep byte-capture
enrollment versions 1 and 2, their bindings and their service responses unchanged.
Do not add a second receive-pack protocol as an intermediate requirement.

The new bounded request separately consents to retaining the complete current
base closure, including removed files and raw commit metadata. It identifies the
prepared checkout, selected paths, an existing private same-volume destination
outside the checkout and a caller-chosen attempt name. Attested retained storage
is an operator assertion, not provider qualification.

Only successful exclusive claim grants permission to populate a generation.
Its private directory retains scratch, named packs and named bundles. Publish a
create-new generation manifest only after verifying every referenced artifact.
Bind the preparation, sampled commit/branch, declared coverage, pack digest and
length, overlay identity and observation times. No successful response may imply
that unselected dirty files, task state or Git parent history were captured.
Unsupported submodules and LFS payload recovery must fail visibly, not masquerade
as ordinary complete file recovery.

Retain earlier generations and all uncertain attempts. Existing attempts are
not adopted, overwritten, retried or cleaned up. Failure locators do not prove
ownership. Account scratch and partial data as well as completed artifacts;
bounded admission and native limits are not a hard filesystem quota or a promise
that uninterruptible I/O terminates promptly. Initial generations are self-contained;
shared-pack reuse is a later optimization, not a correctness prerequisite.

Existing destination and generation roots must remain owned private `0700`
directories with no symlink ancestry. Confined descendants may inherit the
caller's umask, as in the named receiver: their modes alone do not expose data
through the verified private root. Accounting still rejects foreign ownership,
devices, links, special nodes and set-id/sticky modes. It does not chmod retained
data or change process-global umask; stable exclusive ownership remains required.
Admission requires the pinned checkout and destination to share a kernel mount
identity, not merely a device, and rejects overlapping descriptor ancestry before
claiming anything. Bind aliases and unavailable mount identity fail closed; a
separately mounted destination is not supported by this first implementation.

## Options considered

| Option | Complexity and cost | Growth and maintenance |
| --- | --- | --- |
| Direct one-shot generation using existing core APIs | Bounded integration; no new cloud cost | Reuses existing Rust boundaries; periodic consumption can follow |
| Version another receive-pack CLI first | Additional protocol and tests | Does not by itself connect protection to the worker service |
| Change periodic enrollment and restoration together | Largest immediate scope | Couples compatibility, scheduling and recovery before one-shot proof |

## Trade-offs and consequences

The first option gives a usable verified generation while keeping compatibility
and failure diagnosis bounded. It is not yet periodic full-repository/task
protection, a coherent application snapshot, independent backup or physical
durability proof. Linux is the first implementation; existing platform support
and CI remain intact. No provider operation, network transfer, task replay,
destructive deletion or `RepositoryCheckpoint` watermark advancement is implied.

## Action items

- [x] Implement and prove the exact-binary one-shot operation, refusal paths,
  unchanged source/earlier generations and retained response uncertainty.
- [ ] Add periodic consumption with separate explicit enrollment and verified
  generation readback before success; measure progress with clients disconnected.
- [ ] Restore a declared generation into a fresh private checkout, verifying all
  references without overwriting the original or inferring task resumption.
- [ ] Complete declared coverage, provider filesystem/mount qualification,
  verified-loss recovery and final-write-freeze/Delete acceptance in #471/#475.
