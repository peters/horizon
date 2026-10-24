---
procedure: exposure-management
feature: Original Hetzner SDK exposure management
platforms: [linux, macos]
cost: rents compute
destructive: yes
secrets: [original selected Hetzner credential reference]
owner: peters
---

# Original Hetzner exposure management test procedure

## 1. Purpose

Test original-client policy, journal order, sibling preservation and exact cleanup.
Separate offline algorithm coverage from actual provider results.

## 2. Applicability

- Candidate: the Cloud SDK with the existing `exposure-firewall` feature enabled.
- Offline tasks: all supported SDK platforms.
- Provider tasks: a separately approved task-owned Hetzner CPU worker.
- This procedure does not qualify RunPod, public interfaces, source attribution or physical worker expiry.

Read [the boundary](../../architecture/exposure-management.md) first.

## 3. Safety

Use synthetic credentials for offline tasks. Do not read real credentials in those tests.
The real task requires an original owner and its durable journal.
A saved record or a provider name cannot create that owner.
Do not change foreign resources to test refusal.

## 4. Equipment and preconditions

- Use the repository toolchain and exact candidate source.
- Preserve commands, feature sets, relevant nonsecret environment and results.
- For a real task, record a reviewed consumer and its immutable executable inputs.
- The consumer must call this exact Manager with its original client and durable Journal.
- The consumer must physically close its original Gate on errors and before ingress removal.
- Use the owner's actual pinned SSH policy and exact allocation/server identities.
- Use the original normal provision/delete journal and a separately approved resource/cost cap.

The SDK has no standalone public command for this controller.
The ignored Core test below calls Manager directly with the original normal bindings.
Review its fixed private input and supervisor cleanup before any provider task.
Do not replace those bindings with saved permissions or a credential helper.

## 5. Setup

1. Keep default features and public dispatch unchanged.

   Result: The component has no application or worker activation.

2. Preserve the candidate's exact inputs and current Main comparison.

   Result: Later reuse has an explicit source and feature comparison.

## 6. Tasks

### 6.1 M01 — Offline original-policy transitions

1. Run the management regressions.

   ```bash
   RUSTFLAGS="-D warnings" cargo test -p horizon-cloud --features exposure-firewall hetzner::tests::firewalls::management::
   ```

   Result: Original allocation/server/SSH drift refuses. Foreign policies and sibling contributions survive.
   Intent precedes effects. Returned IDs precede action waits. Uncertain creation is not repeated.
   Exact cleanup refuses unexpected rules and cannot reopen a retired manager.

2. Run the full enabled SDK suite.

   ```bash
   RUSTFLAGS="-D warnings" cargo test -p horizon-cloud --features exposure-firewall
   ```

   Result: Existing exact ownership, numeric CIDR and rule-count regressions pass with the new controller.
   Strict attachment decoding applies only when the feature is enabled.

3. Run the default SDK suite.

   ```bash
   RUSTFLAGS="-D warnings" cargo test -p horizon-cloud
   ```

   Result: The default response compatibility remains unchanged.

4. Run the full applicable matrix and repository checks on stable inputs.

   Result: Required tiers pass. New, reused and non-applicable results remain separate.

### 6.2 M02 — Actual provider and management access

Run this task on the first reviewable head and the final head.
Reuse real results only when the executed behavior and consumed inputs match.

1. Record the reviewed consumer, exact operation, cost cap and existing provider inventory.

   Result: Every original server, volume, SSH key, primary IP, network and firewall has an explicit baseline.

> **CAUTION:** RENT ONLY THE APPROVED WORKER. Compute and storage incur charges until their deletion.

2. Create one task-owned worker through its normal journal.

   Result: Original allocation/server/pins and cleanup identities are recorded before firewall effects.
   The original SSH connection works. No foreign resource supplies test ownership.

> **CAUTION:** CHANGE ONLY THE ORIGINAL TASK FIREWALL. Incorrect policy can remove SSH access or expose services.

