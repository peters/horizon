---
applyTo: "crates/horizon-core/src/cloud_runtime/local_network/endpoint.rs,crates/horizon-core/src/cloud_runtime/local_network/endpoint/**,crates/horizon-core/src/cloud_runtime/local_network/session.rs,crates/horizon-core/src/cloud_runtime/local_network/socks.rs,crates/horizon-core/src/cloud_runtime/local_network/socks/private/**,crates/horizon-core/src/cloud_runtime/local_network/socks/private.rs,crates/horizon-core/src/cloud_runtime/local_network.rs,crates/horizon-core/Cargo.toml"
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
Normalize one terminal dot in both approved and requested names. Refuse another
terminal dot or another name/port before source lookup. For LAN, check the complete
canonical IPv4 set before policy filters or caps answers. Duplicate-only answers
can pass; mixed LAN/outside answers or four duplicates plus another IPv4 cannot.
Apply the existing policy to the single literal without resolving again. Cover
initial admission, preflight-to-start changes and permanent active revocation.
Keep the existing bridge's filtering and candidate cap unchanged.
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

Read docs/architecture/pinned-unix-ssh-attachment.md for the attachment boundary.
Keep it default-off and internal. No owner controller, OriginalCloud, saved-membership
path or public activation belongs to this primitive. Preserve the original SSH pins
and unchanged worker protocol; no new discovery or credential grant is permitted.
Unix attachment must use its fresh private Unix target, never a reusable TCP port.
Keep fixed parent ownership, non-symlink, directory0700 and socket0600 guards.
Keep Gateway::start TCP behavior on every platform, including macOS.
Only explicit Gateway::start_private supplies Unix custody; attachment has no TCP fallback.
Non-Unix private start and attachment must refuse before SSH.
Keep ordinary Bridge TCP and default session behavior unchanged.
Source revocation must prevent SSH retry and close the exact physical source.
Stalled input cannot block heartbeat or owned Drop. Stale close changes nothing.
Require actual original-worker LAN and tailnet SSH tasks on first/final candidate
inputs before qualification; ignored, synthetic or historical combined tests cannot replace them.
