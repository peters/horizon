---
procedure: exposure-policy
feature: Pure exposure policy and route facts
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Exposure policy test procedure

## 1. Purpose

Test bounded policy parsing, exact source comparisons and explicit route facts.
Make sure no parsed value grants runtime authority or selects a raw fallback.

## 2. Applicability

- Candidate: the optional Protocol `exposure-gate` feature.
- Platforms: each supported Rust host.
- This procedure does not test worker listeners, provider ingress, native owner
  approval, public interface parity, real membership, TLS or service readiness.

## 3. Safety

Use only the synthetic addresses, names and tags in the unit tests. This
procedure uses no account, key, network service or paid resource.

## 4. Equipment and preconditions

- Use the repository Rust toolchain and locked dependencies.
- Use an isolated task checkout and build cache.
- Keep the feature off in the normal application configuration.

## 5. Setup

1. Read `docs/architecture/exposure-policy.md`.

   Result: The pure comparison boundary and later runtime requirements are clear.

2. Run `cargo fmt --all -- --check`.

   Result: The format check passes.

## 6. Tasks

### 6.1 P01 — Parse and compare policy facts

1. Run `cargo test -p horizon-cloud-protocol --features exposure-gate`.

   Result: All tests pass. Missing mode is Off. Invalid JSON, counts, limits,
   management ports, reserved targets, CIDRs, full DNS names and tags are refused.
   Mixed rules use exact OR semantics only after all rules pass validation.

### 6.2 P02 — Refuse an invalid route

1. Examine the route tests from P01.

   Result: HTTPS requires explicit Public policy and port 443, 8443 or 10000.
   An invalid route, DNS name or virtual port returns an error. It does not
   supply a raw fallback. A valid URL remains comparison data.

### 6.3 P03 — Keep the default feature set

1. Run `cargo test -p horizon-cloud-protocol`.

   Result: The existing default tests pass without the exposure module.

2. Complete the repository local matrix for the stable candidate.

   Result: Blocking and strict checks pass. Record advisory findings separately.
   Test the embedded skill references as application build inputs.

### 6.4 P04 — Compare protected ingress geometry and receipt facts

1. Examine the ingress tests included in P01.

   Result: Every logical port maps to one disjoint privileged listener/backend.
   Existing backend calculation stays unchanged. Complete single-family and
   dual-family receipts parse. Partial, duplicate, extra, backend, non-TCP and
   non-wildcard publication sets are refused. Identity changes stay distinct.
   Invalid identities, unsafe claimed sysctls, unknown/duplicate JSON fields and
   receipts larger than 16 KiB are refused.

2. Read `docs/architecture/exposure-protected-ingress.md`.

   Result: Tests grant no live host or kernel authority. Provisioning, effective
   root-only binding, original SSH readback and real source-preservation tests
   remain required before using this geometry in a running exposure.

## 7. Pass criteria

- P01, P02, P03 and P04 pass on the recorded candidate inputs.
- No control DTO, public dispatch or live resource is added by this outcome.
- The results retain the distinction between comparison facts and authority.

## 8. Cleanup

1. Stop the task test process after its result is recorded.

   Result: No live worker, listener, account or paid resource needs cleanup.

## 9. Record of results

Put the commands, source inventory, features, toolchain and result logs in the
pull request. Preserve raw failures and exact environment complements when used.
Use `docs/testing/reports/TEMPLATE.md` if the run requires a permanent report.
Do not describe pure component tests as real ingress or completed epic delivery.
