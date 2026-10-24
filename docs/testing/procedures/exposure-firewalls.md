---
procedure: exposure-firewalls
feature: Exact owned Hetzner firewall operations
platforms: [linux, macos]
cost: free
destructive: no
secrets: []
owner: peters
---

# Exposure firewall component test procedure

## 1. Purpose

Test provider transport identity checks and uncertainty without changing a real firewall.

## 2. Applicability

Use the explicit Cloud `exposure-firewall` feature. Read
[the boundary](../../architecture/exposure-firewalls.md) first.

## 3. Safety

Use synthetic credentials and loopback scripted HTTP providers. Do not load real
credentials, attach a firewall or change a live server during this procedure.

## 4. Equipment and preconditions

Use the repository toolchain. Preserve exact source, environment, commands and logs.
Have the current public Hetzner OpenAPI specification for the API check.

## 5. Setup

1. Keep the feature off in default application builds and worker dispatch.

   Result: No running cloud or owner permission changes.

## 6. Tasks

1. Run `python3 scripts/check-hetzner-api.py`.

   Result: All request/response fields and operations match the current primary specification.

2. Run `cargo test -p horizon-cloud --features exposure-firewall`.

   Result: Create remains unattached. An uncertain POST is not repeated. Current
   original server ownership and exact firewall scope are checked. Apply and remove
   use one numeric server; selectors, foreign targets, changed IDs and ownership
   refuse before mutation. Delete refuses attached firewalls. Reconciliation
   follows pagination and rejects ambiguity. Invalid rules and action batches fail.
   Effective-rule counts use numeric network prefixes, so equivalent IPv6
   spellings share one prefix. Inbound and outbound count separately. More than
   500 distinct directional prefixes fail before provider I/O.

3. Run applicable default/enabled checks and required lint tiers, formatting,
   maintainability and bundled skill coverage on the stable inputs.

   Result: Each reported result covers its exact inputs; reuse is documented.

## 7. Pass criteria

All applicable component checks pass. The report does not claim live management
access, source attribution, owner lifecycle or provider release qualification.

## 8. Cleanup

1. Stop only test-owned HTTP listeners and remove owned temporary files.

   Result: No paid resource or user state changes.

## 9. Record of results

Retain source, specification provenance, command logs, result counts and limits.
