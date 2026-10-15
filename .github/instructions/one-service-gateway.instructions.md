---
applyTo: "crates/horizon-core/src/cloud_runtime/local_network/endpoint.rs,crates/horizon-core/src/cloud_runtime/local_network/endpoint/**,crates/horizon-core/src/cloud_runtime/local_network.rs,crates/horizon-core/Cargo.toml"
---

# One-device endpoint engine

Read the endpoint-engine trust boundary in docs/local-network-bridge.md.
This is the feature-off prerequisite of #1335 M1-1, not completed one-device sharing.
Keep both LAN and tailnet endpoint requirements, owner controls, durable attempts,
worker SSH, public parity and real-worker qualification in the delivery plan.
Do not add an engine caller or authority from a parsed endpoint, path or receipt.
Keep one-service-gateway out of defaults. Preserve the base cloud-workspaces
feature graph when adding the separate engine feature. On the independent Engine
base, tailnet-node-cleanup remains off by default; #1646 owns its activation.

Treat endpoint and SOCKS input, DNS, routes and client replies as untrusted.
Reject malformed input before lookup. Admit only the exact captured endpoint;
connect to its checked literal address without a second destination resolution.
LAN keeps the existing route/subnet/interface policy and Self refusal.
Tailnet requires Running kernel mode, trusted installed CLI/fixed socket,
Self source route, direct peer address, peer StableID and normalized full name.
Refuse Self, subnet-route identities, aliases, ambiguous IPv4 sets and changed
post-lookup Self. Never enroll, mutate policy or use desktop API credentials.

Compare the complete binding at start, admission and source checks.
Source loss/change closes old relays and refuses new admission. Busy checks may
refuse one request; they must not widen access or clear a prior revocation.
Keep operation/generation in close receipts; reused ports cannot authorize close.
Preserve bounded commands/output, relay limits, half-close and private errors.
Status exposes only the approved endpoint, counters and fixed lifecycle reasons.

Require focused Engine tests and existing bridge regression checks.
Real-kernel tests run only in explicitly owned disposable guests with an original
Self/Peer fixture; no host execution or fabricated wrong-route outcome.
A missing real counterexample is blocked, not a pass. Do not count ignored tests
as executed, or component tests as owner/worker qualification.
