---
procedure: snapshot-panel-text
feature: Snapshot and placeholder panel text
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Snapshot panel text test procedure

## 1. Purpose

This procedure tests that a snapshot panel keeps its saved text and title.
A snapshot panel is a restore failure placeholder, a cloud placeholder, a parked cloud member or a disconnected SSH snapshot.
Each one starts a short process only because a terminal needs a PTY.
The output of that process must not change the panel.

## 2. Applicability

- Candidate: each build that changes terminal spawn, PTY output handling or snapshot panels.
- Platforms: Linux, macOS and Windows.
  On Windows, ConPTY clears the screen and sets a console title when the process starts.
  Windows is the platform that shows a fault in this area.
- This procedure does not test: live terminals, cloud workers or SSH connections.

## 3. Equipment and preconditions

- A checkout of the candidate commit.
- The Rust toolchain from the repository Quick Start.
- On Linux, a writable private `TMPDIR` outside `/tmp` if `/tmp/.git` exists.

## 4. Setup

1. Go to the root of the candidate checkout.

   Result: `cargo metadata --no-deps` shows the `horizon-core` and `horizon-ui` packages.

## 5. Tasks

### 5.1 SNAP-CORE: Snapshot terminal

1. Run the snapshot terminal tests.

   ```bash
   cargo test -p horizon-core terminal::lifecycle::snapshot_tests
   ```

   Result: `a_snapshot_keeps_its_replay_whatever_its_process_prints` passes.
   The process clears the screen and sets a title, but the snapshot shows only its saved text and saved title.
   On Linux and macOS, `a_live_terminal_shows_what_its_process_prints_over_the_replay` also passes.
   That test shows that the same process clears a live terminal.

### 5.2 SNAP-UI: Restore failure placeholder

1. Run the Device panel restore test.

   ```bash
   cargo test -p horizon-ui app::device_tests::invalid_restored_device_displays_its_failure_transcript
   ```

   Result: The test passes. The painted panel shows "Device panel requires".

2. On Windows, run the test from step 1 ten times.

   Result: All runs pass.
   An earlier fault let ConPTY erase the text before the first frame on some runs.

## 6. Pass criteria

- SNAP-CORE and SNAP-UI pass on each platform in the candidate run.
- On Windows, all ten SNAP-UI runs pass.

## 7. Cleanup

1. Do nothing. The tests remove their temporary files.

   Result: No test process stays alive.

## 8. Record of results

Put the results in the pull request. CI runs both tasks on Linux, macOS and Windows.
