---
applyTo: "crates/horizon-cloud-protocol/src/exposure.rs,crates/horizon-cloud-protocol/src/exposure/**,crates/horizon-cloud-protocol/Cargo.toml"
---

# Exposure policy boundary

Read `docs/architecture/exposure-policy.md`. This default-off component provides
pure comparison facts. It adds no public dispatch. The separate default-off
`exposure-control` comparison contract is documented in
`docs/architecture/exposure-control.md`.
For protected ingress geometry and receipt code, also read
`docs/architecture/exposure-protected-ingress.md`.

Report these failures:

- Missing mode selects public access, or malformed Restricted sources fall back to Public.
- JSON accepts unknown fields, duplicate fields, more than 16 KiB, or unbounded source and connection counts.
- CIDR parsing crosses address families or accepts malformed prefixes.
- A short DNS name, partial name, wildcard or partial tag supplies an exact named match.
- A management port, reserved logical ingress/backend port or protected physical
  port 600–663 becomes an accepted target.
- A named-rule comparison, saved URL or tailnet-range IP becomes authenticated membership, owner approval or runtime authority.
- A refused HTTPS route falls back to raw TCP, accepts a non-Public policy, or accepts a port other than 443, 8443 or 10000.
- A URL supplies readiness or TLS proof without the later original live controller.
- The M2 eight-hour constant changes M1 sharing lifetime.
- Receipt parsing accepts extra/missing/duplicate publications, a backend or
  non-TCP publication, an incomplete address family, mutable image tags or
  unbounded/unknown/duplicate JSON fields.
- Passive receipt claims become actual host publication, root-only binding,
  source preservation, ownership or readiness without original runtime checks.
- New port helpers silently change existing policy, backend or listener behavior.

Keep worker, provider and public owner runtime activation outside this
prerequisite. Control DTOs remain separately feature-gated passive facts. The Core
record outcome must preserve existing publication, enrollment and admission
formats on current Main.