3. Run the reviewed ignored consumer with the exact frozen Core test executable.

   The Cargo selector for that executable is:

   ```bash
   cargo test -p horizon-core --lib --features cloud-exposure cloud_runtime::deployment::hetzner::tests::exposure_management::real_original_worker_management_retains_siblings_and_cleans -- --ignored --exact
   ```

   Result: Intent and returned action IDs precede effects and waits.
   On an unrestricted server, the actual approved SSH management rules remain present.
   With foreign firewalls, their complete rules remain unchanged and no extra management allowance appears.

4. Examine actual readbacks and the original SSH connection.

   Result: Only the original owned firewall attaches. Complete sibling rules and original management access remain.
   Record actual source/traffic observations separately; provider policy alone proves no source attribution.

> **CAUTION:** REMOVE ONLY TASK-OWNED INGRESS. Close the original Gate before rules are removed.

5. Examine the same test's service-removal observations.

   Result: The remaining service and management policy survive. Foreign inbound and outbound rules remain unchanged.

> **CAUTION:** DELETE ONLY THE ORIGINAL TASK FIREWALL. Do not adopt a successor or delete a foreign attachment.

6. Examine the same test's original-only cleanup observations.

   Result: Fresh readback shows the exact original firewall absent.
   Foreign attachments remain equal to the baseline. No reopened service or successor controller appears.
   Provider absence does not replace original worker retirement proof.

## 7. Pass criteria

- All applicable offline checks pass on the exact candidate inputs.
- M02 records actual original policy and management access before provider qualification is claimed.
- Lost-reply, failed-save and drift refusal tests remain offline algorithm coverage unless actually observed live.
- The result does not claim full M2 activation, source preservation, RunPod support or physical worker retirement.

## 8. Cleanup

1. Stop only offline test listeners and remove their owned temporary files.

   Result: No offline fixture remains.

> **CAUTION:** DELETE ONLY RESOURCES RECORDED BY THIS RUN. Other resources can contain unrelated work.

2. Delete the task worker through its original normal journal.

   Result: Fresh exact-ID absence and the full baseline comparison show no owned resource remains.
   Preserve uncertainty and the original owner if cleanup fails.

## 9. Record of results

Record exact source and executable inputs, commands, journal ordering, readbacks,
management results, limitations and cleanup. Keep credential values and private
account details out of repository reports. Write a permanent real-input report
with the [report template](../reports/TEMPLATE.md).

## Private actual-test input

The ignored test reads only its fixed `sdk-exposure-management/input.json` under
the task-owned native evidence directory. The directory is canonical and private;
the input is an exclusive regular 0600 file with one link and at most 16 KiB.
Create the original normal cloud inside this directory, not a copied state tree.
The supervisor must review and capture these comparison fields after normal Ready:

- `cloud`, `settings`, `cloud_id`, `allocation`, numeric `server`, `volume` and `key`.
- Fresh `test_operation`, 32 hexadecimal `nonce` and actual `management_source` IP.
- `settings_sha256`, `deployment_sha256`, `normal_journal_sha256`, `known_hosts_sha256`.
- `credential` and `ssh_key` metadata: `path`, `device`, `inode`, `uid`, `size`, `mode`, `changed`, `changed_nsec`.

Never put credential values or private-key contents in this file.
The test compares credential/key metadata without copying their contents.
Only the existing host credential reader opens the provider credential for the SDK.
It checks an actual pinned SSH nonce and connection tuple before and after changes.
It requires an SSH-only worker with no protected ingress, active admission or exposure.
It opens no service socket, so ingress removal has no live test Gate to retire.

The supervisor must watch the same exclusive, fsynced `sdk-journal.jsonl`.
It contains the exact scope before create and received firewall/action IDs before waits.
On uncertain creation, preserve that scope and the pending complete rules.
Do not restart the test or create another Manager to recover a lost response.
Agree exact original cleanup after process loss before execution; retain unresolved
state if that cleanup cannot complete. Normal worker deletion alone is insufficient.
