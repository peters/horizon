---
applyTo: "crates/horizon-ui/src/app/cloud_panel/production/removal*,crates/horizon-ui/src/app/cloud_panel/production/removal/**,crates/horizon-ui/src/app/cloud_panel/production/close*,crates/horizon-ui/src/app/cloud_panel/production/close/**,crates/horizon-ui/src/app/cloud_panel/production/lifecycle.rs,crates/horizon-ui/src/app/cloud_panel/production.rs"
---

# Cloud removal boundary

Read the cloud-close boundary in docs/architecture/maintainability.md and the
cloud-close test procedure. Records, journals and filesystem timing are untrusted.
Keep all removal eligibility filesystem and Store operations on the worker.
Core can_remove remains authoritative. Display snapshots cannot authorize discard.
A fresh final check keeps its Store on the worker through exact UI application;
the UI owns only a release sender. Do not join workers or drop Stores on the UI.
Check the root, session, workspace, complete launch and request generation before
applying a result. Cancel, close, root drift and session reset must not authorize a
newer cloud or spawn another read while the original record read remains blocked.
Cancellation cannot interrupt blocking OS I/O. Retain that record's job slot until
its channel disconnects. Remove anyway preserves the original record and refuses
an operation that still holds its lock. Pending is not a failed removal, including
automatic close after deletion. This UI change adds no provider action or MCP grant.
The `cloud_delete` tool of an agent deletes provider resources and grants no
discard authority: it leaves the record and the card, which this removal flow
still owns. See `cloud-delete-request.md`.
Dismissal must clear automatic-close intent and restore hidden panels, so a blocked
AfterDelete read cannot restart removal after Cancel or Escape. A Ready watch is
not a busy operation: explicit Remove anyway must pass its receiver-only guards,
while Runtime::busy and the original Store lock still refuse an active operation.
