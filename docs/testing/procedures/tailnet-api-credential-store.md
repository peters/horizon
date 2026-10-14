---
procedure: tailnet-api-credential-store
feature: desktop tailnet API credential storage foundation
platforms: [linux, macos, windows]
cost: none
destructive: yes       # synthetic credential and metadata removal
secrets: [synthetic enrollment auth keys and API credentials in isolated stores, operator-approved staged API credential read-only in optional R01]
owner: peters
---

# Tailnet API credential store test procedure

## 1. Purpose

This procedure tests independent API credential storage with synthetic local inputs.
It proves the staged-entry contract and recovery at interrupted mutation boundaries.

## 2. Applicability

- Candidate: issue #1335, milestone M0c-1A storage foundation.
- The fake store and temporary metadata run without a live OS keychain.
- Platform-specific OS-store adapters need their separate real-keychain qualification.
- This procedure does not test a new UI editor, remote API validity, OAuth
  exchange, enrollment receipts, device deletion or provider cleanup.
- F05 is mandatory. It qualifies the real Linux OS-store adapter with synthetic inputs.
- F05 uses the checked-in fixture runner and ignored Linux test.
  A normal workspace test run skips that test. A skip is not an F05 pass.
- F05 supports Linux x86_64. Other real OS-store adapters need separate qualification.
- R01 is optional and requires the operator's explicit authorization for the
  exact existing desktop staged binding. It reads stored kind only; it does not
  expose the value or test API access. The required synthetic lane cannot qualify R01.

## 3. Safety

> **CAUTION:** REMOVE ONLY TASK-OWNED SYNTHETIC CREDENTIALS AND METADATA.
> Removal of another binding can stop enrollment or API access.

The required tests use fake credentials and task-owned temporary files.
They send no real secret and create no provider resource.
Do not use the operator's staged token or shared OS store for the synthetic lane.
R01 belongs to the authorized desktop lane and must stay separate in the evidence.

## 4. Equipment and preconditions

- A checkout of the candidate commit.
- The repository Rust toolchain.
- A private evidence directory outside the repository.

F05 also needs:

- Python 3 and a healthy local Docker daemon.
- A Linux x86_64 host with the repository Rust toolchain.
- A hydrated checkout with its required Git LFS assets.
- `/usr/bin/dbus-daemon`, `/usr/bin/gnome-keyring-daemon` and `/usr/bin/gdbus`.
- The host runtime libraries at `/usr/lib/x86_64-linux-gnu`.
- The fixed runtime image from the command below.

The runtime image supplies OS packages only. It is not the candidate worker image.
Its use does not qualify a worker image, marker, enrollment or provider lane.

## 5. Setup

1. Record the candidate commit and tool versions in the private evidence.

   Result: The record identifies the exact candidate inputs.

2. Open a terminal at the candidate checkout root.

   Result: Each command below uses the candidate source.

## 6. Tasks

### 6.1 F01 — Staged credentials and status

1. Run the API storage tests.

   ```sh
   cargo test -p horizon-cloud tailnet::api
   ```

   Result: A staged personal token works without metadata migration.
   Status returns kind only. Missing, locked and invalid stored data remain distinct.
   Unknown, contradictory, null and oversized fields fail without secret output.

### 6.2 F02 — Replacement, removal and recovery

1. Examine the replacement and interruption results from F01.

   Result: Every personal/OAuth replacement preserves the published generation.
   Recovery removes only the exact new or retired slots that its phase permits.

2. Examine the removal results from F01.

   Result: Removal suppresses staged fallback, also after interrupted cleanup.
   Repeated removal cleans the exact staged account without reviving it.

3. Examine the metadata and namespace results from F01.

   Result: Metadata has no token or client secret.
   Forged journals and conflicting metadata cannot retire another network's slot.

### 6.3 F03 — Independent enrollment keys

1. Run the complete tailnet regression tests.

   ```sh
   cargo test -p horizon-cloud tailnet
   ```

   Result: API rotation preserves auth keys. Auth-key rotation preserves API bindings.
   Saved-network removal cleans both exact bindings. Existing catalog data stays readable.
   Committed catalog deletion prunes only the removed network's disabled record.
   An interrupted deletion keeps its journal until metadata cleanup succeeds.
   Repeated create/delete cycles do not exhaust API metadata; saved-network
   tombstones and sibling bindings remain intact.

2. Run the core tailnet adapter tests.

   ```sh
   cargo test -p horizon-core tailnet
   ```

   Result: Enrollment still uses the auth key. API secrets do not enter worker exchanges.

### 6.4 F04 — Bundled policy guidance

1. Run the skill coverage check.

   ```sh
   python3 scripts/check-horizon-mcp-skills.py
   ```

   Result: Both bundles describe desktop-only API storage and nonsecret stored presence.
   Neither bundle claims a shipped API editor or automatic node cleanup.

### 6.5 F05 — Isolated real OS-store adapter

> **CAUTION:** USE A NEW PRIVATE EVIDENCE DIRECTORY.
> Do not reuse another task's state, credentials or build output.

The commands below show example paths. Each run needs its own new private evidence path.

1. Get the fixed OS runtime image.

   ```sh
   docker pull ghcr.io/peters/horizon-worker-base@sha256:6550aacb6c60187cd346a795beb3c8bffd998fe9b95ec74f5fb844cf78870bdc
   ```

   Result: Docker has the fixture runtime. This is not worker image qualification.

