---
applyTo: "crates/horizon-cloud/src/hetzner/servers/deletion.rs,crates/horizon-cloud/src/hetzner/cloud/lifecycle.rs,crates/horizon-cloud/src/hetzner/servers.rs,crates/horizon-cloud/src/hetzner/cloud.rs,crates/horizon-cloud/src/hetzner/tests/servers.rs,crates/horizon-cloud/src/hetzner/cloud/tests/lifecycle.rs"
---

# Hetzner observed absence boundary

Read `docs/architecture/hetzner-observed-absence.md` before review.

- Keep existing `delete` behavior and its persistence order unchanged.
- Produce an observation only for an initially `Bound`, canonical nonzero server
  ID. The same immutable client must read its positive identity and exact absence.
- Check the exact ID, operation name and ownership label. A provider response is
  untrusted data. Never treat an initial HTTP 404 as inherited identity.
- Keep `Prepared`, resolved `Requested`, `Terminated`, another client's initial
  HTTP 404 and failed replies outside the observation contract.
- Return the observation only after all remaining cleanup and durable saves
  succeed. Errors, cancellation and failed saves must return no observation.
- Keep the result opaque, nonserializable and without a public constructor.
  Do not add a credential identity or infer durable authority from a saved path.
- Do not add Core consumers, public operations, tailnet cleanup or release
  effects as part of this SDK prerequisite.
