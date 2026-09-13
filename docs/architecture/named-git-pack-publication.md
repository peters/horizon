# Explicit named Git-pack publication

Status: proposed. Date: 2026-09-13.

## Context and decision

Some storage does not provide the qualified sibling publisher's required
no-replace rename behavior. An explicit, separate `publish_named_git_pack` API
uses `<root>/<encoded-pack-sha256>/pack` in an existing private root. The default
qualified publisher and receiver remain unchanged; there is no fallback.

Only a successful exclusive mkdir grants one invocation ownership of a fresh
permanent digest slot. It reuses the fixed-layout observer and synchronization
sequence, persists the empty claim, then renames the verified incoming directory
once and synchronizes the resulting pack, slot and root. Named/held identities
are rechecked throughout; the filesystem and same-user owner must remain trusted.

## Alternatives and consequences

Weakening the default publisher would silently change its contract. Copying into
an existing slot would allow partial replacement. Neither is used. Partial or
foreign slots remain conflicts, not absence or reusable locks. Only an identical
complete pack may be explicitly reverified and resynchronized; an unused incoming
pack remains in the returned outcome. No rename occurs on that existing path.

Separate named failures retain input and destination locators, distinguish an
uncertain claim/rename from a known rename with incomplete synchronization, and
never authorize cleanup, overwrite, retry or task execution. Cancellation and
lost replies do not release claims. Directory synchronization can block outside
native process deadlines; no latency or physical durability guarantee is added.

## Follow-up boundary

The receiver still uses no-replace renames for its private pack/index names.
Named publication alone therefore does not establish end-to-end compatibility
with a particular remote filesystem. A separately explicit receive mode may be
needed. Complete-base export consent, checkpoint manifests, worker scheduling,
provider qualification and real retained-data recovery proof remain separate.
