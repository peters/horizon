---
applyTo: "crates/horizon-cloud-protocol/src/exposure.rs,crates/horizon-cloud-protocol/src/exposure/**,crates/horizon-cloud-protocol/Cargo.toml"
---

# Exposure policy boundary

Read `docs/architecture/exposure-policy.md`. This default-off component provides
pure comparison facts. It adds no control frames or public dispatch.

Report these failures:

- Missing mode selects public access, or malformed Restricted sources fall back to Public.
- JSON accepts unknown fields, duplicate fields, more than 16 KiB, or unbounded source and connection counts.
- CIDR parsing crosses address families or accepts malformed prefixes.
- A short DNS name, partial name, wildcard or partial tag supplies an exact named match.
- A management port or reserved ingress/backend port becomes an accepted target.
- A named-rule comparison, saved URL or tailnet-range IP becomes authenticated membership, owner approval or runtime authority.
- A refused HTTPS route falls back to raw TCP, accepts a non-Public policy, or accepts a port other than 443, 8443 or 10000.
- A URL supplies readiness or TLS proof without the later original live controller.
- The M2 eight-hour constant changes M1 sharing lifetime.

Keep the control DTO, worker, provider and public owner interfaces outside this
prerequisite. The Core record outcome must preserve existing publication,
enrollment and admission formats on current Main.
