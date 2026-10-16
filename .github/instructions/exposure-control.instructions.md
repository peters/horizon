---
applyTo: "crates/horizon-cloud-protocol/src/exposure/control.rs,crates/horizon-cloud-protocol/src/exposure/control/**"
---

# Exposure control input and authority boundary

Read docs/architecture/exposure-control.md before reviewing this default-off pure
contract. It does not implement a keeper, transport, membership issuer or approval.

Report concrete inputs and wrong results for:

- Raw frames bypassing the 20 KiB request or 144 KiB reply limit, unknown or
  duplicate known fields, trailing JSON or invalid/non-nil operation identities.
- Shared policy validation accepting management, logical reserved or protected
  physical 600–663 targets, Off Open requests or an invalid explicit HTTPS route.
- Overflowing/zero/excessive lifetimes or decoded timestamps supplying a real
  monotonic deadline or changing M1's no-expiry behavior.
- Repeated service/operation/ingress IDs, inconsistent Ready/Closed shapes or
  active/terminal intervals and URLs that do not match exact route observations.
- Unverified observations granting Ready/Applied, or a lost original daemon
  becoming an authenticated Unavailable fallback.
- Raw member vectors, recipient facts, DNS/tags/IPs or serialized Running/Closed
  facts granting live membership, approval, custody, socket closure or cleanup.
- Public activation, root processes, provider effects or unmerged runtime overlays.

Direct serde decoding does not enforce the raw frame-size gate or full semantic
validation. A future stream reader must bound bytes during input and retain the
original live authority separately. Keep those later consumers outside this PR.
