---
applyTo: "crates/horizon-cloud-worker/src/exposure_gate/**,crates/horizon-cloud-worker/Cargo.toml,scripts/ci-cargo-test-shard.sh,scripts/test_ci_cargo_test_shard.py"
---

Read docs/architecture/trusted-exposure-gate.md and the Producer boundary.
The adapter consumes the original Producer and opaque AcceptedPeer; never add a
metadata/JSON/vector/callback peer factory or replace its original stream.
Pending registration retains only the opaque PendingCancellation weak handle,
never a cloned payload FD or raw UnixStream. Producer::accept alone issues a
PendingStream bound to its private backend origin; identification consumes it.
Compare the private original-accept cookie through AcceptedStream::matches_pending
before moving the original opaque peer into named policy and StableID accounting.
The weak handle must physically cancel payload and installed control before and
after identification without retaining either descriptor. Keep the original
16 KiB socket-buffer request and 64 KiB observed maximum through the opaque pending
operation; do not expose an FD to preserve this bound. Copied DNS/tags, a PROXY
header or loopback tuple alone cannot authorize it. Direct P admission remains
actual socket CIDR only; direct constructor and TailnetOnly still refuse names.

Identified whole-flow cancellation must call AcceptedStream.cancel, not only shutdown its payload.
Wait for original control DRAINED0x01+EOF before normal or budget-grace completion,
using existing deadlines/phase/cancel only. Observe control failure during connect,
forwarding and graceful drain; retained reader Arcs cannot defer cancellation.
Direct TCP finish stays immediate. Never count queued response bytes as remote delivery.

Preserve Main conservative byte charges/grace/release, pending limits, bounded
buffers/draining, immutable deadlines, unpolled-drop and JoinError behavior.
Close shuts sockets first, joins all flow callers, then retires the SAME Producer.
Drop/error is not a clean mapping/retirement receipt. No owned Serve/Funnel,
protected runtime, keeper, Core owner, desktop or public path belongs in this cut.
Keep high-port protection/source/public qualification limitations explicit.

Require actual genuine original-stream DNS/tag allow AND deny in both daemon
modes before claiming live qualification. A custody refusal or no connection
cannot satisfy named denial. Tests may inspect kernel sockets and negative
futures, but must not fabricate successful AcceptedPeers or fake WhoIs facts.
Actual selectors must match their sole verified peer to the separate captured
StableID; another flow cannot supply a named-denial observation.
Use only Producer::accept on its original root-private Unix listener; never add
a TCP backend, bridge, caller-supplied stream factory or fabricated peer. Root
Unix credentials and the pinned Unix Serve PROXY-v1 runtime are prerequisites.
Keep provisional Unix accounting separate from real peer IPs and StableIDs.
Preserve the Producer dependency custody checks. Preserve SDK, gateway,
Owner, Producer and encoder CI commands; add the Gate suite only once in Linux
libs with independent failure propagation. Keep paired guidance identical.
