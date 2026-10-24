---
applyTo: "crates/horizon-cloud-protocol/src/exposure.rs,crates/horizon-cloud-protocol/src/exposure/**,crates/horizon-cloud-worker/src/exposure_gate/**,crates/horizon-cloud-worker/Cargo.toml,crates/horizon-cloud-protocol/Cargo.toml"
---

# Worker TCP gate

Read docs/architecture/worker-exposure-gate.md before this review.
This is a default-off component, not completed M2 or public interface activation.
Off/default creates no listener. Public requires an explicit choice.
Shared policy syntax validation does not grant runtime support. Empty Restricted and any unsupported or malformed rule must fail before binding, including mixed CIDR/name/tag lists.

CIDRs use peer_addr from the same original accepted stream.
Never trust a caller's identity, PROXY header, loopback tuple or stored metadata.
DNS, tag and TailnetOnly dispatch stays unsupported until a trusted producer is
integrated and qualified against the actual daemon/accepted-stream boundary.
Do not add a fallback that converts these rules to Public or CIDR-only dispatch.

Keep production root custody and external owner authority distinct.
Use exclusive fresh listeners in the fixed ingress pool, separate IPv4/IPv6,
with no adoption, reuse-port or partial-bind fallback.
Targets are literal loopback plus explicit nonmanagement ports; reject the
management, worker control/proxy and reserved ingress/backend ports.
Raw TCP adds no TLS or authentication.

Require finite connection/source/buffer/byte/time bounds and atomic charging.
Count pending connects, preserve source charges and cap source-history growth.
The lifetime is monotonic, immutable and at most eight hours; no renewal extends it.
Close must shut transports before joining and acknowledge only after listeners
and every tracked flow task close. Cover cancellation before a flow's first poll.
Drop must cancel fail closed. Do not treat TIME_WAIT as a live listener descriptor.

No counters or decoded policy authorize provider release, owner approval or retry.
Require actual-socket allow/deny, spoof refusal, bounds, half-close and close tests.
Keep both bundled references and the permanent procedure accurate about limits.
