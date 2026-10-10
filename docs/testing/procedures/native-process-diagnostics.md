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

- Candidate: The approved native-process source commit.
- Platform: Linux with a private temporary directory in `/dev/shm`.
- This procedure does not test paid devices or the host adapter's integration.

## 3. Safety

> **CAUTION:** RUN ONLY THE OWNED SYNTHETIC FIXTURES.
> The tests start local child processes and delete their owned temporary directories.

Keep the copied logs, markers and receipts private. Publish only the reviewed summary.

## 4. Equipment and preconditions

- A clean worktree at the approved candidate commit.
- The repository's Rust toolchain and Python 3 on `PATH`.
- A dedicated Bash shell with the candidate worktree as its current directory.
- At least 32 MiB of free space in `/dev/shm`.
- No paid provider or credential is required.

## 5. Setup

1. Check the candidate commit.

   ```bash
   git rev-parse HEAD
   ```

   Result: The commit matches the approved candidate.

2. Set private file permissions for this dedicated shell.

   ```bash
   umask 077
   ```

   Result: New evidence files have private permissions.

3. Create this run's temporary directory.

   ```bash
   PROCESS_LOG_TMP=$(mktemp -d /dev/shm/horizon-process-log.XXXXXX)
   ```

   Result: The owned directory has permission mode 700.

4. Set the temporary variables to that directory.

   ```bash
   export TMPDIR="$PROCESS_LOG_TMP" TEMP="$PROCESS_LOG_TMP" TMP="$PROCESS_LOG_TMP"
   ```

   Result: The guardian uses the trusted private directory.

5. Create a fresh private evidence directory.

   ```bash
   PROCESS_LOG_EVIDENCE=$(mktemp -d /var/tmp/horizon-process-log-evidence.XXXXXX)
   ```

   Result: No earlier evidence files can conflict with this run.

6. Set the evidence directory for the tests.

   ```bash
   export HORIZON_PROCESS_TEST_EVIDENCE_DIR="$PROCESS_LOG_EVIDENCE"
   ```

   Result: The selected synthetic fixtures retain their private logs, markers and receipts.

## 6. Tasks

### 6.1 PROCESS-LOG-01 — Run the process tests

> **CAUTION:** RUN ONLY THIS CANDIDATE'S OWNED SYNTHETIC FIXTURES.
> The tests start local children and delete their owned temporary directories.

1. Run the process tests once with the fresh evidence directory.

   ```bash
   RUSTFLAGS="-D warnings" cargo test -p horizon-app-process -- --test-threads=4 \
     > "$PROCESS_LOG_EVIDENCE/test-results.log" 2>&1
   ```

   Result: The command returns success. All process tests pass without paid allocation or real credentials.

### 6.2 PROCESS-LOG-02 — Examine the actual guardian logs

1. Examine the normal close log.

   ```bash
   rg -n 'app_host_unavailable' "$PROCESS_LOG_EVIDENCE/after-close-output.log"
   ```

   Result: `app_host_unavailable: Guardian: I/O BrokenPipe` occurs once after guardian closure.

2. Examine the startup journal failure log.

   ```bash
   rg -n 'app_host_unavailable' "$PROCESS_LOG_EVIDENCE/startup-failure-output.log"
   ```

   Result: `app_host_unavailable: Guardian: MissingState` occurs once. The declared child did not start.

3. Validate the three copied markers, logs and terminal receipts.

   ```bash
   python3 - "$PROCESS_LOG_EVIDENCE" <<'VERIFY'
   from pathlib import Path
   import json, sys
   root = Path(sys.argv[1])
   for name in ("after-close", "startup-failure", "concurrent-cap"):
       files = [root / f"{name}-{suffix}" for suffix in
                ("output.log", "host-diagnostic.json", "process.json")]
       data = files[0].read_bytes()
       marker = json.loads(files[1].read_text())
       receipt = json.loads(files[2].read_text())
       frame = data[marker["offset"]:]
       assert frame.startswith(b"\n")
       record = json.loads(frame[1:].splitlines()[0])
       assert record["cause"] == {"Host": marker["cause"]}
       assert marker["operation"] == receipt["operation"]
       assert receipt["complete"]
       assert data.count(record["message"].encode()) == 1
       assert len(data) <= 4 * 1024 * 1024
       assert all(file.stat().st_mode & 0o777 == 0o600 for file in files)
       print(name, len(data), "pass")
   VERIFY
   ```

   Result: Each marker matches its exact cause record. Each receipt confirms cleanup. The shared physical limit stays within 4 MiB.

### 6.3 PROCESS-LOG-03 — Examine the boundary regressions

1. Examine the process test results.

   ```bash
   cat "$PROCESS_LOG_EVIDENCE/test-results.log"
   ```

   Result: The tamper, partial-write and repeated-fsync tests refuse unconfirmed retention.
   Finite causes survive serialization. Uncategorized I/O errors use `Other`.
   The guardian uses the trusted directory. The child uses its separate owned task directory.

## 7. Pass criteria

- All process tests pass.
- Each actual log preserves its first cause once.
- The concurrent log stays within 4 MiB.
- All three terminal receipts record complete cleanup.
- The boundary tests refuse unconfirmed retention and preserve the original failure.

## 8. Cleanup

The fixtures close their own children and remove their temporary directories.
Keep the copied synthetic evidence for review.

> **CAUTION:** REMOVE ONLY THIS RUN'S OWNED TEMPORARY DIRECTORY.
> Do not delete an unknown directory or evidence from another run.

1. Remove the empty temporary directory.

   ```bash
   rmdir -- "$PROCESS_LOG_TMP"
   ```

   Result: Only this run's empty directory is removed. Unexpected remaining content causes refusal.

2. Exit the dedicated shell.

   ```bash
   exit
   ```

   Result: The caller's original environment remains intact.

## 9. Record of results

Write a report in `docs/testing/reports/` with the report template.
Record the candidate commit, source or binary hash, test results, deviations and cleanup.
Keep the logs, markers, receipts and source hashes in the private evidence directory.
