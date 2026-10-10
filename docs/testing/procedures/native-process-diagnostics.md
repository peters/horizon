---
procedure: native-process-diagnostics
feature: native process failure logs
platforms: [linux]
cost: none
destructive: yes
secrets: none
owner: peters
---

# Native process failure log test procedure

## 1. Purpose

Test finite failure causes in the actual guardian's private backend log.
Test the shared file limit, first cause, ownership checks and cleanup.

## 2. Applicability

This procedure tests the native-process library on a Unix host.
The recorded run used Linux and synthetic local commands.
It does not qualify paid devices or the host adapter's integration.

## 3. Equipment and preconditions

Use the candidate source and its trusted guardian executable.
Use an owned private temporary directory for `TMPDIR`, `TEMP` and `TMP`.
Use a fresh private evidence directory for each test command.
Set `HORIZON_PROCESS_TEST_EVIDENCE_DIR` to that directory to retain the synthetic logs.
Keep all evidence private.

## 4. Tasks

> **CAUTION:** RUN ONLY THE OWNED SYNTHETIC FIXTURES.
> The tests start local child processes and delete their owned temporary directories.

### 4.1 PROCESS-LOG-01 — Run the process tests

1. Run the tests in the candidate worktree.

   ```bash
   RUSTFLAGS="-D warnings" cargo test -p horizon-app-process
   ```

   Result: All process tests pass without paid allocation or real credentials.

### 4.2 PROCESS-LOG-02 — Examine the actual guardian logs

1. Examine the retained log from the normal close test.

   Result: The original finite host cause occurs once after guardian closure.

2. Examine the retained log from the startup journal failure test.

   Result: The original cause occurs once, and the declared child did not start.

3. Examine the retained log from the concurrent output test.

   Result: Child stdout, child stderr and the host cause share a file no larger than 4 MiB.

4. Examine each private marker and terminal receipt.

   Result: The marker identifies the exact cause bytes, and the receipt records complete cleanup.

### 4.3 PROCESS-LOG-03 — Examine the boundary regressions

1. Examine the tamper, partial-write and repeated-fsync test results.

   Result: Uncertain identity or durability cannot acknowledge retained diagnostics.

2. Examine the finite-cause round-trip tests.

   Result: Unknown text is refused, and uncategorized I/O errors use the finite `Other` code.

3. Examine the temporary directory test.

   Result: The guardian uses the trusted caller's directory, and the child uses its owned task directory.

## 5. Pass criteria

All process tests pass. Each actual log preserves its first cause once.
The concurrent log stays within 4 MiB. All three terminal receipts record complete cleanup.
The boundary tests refuse unconfirmed retention and preserve the original failure.

## 6. Cleanup and evidence

The fixtures close their own children and remove their temporary directories.
Keep the copied logs, markers, receipts, source hashes and test results in the private evidence directory.
Publish only the reviewed summary.
