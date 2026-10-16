---
applyTo: "crates/horizon-core/src/cloud_runtime/local_network/endpoint/admission.rs,crates/horizon-core/src/cloud_runtime/local_network/endpoint/admission/**,crates/horizon-core/src/cloud_runtime/local_network/endpoint.rs,crates/horizon-core/src/cloud_runtime/owner_custody.rs,crates/horizon-core/src/cloud_runtime/owner_custody/**,crates/horizon-core/src/cloud_runtime/state/service_admission.rs"
---

# Core original-owner controller

Read docs/architecture/one-device-owner-admission.md before reviewing this boundary.
This is an optional Core prerequisite. Keep one-service-gateway off by default;
there are no native/public callers in this outcome.
Public requests are metadata, never admission. Full M1 qualification remains open.

Treat endpoint/source/worker replies and serialized records as untrusted facts.
OwnerScope validates syntax, not approval or saved membership. Only the
trusted native caller may supply a current verified original owner and explicit
endpoint approval. A parsed record, port, receipt or filesystem path grants none.
Keep complete current launch, original ready deployment/spec/worker/image/route,
settings and pinned SSH arguments bound to the original held Store. Refuse drift,
pending replacement/restart and unresolved Armed/Attached attempts.

Save validated Armed intent BEFORE starting any listener or SSH. Startup/save
failures preserve original uncertainty; no reconstruction or successor fallback.
Start cancellation's I/O-free physical watcher before Attached persistence.
Blocked record/settings/native observations cannot delay physical revocation.
Retain the same OriginalCloud alias until original observers and SSH have retired.
The original OS Store lock is held once across original controller aliases. Use fresh admission-
field transactions and preserve every sibling field; an alias grants no approval.
Attached/local Active is custody, not real helper Ready or service reachability.

Exact owner+operation/generation governs close. `finish` must borrow its handle:
invalid close arguments must not consume it and trigger Drop retirement. Test both
foreign-owner and stale-generation finish against actual helper Ready, unchanged
active source/Store custody and durable Attached bytes. Revoke the original physical
source BEFORE waiting for observers or SSH. Save Closed only after retirement,
reloading the same held Store and matching the exact original attempt. Failed
save/missing/changed records preserve uncertainty. Drop never fabricates Closed.
Preserve stale-close refusal, delayed-read cancellation and lock/byte assertions.

Do not change the current Engine DNS/source checks, private Unix ingress fences,
worker protocol or default bridge behavior. No provider/API/OS-store operation,
enrollment, policy change, credential grant, public ingress or M1 expiry belongs
in this outcome. Component tests and source recovery are not real-owner, real
LAN/tailnet/SSH, native GIF or public parity qualification.

The real controller dispatch must call explicit Gateway::start_private; ordinary
Gateway::start remains TCP. Keep private physical fixtures Linux-only and gate
Store-backed fixtures to Unix directory-durability support. Preserve the portable
pure generation/state cases and the existing non-Unix Store refusal regression;
non-Unix production still compiles but cannot acquire live Store custody. Preserve passive Main exposure sibling tests,
but exclude future held-control reservations, membership observers and M2 runtime flags.
