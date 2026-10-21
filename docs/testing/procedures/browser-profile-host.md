---
procedure: browser-profile-host
feature: Profile host of horizon-browser
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Browser profile host test procedure

## 1. Purpose

This procedure tests the profile host of `horizon-browser`, part by part.
The first part is the background windows of a headed Chromium.
A new page of a profile host on a shared display must not take the keyboard focus from a person.

## 2. Applicability

- Candidate: a source build of `horizon-browser` that has the background windows of a headed Chromium.
- Platforms: Linux and macOS.
- This procedure does not test: remote targets, the Horizon desktop host or a Windows computer.
- This version does not use a real display. The `host` command comes in a later part, with a task on a real display.

## 3. Safety

The tasks of this version run only regressions. They rent no compute, delete no data,
send no secret and change no access.

## 4. Equipment and preconditions

- The source of the candidate and the Rust toolchain of the repository.

## 5. Setup

1. Build the test executables of the candidate.

   ```sh
   cargo test -p horizon-browser --lib --no-run
   ```

   Result: The command succeeds.

## 6. Tasks

Give each task an ID. A report uses the ID to give a result.

### 6.1 PH-WINDOWS: Background windows of a headed Chromium

1. Run the regressions of the launch and of the first page.

   ```sh
   cargo test -p horizon-browser --lib background
   cargo test -p horizon-browser --lib shared_process_pins_profile_and_launch_settings_across_restarts
   cargo test -p horizon-browser --lib initial_target_claim_preserves_empty_startup_fallback
   ```

   Result: The tests pass. A headed launch with background windows starts Chromium with `--no-startup-window`.
   A headless launch does not get this argument.
   The first page of this launch opens in a new window that does not get the focus.
   A headed shared browser keeps the setting of its first launch. A headless browser ignores the setting.

## 7. Pass criteria

- Every result in section 6 is as written.

## 8. Cleanup

No cleanup is necessary. The tasks make no files outside the build folder.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
