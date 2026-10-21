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
A profile host keeps named browser profiles for connect clients.
Each connect client controls only its own pages.
A new page of a profile host on a shared display must not take the keyboard focus from a person.

## 2. Applicability

- Candidate: a source build of `horizon-browser` that has the profile host library.
  The `host` command is not part of this candidate, so the tasks use the regressions only.
- Platforms: Linux and macOS. The mode checks of files apply only to Unix.
- This procedure does not test: remote targets, the Horizon desktop host or a Windows computer.

## 3. Safety

The tasks of this version run only regressions. They rent no compute, delete no data,
send no secret and change no access.

## 4. Equipment and preconditions

- The source of the candidate and the Rust toolchain of the repository.

## 5. Setup

1. Build the test executables of the candidate.

   ```sh
   cargo test -p horizon-browser --lib --no-run
   cargo test -p horizon-browser-cli --lib --no-run
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

### 6.2 PH-FILE: The host file

1. Run the regressions of the host file.

   ```sh
   cargo test -p horizon-browser-cli --lib standalone::profile::tests::a_host_file
   cargo test -p horizon-browser-cli --lib standalone::profile::tests::user_js
   cargo test -p horizon-browser-cli --lib standalone::profile::tests::a_preference
   ```

   Result: The tests pass. The profile host refuses a host file with one of these problems:
   - a profile name that is not 1 to 40 of `a-z`, `0-9` and `-`;
   - an engine other than `chromium` or `firefox`, or an unknown field;
   - an argument that the engine manages, for example `--user-data-dir` or `--headless`;
   - a control character in an argument or a preference;
   - more than 32 browser profiles, or more than 256 KiB;
   - write permission for the group or other users.

   A Firefox preference that the host file no longer names gets its default again at the next start.

### 6.3 PH-RULES: The rules of a create

1. Run the regressions of the create rules, the page IDs, the idle pages and the host record.

   ```sh
   cargo test -p horizon-browser-cli --lib standalone::profile::tests
   ```

   Result: The tests pass. These rules apply:
   - A create must name a browser profile of the host file. A create with no target, a remote target or a backend is refused.
   - A duplicate opens only from a page of the same agent.
   - One agent has a maximum of 4 pages. The host has a maximum of 16 pages.
     Only an agent with no page gets the 16th page.
   - A page ID is `profile-<name>-` and 12 hexadecimal digits.
   - A page with no action for the idle limit closes.
   - One profile host runs on a browser runtime root. It keeps its host instance after a restart.

## 7. Pass criteria

- Every result in section 6 is as written.

## 8. Cleanup

No cleanup is necessary. The tests remove their temporary folders.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
