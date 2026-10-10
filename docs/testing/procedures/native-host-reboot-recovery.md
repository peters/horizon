---
procedure: native-host-reboot-recovery
feature: Native host reboot recovery
platforms: [linux]
cost: none
destructive: yes
secrets: original client credential reference only; values stay in the current user keyring
owner: peters
---

# Native host reboot recovery test procedure

## 1. Purpose

Test exact local recovery after a Linux reboot with synthetic fixtures.
Test status inspection without changes to an existing journal or owner binding.

## 2. Applicability

- Candidate: A final Horizon checkout with the reboot recovery changes.
- Platform: Linux with the required Rust toolchain and local build prerequisites.
- The fixtures use synthetic account values and task-owned private directories.
- This procedure does not test paid devices, provider deletion, or endurance.
- macOS has no automatic reboot proof.

## 3. Safety

> **CAUTION:** PRESERVE SHARED STATE AND PROCESSES.
> A real reconciliation command can stop resources and change the private journal.
> This procedure uses synthetic fixtures and read-only status only.

Do not reboot a shared computer for this procedure.
Real recovery requires separate approval for the exact original resources.
Use [the operator procedure](../../architecture/remote-device-testing.md#recover-local-resources-after-a-linux-reboot) for that operation.

## 4. Equipment and preconditions

- Use an isolated final worktree with all local prerequisites from `AGENTS.md`.
- Use a task-owned build target and private test directory.
- Set the private test directory mode to `0700`.
- Use private shared memory for `TMPDIR`, `TEMP`, and `TMP`; do not use `/tmp`.
- Set `CARGO_TARGET_DIR` to the exclusively owned build target.
- Set `CARGO_BUILD_JOBS=4`, `RUST_TEST_THREADS=4`, and `CARGO_INCREMENTAL=0`.
- Preserve the source hash, candidate binary hash, and command results.
- For optional status inspection, use the original existing client and owner.
- Unlock the current user keyring before optional status inspection.
- Use that user’s `DBUS_SESSION_BUS_ADDRESS` and `XDG_RUNTIME_DIR`.
- Status reads the selected credential reference through the current user keyring.
- Keep secret values and private identities out of public evidence.

## 5. Setup

1. Record the candidate commit and source patch hash.

   Result: The private manifest identifies the exact source.

2. Set the task-owned build target and private test directory variables.

   ```bash
   export CARGO_TARGET_DIR="/absolute/path/to/task-owned-target"
   export TMPDIR="$(mktemp -d /dev/shm/horizon-recovery-tests.XXXXXX)"
   export TEMP="$TMPDIR"
   export TMP="$TMPDIR"
   export CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 CARGO_INCREMENTAL=0
   ```

   Result: Test fixtures cannot change another task's cache or private state.

3. Set the original user's D-Bus and runtime directory for optional status inspection.

   ```bash
   export XDG_RUNTIME_DIR="/run/user/$(id -u)"
   export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
   ```

   Result: The optional command uses the current user keyring.

4. Select the absolute frozen candidate path.

   ```bash
   recovery_native="/absolute/path/to/frozen/horizon-native"
   ```

   Result: The test uses the selected candidate, not a command from `PATH`.

5. Record the frozen candidate hash.

   ```bash
   sha256sum "$recovery_native"
   ```

   Result: Private evidence identifies the exact executable.

## 6. Tasks

### 6.1 RECOVERY — Synthetic local recovery

> **CAUTION:** USE ONLY TASK-OWNED FIXTURE STATE.
> These tests stop synthetic processes and delete their temporary state.

1. Run the focused recovery tests.

   ```bash
   cargo test -p horizon-app-host --lib local::recovery -- --test-threads=1
   cargo test -p horizon-app-process --lib boot::tests
   ```

   Result: An earlier boot ID releases only the exact owned local resource.
   A reused process ID receives no signal.
   The original receipt stays unchanged.
   A private recovery record retains the boot proof.

### 6.2 REFUSAL — Unsafe or incomplete recovery proof

1. Examine the results from the recovery fixtures.

   Result: Missing boot IDs require an exact operator confirmation.
   Current boot receipts and live legacy processes stay uncertain.
   Invalid kernel boot IDs, mismatched ownership, and invalid IDs cause refusal.
   Complete operations cannot receive a new operator confirmation.
   Missing or extra pending `run` and `tunnel` IDs cause refusal before cleanup.
   This check includes records with no dispatched resource.

### 6.3 EXISTING — Existing state and original ownership

> **CAUTION:** USE ONLY TASK-OWNED FIXTURE STATE.
> These tests stop synthetic processes and delete their temporary state.

1. Run the existing-state fixtures.

   ```bash
   cargo test -p horizon-app-host --lib bootstrap::tests::reboot_reconciliation -- --test-threads=1
   cargo test -p horizon-app-host --lib bootstrap::tests::normal_reconciliation -- --test-threads=1
   cargo test -p horizon-app-runtime --lib journal::store::tests -- --test-threads=1
   ```

   Result: Missing journals and owner bindings stay missing.
   Status inspection preserves file contents, identities, modes, and modification times.
   Explicit reboot recovery refuses missing state before writes.
   Active leases and foreign owners, roots, or credential realms cause refusal.
   Ordinary reconciliation retains its normal initialization behavior.

### 6.4 STATUS — Optional original-owner inspection

1. Record every entry's hash, inode, mode, size, and modification time in the original private state.

   Result: Private evidence identifies the unchanged state before inspection.

2. Run the frozen candidate's read-only status command with the original client.

   ```bash
   "$recovery_native" --reconcile-status --client "/absolute/path/to/original-private-client.json"
   ```

   Result: The command reports the current boot ID and original owner's pending operations.
   It does not create state or clean up resources.

3. Compare the complete private state with the recorded snapshot.

   Result: Every recorded entry has the same content and metadata.

### 6.5 MATRIX — Final-checkout validation

> **CAUTION:** USE ONLY TASK-OWNED FIXTURE STATE.
> These tests stop synthetic processes and delete their temporary state.

1. Run all pre-push validation commands from `AGENTS.md` in the final worktree.

   Result: All required commands pass.
   Record the advisory pedantic result separately.

## 7. Pass criteria

- Exact synthetic recovery preserves original receipts and does not signal reused PIDs.
- Invalid or incomplete proof causes refusal before cleanup.
- Status inspection preserves existing state and does not initialize missing state.
- The final checkout passes every required validation command.
- Any optional original-owner inspection preserves every recorded state entry.

## 8. Cleanup

1. Keep the original private state and receipts.

   Result: Later inspection retains the original proof.

> **CAUTION:** STOP ONLY TASK-OWNED FIXTURE RESOURCES.
> Other resources can belong to an active test or another person.

2. Stop only synthetic resources that the test fixture created.

   Result: The fixture leaves no owned process running.
   No real recovery or provider cleanup repeats.

## 9. Record of results

Use [the report template](../reports/TEMPLATE.md) for a retained run.
Record synthetic recovery, historical recovery, and current read-only status separately.
Keep private evidence outside the repository.
