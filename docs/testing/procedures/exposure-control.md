---
procedure: exposure-control
feature: Default-off exposure control comparison messages
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Exposure control comparison messages test procedure

## 1. Purpose

Test bounded untrusted control frames and passive observations without creating
runtime authority.

## 2. Applicability

Use a candidate with Protocol `exposure-control` enabled. This offline component
does not test an owner stream, root keeper, listener, provider or public interface.

## 3. Safety

No credential, real tailnet, paid worker or live resource is needed. Do not treat
decoded IDs or observations as an approval or a request to start compute.

## 4. Equipment and preconditions

Use a separate checkout, private test state and bounded build cache. Complete
source review against the documented trust boundary first.

## 5. Setup

1. Read `docs/architecture/exposure-control.md` and record current source inputs.

   Result: Frame limits and the serialized-facts boundary are explicit.

## 6. Tasks

### 6.1 C01 — Refuse invalid raw frames and observations

1. Run the enabled focused tests.

   ```bash
   cargo test --locked --offline -p horizon-cloud-protocol --features exposure-control exposure::control::tests
   ```

   Result: Fifteen tests pass. Commands round-trip with independent IDs. Invalid
   versions, nil IDs, unknown/duplicate fields, trailing JSON, oversized frames,
   invalid policy/routes, zero/overflowing lifetimes and duplicate or inconsistent
   observations are refused. Repeated tailnet endpoints and IPv4-mapped Worker
   or recipient addresses are refused. Every physical target 600–663 is refused.
   Every tagged Command, Route and Worker observation refuses extra fields.

2. Inspect member and Worker observation cases.

   Result: Bounded sorted recipient facts do not grant membership. Unverified
   facts do not supply Ready or Applied. No-tailnet direct observations remain
   distinct from a lost daemon, and HTTPS never selects a raw fallback.

### 6.2 C02 — Complete the applicable local matrix

1. Complete repository checks, default workspace and speech UI tests, required
   blocking/strict tiers and enabled Protocol checks on the stable candidate.

   Result: Required checks pass. Record fresh, valid reused and non-applicable
   results separately. Advisory findings stay separate.

2. Check both bundled skills and the normal UI builds that embed them.

   Result: The complete current guidance is present in both normal candidates.

### 6.3 C03 — Review consumers and feature boundaries

1. Inspect the feature/module wiring and all new callers.

   Result: The feature stays off by default. No keeper, transport, runtime,
   provider or public operation is activated. Raw decoders enforce slice bounds;
   a future stream reader still needs its own input bound and original custody.

## 7. Pass criteria

C01 through C03 cover the current candidate. No decoded object is described as
live authority or complete M2 delivery. The only shared behavior change is the
explicit protected physical-target denial in policy validation.

## 8. Cleanup

Remove only owned temporary test state when needed. No live resource requires
cleanup.

## 9. Record of results

Record exact source, commands, environment and logs in the PR. Use the report
template if a later actual-input run requires a retained report.
