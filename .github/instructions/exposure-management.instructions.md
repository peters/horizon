---
applyTo: "crates/horizon-cloud/src/hetzner/firewalls/management/**,crates/horizon-cloud/src/hetzner/tests/firewalls/management.rs,crates/horizon-cloud/src/hetzner/servers.rs,crates/horizon-cloud/src/hetzner/tests/servers.rs,crates/horizon-core/src/cloud_runtime/deployment/hetzner/tests/exposure_management.rs"
---

Review against docs/architecture/exposure-management.md. The original client is
consumed, never reconstructed from journal/DTO facts. Check exact original
allocation, numeric server, SSH endpoint and settled attachment snapshots. Refuse
selectors, unknown enabled attachment fields, duplicate IDs and foreign drift.
ManagementRules validates syntax only; the real owner must cover its pinned SSH
policy. No management widening is allowed when foreign firewalls exist. Preserve
all sibling contributions and every foreign inbound/outbound rule.

Intent precedes each mutation and returned IDs precede waits. After uncertainty,
never reopen, retry create or adopt a successor. Original-only cleanup must compare
expected rules/foreign baseline and cannot issue physical worker retirement proof.
The caller must physically close its Gate on errors and before ingress removal.
Retain requested create rules before fallible action saves or waits. A failed
current verification must latch cleanup-only uncertainty, even after provider
recovery. Before snapshot adoption in cleanup, compare the original SSH endpoint,
foreign baseline and the captured owned rules against the first validated lookup;
journal the fresh captured firewall. The absent branch must also reject SSH drift.
Inspect lost replies, failed journal saves and action waits together. No new
activation, credential factory or restored controller belongs in this outcome.

For the ignored actual-provider consumer, require the actual normal Store lock,
normal provider journal, current settings reference, exact server/volume/key facts
and original pinned SSH. Input DTOs cannot grant Native authority. Keep credential
bytes on the host and retain the same Manager after failures. Check durable scope
before create, returned IDs before waits, original-only cleanup before dropping it,
and supervisor recovery after process loss. Ordinary delete alone can orphan an
unattached firewall. Do not expand this component into public owner activation.
