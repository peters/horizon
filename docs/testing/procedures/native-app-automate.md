---
procedure: native-app-automate
feature: Native app tests through MCP and CLI
platforms: [linux, macos]
cost: paid device
destructive: yes
secrets: Horizon OS credential-store references
owner: peters
---

# Native app test procedure

## 1. Purpose

This procedure tests the declared native app matrix through MCP and CLI.
It also tests live views, evidence and cleanup after cancellation or a native host crash.

## 2. Applicability

- Candidate: the packaged Horizon executable with native host support.
- Provider: BrowserStack App Automate.
- Devices: the complete project matrix, with up to two concurrent lanes.
- Project recipes define the app features under test.
- This procedure does not test production deployment or Windows process execution.

## 3. Safety

> **CAUTION:** USE ONLY THE APPROVED ACCOUNT AND DECLARED LOOPBACK PORTS.
> Device use consumes account capacity. A broad tunnel can expose unrelated services.

> **CAUTION:** STOP ONLY RESOURCES THAT THIS RUN OWNS.
> Shared service changes can remove another person's data or interrupt another run.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- A private client file with a stable owner and shared provider state.
- Unlocked Horizon credential-store references.
- A trusted tunnel executable with its pinned SHA-256.
- The project's build tools, Debug configuration and companion development services.
- The public `device_panel` tool in the agent's workspace.

## 5. Setup

1. Read [the native runbook](../../architecture/remote-device-testing.md).

   Result: The selected project and host prerequisites are known.

2. Read the project and companion `AGENTS.md` files.

   Result: The app builds, matrix, synthetic backend and executable recipes are declared.

3. Record the candidate hash and project commits in private evidence.

   Result: Every result names the exact source and executable.

4. Run the packaged catalog command.

   ```bash
   horizon --native-catalog
   ```

   Result: The provider returns the current native devices and account capacity without allocation.

## 6. Tasks

### 6.1 NATIVE-MATRIX — MCP execution

1. Start the native host with `--native-mcp --client <private-client-file>`.

   Result: MCP initialization lists `device_test_run` and the interactive `app_*` tools.

   > **CAUTION:** USE ONLY THE APPROVED DEVICE QUOTA.
   > This operation allocates paid devices and deletes its owned uploads after completion.

2. Call `device_test_run` with the approved finite lifetime and progress notifications.

   Result: Each platform builds and uploads once. The complete matrix uses at most two device lanes.

3. Attach each reported live endpoint through the public `device_panel` tool.

   Result: Each owned Device panel connects to the exact reported session.

4. Save three public inspections at least two seconds apart for each Device panel.

   Result: Each panel shows actual displayed pixels and frame advancement during app activity.

5. Examine the per-device report and backend request evidence.

   Result: Every recipe step passes. Each app sends successful requests to its assigned backend.

6. Decode each retained video.

   Result: Each video contains app activity. Screenshots and requested logs have explicit results.

### 6.2 NATIVE-CANCEL — CLI cancellation

> **CAUTION:** USE ONLY THE APPROVED DEVICE QUOTA.
> This operation allocates paid devices and deletes its owned resources after cancellation.

1. Start `--native-run --client <private-client-file>` with separate stdout and stderr evidence files.

   Result: Stderr contains progress events. Stdout contains the terminal JSON report.

2. Send SIGINT after progress reports the first native recipe step.

   Result: The run reports cancellation and exit code 2. Every owned resource has a confirmed cleanup result.

### 6.3 NATIVE-EOF — MCP parent closure

> **CAUTION:** USE ONLY THE APPROVED DEVICE QUOTA.
> This operation allocates a paid device.

1. Start one interactive session through the declared `app_*` tools.

   Result: The app responds to a semantic wait through its own backend and tunnel.

   > **CAUTION:** CLOSE ONLY THE TASK-OWNED NATIVE HOST.
   > This operation deletes the host's owned sessions and uploads.

2. Close the native host's stdin.

   Result: The host stops its sessions, uploads and local services before normal exit.

### 6.4 NATIVE-CRASH — Exact recovery

> **CAUTION:** USE ONLY THE APPROVED DEVICE QUOTA.
> This operation allocates a paid device.

1. Start one interactive session and record its exact cleanup receipts.

   Result: Private evidence identifies its session, tunnel, backend and upload.

   > **CAUTION:** STOP ONLY THE RECORDED TASK-OWNED HOST.
   > Its guardians remove owned local services. Another host can contain unrelated work.

2. Stop only the task-owned native host with SIGKILL.

   Result: The guardians stop their exact local child groups after parent EOF.

   > **CAUTION:** USE THE SAME OWNER AND PRIVATE STATE.
   > Recovery deletes the recorded provider sessions and uploads.

3. Run `--native-reconcile --client <the-same-private-client-file>`.

   Result: Exact provider acknowledgements and local receipts release the recorded resources.

4. If recovery remains uncertain, keep the original owner and receipts.

   Result: No new allocation bypasses an uncertain operation. A bounded repeat reconciles only the same operations.

### 6.5 NATIVE-REMOTE-BUILD — Remote build parent closure

1. Start a project build through its declared remote guardian.

   Result: Private evidence records the unique remote directory and exact build process group.

   > **CAUTION:** STOP ONLY THE RECORDED TASK-OWNED HELPER.
   > Remote cleanup removes its unique source directory.

2. Stop only the task-owned local build helper with SIGKILL.

   Result: Remote SSH EOF stops the exact build group and removes its owned source directory.

## 7. Pass criteria

- All matrix steps pass with real app-to-backend requests.
- The run respects the current native quota and the two-lane limit.
- Every Device panel displays changing app frames.
- All required evidence has a successful explicit result.
- Cancellation, parent closure and exact recovery release all owned resources.
- Missing acknowledgements retain uncertainty and block fresh allocation.

## 8. Cleanup

1. Close only the run's Device panels.

   Result: The owned panels disappear. Unrelated panels remain intact.

2. Examine the final cleanup receipts and exact recovery result.

   Result: No owned session, upload, tunnel, backend or remote build remains active.

3. Keep the private reports, recordings and receipts within their documented limits.

   Result: Evidence remains available for review without public app or credential data.

## 9. Record of results

Use [the report template](../reports/TEMPLATE.md).
Keep app-specific evidence private. Publish only reviewed nonsecret results.
