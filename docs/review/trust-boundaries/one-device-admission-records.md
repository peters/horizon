---
applyTo: "crates/horizon-core/src/cloud_runtime/state.rs,crates/horizon-core/src/cloud_runtime/state/service_admission/**,crates/horizon-core/src/cloud_runtime/state/service_admission.rs,crates/horizon-core/src/cloud_runtime/state/migration/**,crates/horizon-core/src/cloud_runtime/allocation/legacy.rs,crates/horizon-core/src/cloud_runtime/allocation/legacy/**,crates/horizon-core/src/cloud_runtime/deployment.rs"
---

# One-device admission metadata

Read docs/architecture/one-device-admission-records.md before this review.
This outcome stores metadata only. Do not add an owner decision, runtime constructor,
gateway restoration factory, worker command or public sharing operation.
Treat all saved JSON as untrusted facts, never serialized authority.
Keep Record fields and its decoder private. Scope construction validates shape only.
Require valid original scope, attempt UUIDs, endpoint fields and retained phase.
Reject unknown fields, malformed records, stale version shapes and moved ownership.

Deployment v6 and allocation v4 apply only to nonempty admission evidence.
Missing admission preserves existing versions and omitted-field encodings.
Enrollment, publishing, siblings and working-copy facts must survive split/join.
Publishing checks remain active when admission selects the newer version.
Keep Armed/Attached uncertainty and original attempts after worker removal.
Decoded Closed does not prove physical cleanup or grant a fresh operation.

Preserve current Store::load/snapshot/read delegation and pending storage fences.
Snapshots create no root, take no lock and grant no operation custody.
Preserve current publishing preparation, profile choices and account checks.
Initializer adaptations must not copy an older deployment implementation.
The metadata reader stays available under cloud-workspaces without an Engine flag.

Require version, paired ownership, retained-phase and publication coexistence tests.
Keep both bundled reference copies equal and describe storage limits accurately.
Component storage tests do not qualify owner, worker, network or public parity.
