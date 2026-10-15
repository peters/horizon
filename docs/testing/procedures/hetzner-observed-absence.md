---
procedure: hetzner-observed-absence
feature: SDK server absence observation
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Hetzner server absence test procedure

## 1. Purpose

This procedure checks the additive SDK deletion result and its trust boundary.
It uses synthetic HTTP responses and makes no provider resources.

## 2. Applicability

- Candidate: `horizon-cloud` with the `delete_observed` API.
- Platforms: Linux, macOS and Windows with the repository Rust toolchain.
- This procedure excludes real provider accounts, Core release operations and
  Horizon UI, CLI or MCP behavior.

## 3. Equipment and preconditions

- Use a task checkout with the candidate source and locked dependencies.
- Use a private home directory without provider credentials or a Secret Service.
- Permit the tests to bind an operating-system-assigned loopback port.
- Keep the test log and the exact source revision.

## 4. Setup

1. Run the format check.

   ```sh
   cargo fmt --all -- --check
   ```

   Result: The check succeeds.

## 5. Tasks

### H01 — Positive identity and completed cleanup

1. Run the observation tests.

   ```sh
   RUSTFLAGS="-D warnings" cargo test --locked -p horizon-cloud --all-features observed
   ```

   Result: An initially bound server returns its exact operation and server IDs
   only after the server, volume, key and final saves complete.

2. Examine the negative test results.

   Result: Initial absence, resolved requests, terminated history, another client,
   invalid identity, malformed replies, cancellation and failed saves produce no
   observation. Repeated deletion keeps the existing durable state rules.

### H02 — Existing SDK behavior and opaque result

1. Run the complete SDK tests and documentation tests.

   ```sh
   RUSTFLAGS="-D warnings" cargo test --locked -p horizon-cloud --all-features
   ```

   Result: Existing SDK tests pass. The compile-fail example refuses construction
   of an observation from saved IDs.

2. Run the SDK strict check.

   ```sh
   cargo clippy --locked -p horizon-cloud --lib --all-features -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
   ```

   Result: The check succeeds.

## 6. Pass criteria

- Both tasks pass on the candidate source.
- The result remains opaque and no durable consumer or public operation changes.
- The run makes no real provider or credential-store request.

## 7. Cleanup

1. Examine the test completion status.

   Result: The fixture threads stop and their loopback listeners close.

## 8. Record of results

Put the commands, revision, source changes and results in the pull request.
Keep private logs outside the repository. Label real provider qualification as
not run; these fixture results do not replace it.
