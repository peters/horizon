---
procedure: standalone-remote-host
feature: Standalone horizon-browser host for configured remote targets
platforms: [linux, macos]
cost: paid device     # only task SRH-REAL; the other tasks use a loopback provider
destructive: no
secrets: [the username and access key of one remote device provider, in two private files]
owner: peters
---

# Standalone remote host test procedure

## 1. Purpose

This procedure proves that `horizon-browser mcp --standalone --config <host file>` serves the
configured remote targets over MCP, keeps the credential values from the MCP client, and
releases each session on close, at the end of stdin and after a kill.

## 2. Applicability

- Candidate: a build of `horizon-browser` that has the `--config` option.
- Platforms: Linux and macOS. The permission checks of the files apply only to Unix.
- This procedure does not test: local browsers, the Horizon desktop host, or the cloud worker.

## 3. Safety

> **CAUTION:** OPEN ONLY ONE SHORT SESSION AT A PAID PROVIDER IN TASK SRH-REAL.
> Each session uses paid device time. Close it at once when the task is done.

> **CAUTION:** DO NOT WRITE A CREDENTIAL VALUE INTO A COMMAND LINE, A LOG OR A REPORT.
> Put each value in a file that only your user can read, and name only the file.

## 4. Equipment and preconditions

- The `horizon-browser` binary of the candidate.
- For the loopback tasks: the test `tests/standalone_remote.rs` of `horizon-browser-cli`, or a
  WebDriver stand-in on `127.0.0.1`.
- For task SRH-REAL: an account at a remote device provider, with its username and access key.
- An empty runtime root, for example `HORIZON_BROWSER_ROOT=/var/tmp/srh/root`.

## 5. Setup

1. Write the username and the access key into two files with the mode 0600.

   Result: `stat -c %a <file>` shows `600` for each file.

2. Write a host file with one provider, one target and `credential_files` that name the two
   files. Use the example in `docs/architecture/remote-browser-sessions.md`, section
   "Standalone remote host". Give the host file the mode 0600.

   Result: The host file has no credential value.

## 6. Tasks

Give each task an ID. A report uses the ID to give a result.

### 6.1 SRH-FILES: Refuse files that other users can change

1. Give the host file the mode 0664 and start the host.

   Result: The host stops with "must not be writable by group or other users".

2. Give the host file the mode 0600 again. Give one credential file the mode 0640. Start the
   host and call `browser_create` with the target.

   Result: The call fails with `credentials_not_ready` and "readable only by its owner". The
   message names the reference, not the value or the path.

3. Give the credential file the mode 0600 again.

   Result: The next `browser_create` does not fail because of the file.

### 6.2 SRH-OPEN: Open, use and close a session

1. Call `browser_create` with the target.

   Result: The answer has a panel. `browser_list` shows the panel.

2. Call `browser_provider_usage` and `browser_remote_allocations` with `operation: list`.

   Result: Usage shows the provider. The allocation list shows the provider with the status
   `in_use`.

3. Call `browser_create` with no target and `allow_additional: true`.

   Result: The call fails with `target_required`.

4. Call `browser_close` with the panel.

   Result: The provider gets one session delete. The allocation shows `released`.

### 6.3 SRH-EXIT: Release at the end of stdin

1. Call `browser_create` with the target. Then close stdin of the host.

   Result: The provider gets one session delete. The host exits with the code 0. No file stays
   under `<runtime root>/standalone-remote-holds/`.

2. Start the host again, open a session, and send SIGTERM to the host.

   Result: The provider gets one session delete, and the host exits.

### 6.4 SRH-KILL: Take over the holds of a killed host

1. Start the host, open a session, and send SIGKILL to the host.

   Result: The hold files stay under `<runtime root>/standalone-remote-holds/`.

2. Start a new host on the same runtime root. Call `browser_remote_allocations` with
   `operation: list`.

   Result: The list shows the hold of the killed host, with the status that the provider gives
   for the exact session.

3. When the provider ended the session, call `operation: reconcile` with the reference.

   Result: The status is `released`, and the hold files are gone.

### 6.5 SRH-REAL: One session at the real provider

1. Do task SRH-OPEN with the real provider and one target.

   Result: The session opens, a page loads, and the session closes. The allocation shows
   `released`, directly or after the check at the provider.

2. Search the output and the error output of the host for the username and the access key.

   Result: Neither value is found.

## 7. Pass criteria

- Every result in section 6 is as written.
- No output of the host, no answer of a tool and no file outside the two credential files
  contains a credential value.

## 8. Cleanup

1. Close every session and close stdin of the host.

   Result: The host exits.

2. Remove the two credential files and the runtime root.

   Result: No credential file stays on the computer.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
