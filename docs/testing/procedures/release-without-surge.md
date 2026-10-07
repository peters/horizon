---
procedure: release-without-surge
feature: release assets and application updates
platforms: [linux, macos, windows]
cost: none
destructive: no
secrets: none
owner: peters
---

# Release assets and application updates test procedure

## 1. Purpose

This procedure tests Horizon after removal of the Surge updater and installers.
It tests the release guard, toolbar, startup and saved state.

## 2. Applicability

- Candidate: The current pull request head.
- Platforms: Linux, macOS and Windows.
- This procedure does not publish a release or change an existing installation.

## 3. Equipment and preconditions

- A Rust toolchain and the platform build prerequisites.
- Python 3 and Bash for the release guard tests.
- A task-owned isolated desktop and a live Horizon native VNC Device panel.
- A frozen candidate executable, its SHA256 hash and private application state.
- Synthetic terminal content only.

## 4. Setup

1. On Linux, run the [isolated desktop fixture](../../../scripts/device-smoke/README.md) with the candidate executable and `--native-view`.

   Result: The fixture starts a private desktop and gives a VNC address.

2. On macOS or Windows, start the frozen candidate in a dedicated test machine, VM or isolated desktop session.

   Result: The candidate uses private application state.

3. On macOS or Windows, expose that session through VNC and the authorized SSH forwarding path.

   Result: The native Device panel can reach that session. If no isolated target exists, record that platform as blocked.

4. Create a native Device panel for that address.

   Result: The panel shows the candidate desktop.

5. Examine three panel inspections during the fixture heartbeat.

   Result: Displayed frames advance. Record the executable hash and panel inspections.

## 5. Tasks

### 5.1 RELEASE — Release guard

1. Run the release guard tests:

   ```sh
   python3 -B scripts/test_release_github.py -v
   python3 -B scripts/test_detect_ci_build_inputs.py -v
   ```

   Result: All tests pass. A stable release needs four executable assets and `SHA256SUMS.txt`.
   A missing executable keeps the release in draft state. Installers are not required.

2. Examine `.github/workflows/release.yml`.

   Result: The workflow builds four executable assets. It does not build Surge or write update packages.
   Homebrew and WinGet use the executable assets and their hashes.

### 5.2 UI — Startup and toolbar

1. Examine the candidate after startup.

   Result: The board, terminals and toolbar appear. No Update button appears.

2. Open Settings.

   Result: Settings opens. No installer download prompt appears.

3. Close Settings.

   Result: The board appears again.

4. Resize the candidate window to its minimum width.

   Result: Quick Nav, Sessions and Settings remain accessible. Secondary actions remain accessible through More.

5. Restore the original window size.

   Result: The toolbar and board fit the window. Record screenshots and a short video of this flow.

### 5.3 STATE — Saved state and old installation metadata

1. Start the frozen candidate with private saved workspace state.

   Result: The candidate restores the workspace. The removal does not change the configuration or session format.

2. Place a synthetic `.surge/runtime.yml` beside a separate private copy of the executable.

   Result: The fixture contains old installation metadata without real credentials or repository names.

3. Start that copy in the isolated desktop.

   Result: Horizon starts without an updater or installer prompt. The metadata does not change startup behavior.

4. Examine the private configuration and session files.

   Result: The candidate preserves their contents. Horizon does not remove old installer data.

## 6. Pass criteria

- The release guard tests pass, including the missing-executable case.
- No active Surge dependency, packaging step or updater remains.
- Startup, toolbar actions, resize and saved workspace state work.
- The native Device panel shows advancing frames during the UI run.
- The recording shows the tested flow on the frozen candidate executable.

## 7. Cleanup

1. Close the candidate window normally.

   Result: The task-owned application exits.

2. Close the task-owned Device panel.

   Result: Other panels remain open.

3. Stop the task-owned fixture.

   Result: The fixture stops only its own processes.

## 8. Record of results

Put the head commit, executable hash, test results and UI evidence in the pull request.
Record each blocked platform lane separately. Keep private evidence out of the repository.
