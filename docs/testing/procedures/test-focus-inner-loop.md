---
procedure: test-focus-inner-loop
feature: focused test selection for the development loop
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Test focus inner loop test procedure

## 1. Purpose

This procedure checks that `scripts/test-focus.py` selects the tests of the crate and module that a change touches.
It also checks that the script reports a selection that covers no test, and that it names slow tests when `cargo nextest` is available.

## 2. Applicability

- Candidate: A checkout of the commit that adds `scripts/test-focus.py`.
- Platforms: Linux and macOS. The script needs Python 3.11 or later for `tomllib`.
- This procedure does not test the full local matrix. The full matrix stays the check of a candidate head.

## 4. Equipment and preconditions

- A checkout of the candidate commit.
- Python 3.11 or later on `PATH`.
- A built test profile of `horizon-core` for the short result times. A cold build takes longer and is not a failure.
- Optional: `cargo-nextest`, to run the tasks that use it.

## 5. Setup

1. Open a terminal in the candidate checkout.

   Result: `Cargo.toml` names the Horizon workspace.

2. Run `python3 -B scripts/test_test_focus.py`.

   Result: the last line is `OK`.

## 6. Tasks

### 6.1 TF-01 — Print the selection of one module

1. Run `python3 scripts/test-focus.py --print crates/horizon-core/src/board/mod.rs`.

   Result: the script prints `horizon-core: lib -> board::` and one command that contains `-p horizon-core --lib board::`.
   The script starts no build.

### 6.2 TF-02 — Run the selection of one module

1. Run `python3 scripts/test-focus.py crates/horizon-core/src/board/tests/mod.rs`.

   Result: the script prints the command, and the run ends with `test result: ok.` and a passed count above zero.
   The output also contains `filtered out`, so the run did not select the other tests of the crate.

### 6.3 TF-03 — Report a filter that selects no test

1. Run `python3 scripts/test-focus.py crates/horizon-core/src/error.rs`.

   Result: the script prints `warning: the filter selected no test in horizon-core` and exits with status 0.
   The task fails if the script reports the run as a test failure.

### 6.4 TF-04 — Report a path that needs the full matrix

1. Run `python3 scripts/test-focus.py --print Cargo.lock`.

   Result: the script prints `skipped Cargo.lock: a workspace-wide file: run the full matrix`.
   The script starts no build.

### 6.5 TF-05 — Cover a whole crate

1. Run `python3 scripts/test-focus.py --print --all crates/horizon-core/src/board/mod.rs`.

   Result: the printed command has no module filter and contains `-p horizon-core --lib`.

### 6.6 TF-06 — Select the binary target of a crate that has no library

1. Run `python3 scripts/test-focus.py --print crates/horizon-ui/src/app/browser_requests.rs`.

   Result: the script prints `horizon-ui: bin horizon -> app::browser_requests::` and the command contains `--bin horizon`.

### 6.7 TF-07 — Use nextest when it is installed

1. Run `cargo nextest --version`.

   Result: the command prints a version. If it fails, skip this task and record it as not applicable.

2. Run `python3 scripts/test-focus.py --print crates/horizon-core/src/board/mod.rs`.

   Result: the printed command starts with `cargo nextest run` and contains `-E test(board::)`.

### 6.8 TF-08 — Name the slow tests of a crate

1. Run `cargo nextest run -p horizon-core --lib`.

   Result: the summary names the test count, and each test above 20 seconds has a `SLOW` line.
   The task fails if a test above that period has no `SLOW` line.

## 7. Pass criteria

- TF-01 to TF-06 pass without nextest installed.
- TF-07 names the nextest command when nextest is installed.
- No task changes a file outside `target/`.

## 8. Cleanup

1. Run `git status --porcelain`.

   Result: the candidate files are the only changes. These tasks write only to `target/`.

## 9. Record of results

Put the results in the pull request. Write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md) only if the run must be kept.