2. Run the checked-in fixture from the candidate checkout root.

   ```sh
   python3 scripts/tailnet-api-store-fixture/run.py --evidence-dir /tmp/horizon-tailnet-api-f05-first
   ```

   Result: The runner builds and freezes the candidate's Linux library test executable.
   It uses this command automatically:

   ```sh
   cargo test -p horizon-cloud --lib --no-run --locked --message-format=json
   ```

   The runner records source and artifact identities in private evidence.
   It runs the ignored `qualified_synthetic_store` test explicitly for each phase.
   Ordinary tests that ignore this test do not satisfy F05.

   The optional `--target-dir` argument selects a task-owned build cache:

   ```sh
   python3 scripts/tailnet-api-store-fixture/run.py --evidence-dir /tmp/horizon-tailnet-api-f05-first --target-dir /tmp/horizon-tailnet-api-f05-target
   ```

   The fixture uses `--network none`, a read-only container, dropped capabilities
   and `no-new-privileges`. Its state, home, bus, control directory and unlocked
   collection belong only to this run. It mounts no host home, desktop bus,
   runtime socket or graphical session. Bus service activation stays disabled.
   Read-only mounts supply the frozen executable, fixture script and OS runtime tools.

3. Examine `result.json` in the run directory printed by the runner.

   Result: The phases run in this order:

   | Phase | Expected result |
   |---|---|
   | `exercise` | Missing, personal save, OAuth replacement, API removal and another binding's stored kind pass. Synthetic auth bytes remain equal in zeroizing storage. Metadata contains no secret. |
   | `probe` | The first persisted kind check passes. |
   | `unavailable` | The retained fixture keyring stops. An active stored entry reports unavailable; a disabled entry stays known missing. |
   | `probe` | The owned keyring restarts with the same password and restores the persisted stored kind. |
   | `cleanup` | Only the declared synthetic bindings are deleted. The saved catalog is empty. |

   Values do not appear in status output. No desktop prompt or real credential is used.
   The runner creates no provider resource or tailnet node and sends no Tailscale API request.

4. Examine `ownership.json` and `result.json` in that run directory.

   Result: The owned Secret Service is empty and its retained processes stop.
   A healthy Docker daemon reports that the exact fixture container is absent.
   The runner removes only its owned synthetic state and password after that proof.
   Logs, source identities and frozen artifact identities remain in private evidence.
   An unavailable Docker daemon is not proof that the container is absent.

5. If a phase or cleanup fails, keep the evidence and ownership record.

   Result: Logs and ownership records remain private.
   A failed phase does not qualify F05, even if container cleanup succeeds.
   Remaining state stays private when container absence is uncertain.
   Remove only the recorded fixture container and its owned state after its absence is proved.
   Do not use a broad Docker prune or stop another keyring daemon.
   Do not replace the fixture bus with the operator's desktop bus.

6. Repeat F05 for the first reviewable and final candidate inputs.

   Result: Each result identifies its exact source and frozen executable.
   Reuse is permitted only after the repository's input comparison rules are satisfied.
   A private helper from another checkout does not qualify this fixture.

See [the fixture README](../../../scripts/tailnet-api-store-fixture/README.md)
for the runner scope and dependencies. F05 does not qualify R01 or native UI status.

### 6.6 R01 — Optional authorized desktop staged resolver

> **CAUTION:** READ ONLY THE OPERATOR-APPROVED STAGED BINDING.
> Do not copy, print, replace or remove an existing desktop credential.

1. Record authorization for the exact saved local tailnet ID and desktop
   OS-store service. Check nonsecret metadata first: the selected ID must be a
   staged binding without a managed active/disabled API record or pending intent.

   Result: This lane cannot bypass a disabled binding or revive an older staged
   credential. A locked or uncertain store stops this lane without mutation.

2. Build a read-only private qualification helper against the exact candidate.
   Use a task-owned catalog containing only that approved ID and name, with no
   managed metadata or journal. Connect through the authorized desktop OS-store
   adapter. Call only `api_credential_status` and print missing/stored kind.

   Result: The already staged personal token resolves as stored personal_token
   without another paste, metadata migration, secret output or network call.
   The helper has no save, remove, deployment or raw-secret read command.

3. Remove only the task-owned catalog/helper state and keep the nonsecret result.

   Result: The original desktop catalog, staged entry and access policy remain
   unchanged. Record this result separately from F01 through F05.

## 7. Pass criteria

- F01 through F05 pass on the exact candidate inputs.
- F05's explicit runner completes all five phases and proves exact owned cleanup.
- An ignored test or ordinary matrix result cannot substitute for F05.
- R01 is recorded as passed, deferred or not applicable, separately from synthetic proof.
- Stored presence does not claim credential validity, scopes or network access.
- No synthetic test reads a real credential. R01 reads only the authorized staged
  entry through the private resolver and changes no credential or tailnet access.
- The foundation has no visible UI change; no new feature GIF applies to this PR.

## 8. Cleanup

1. Keep the command logs in the private evidence directory.

   Result: Temporary fake-store data disappears through each test's cleanup.
   The operator's OS keychain and real credentials remain unchanged.

## 9. Record of results

Put the command results in the pull request.
Record fake-store results separately from any authorized OS-store qualification.
Use [the report template](../reports/TEMPLATE.md) when the run must be kept.
