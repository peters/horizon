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
- F05 qualifies a task-owned real Linux OS-store adapter with synthetic inputs.
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

1. Prepare a task-owned Secret Service with its own home, data directories,
   control directory, bus and unlocked collection. Use no host home, runtime,
   desktop socket or bus mount. Disable bus service activation for this lane.

   Result: The real adapter can use only this run's synthetic collection.

2. Compile a private helper against the exact candidate's shared store. Save
   synthetic enrollment keys and personal/OAuth API bindings. Read only stored
   kind through `api_credential_status`. Compare only the owned synthetic auth
   bytes in zeroizing storage to check their independence. Print no value.

   Result: Personal save, OAuth replacement and API removal preserve the auth
   binding. A second tailnet's kind remains unchanged. JSON contains no secret.

3. Stop only the retained fixture keyring process. Read an active stored entry.
   Restore the same fixture daemon and password, then read again.

   Result: Active storage reports unavailable while the service is absent.
   It reports its stored kind after restart. An explicitly disabled entry remains
   known missing without an OS-store read. No desktop prompt occurs.

4. Delete only the declared synthetic saved bindings. Check the owned Secret
   Service for remaining items. Stop the owned daemons and container, then remove
   the private fixture credentials and password.

   Result: The owned service is empty. No fixture process or credential remains.
   The operator's OS store stays unchanged.

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
