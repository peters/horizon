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

1. Start the native host.

   ```bash
   horizon --native-mcp --client /absolute/path/to/client.json
   ```

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

1. Start the CLI with separate stdout and stderr files.

   ```bash
   horizon --native-run --client /absolute/path/to/client.json > /private/new-task/report.json 2> /private/new-task/progress.ndjson
   ```

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

3. Run exact reconciliation with the same client file.

   ```bash
   horizon --native-reconcile --client /absolute/path/to/client.json
   ```

   Result: Exact provider acknowledgements and local receipts release the recorded resources.

   For legacy receipts after a confirmed Linux reboot, follow the
   [reboot recovery procedure](../../architecture/remote-device-testing.md#recover-local-resources-after-a-linux-reboot).
   Supply all pending owned `run` and `tunnel` IDs, also records with no dispatched resource.
   The host requires the existing journal and original owner binding.
   Missing or extra IDs cause refusal before cleanup starts.
   A later receipt or provider refusal keeps the affected resource held.

4. If recovery remains uncertain, keep the original owner and receipts.

   Result: No new allocation bypasses an uncertain operation. A bounded repeat reconciles only the same operations.

### 6.5 NATIVE-REMOTE-BUILD — Remote build parent closure

1. Start a project build through its declared remote guardian.

   Result: Private evidence records the unique remote directory and exact build process group.

   > **CAUTION:** STOP ONLY THE RECORDED TASK-OWNED HELPER.
   > Remote cleanup removes its unique source directory.

2. Stop only the task-owned local build helper with SIGKILL.

   Result: Remote SSH EOF stops the exact build group and removes its owned source directory.

### 6.6 NATIVE-MCP — Interactive tools

1. Read `tools/list` from the native host.

   Result: The response lists all 13 native tools and their typed argument schemas.

> **CAUTION:** UPLOAD ONLY THE DECLARED TEST ARTIFACT.
> The provider stores this private app until the host deletes its owned upload.

2. Call `app_upload` for one declared platform.

   Result: The response contains an opaque artifact handle and its hash and size.

> **CAUTION:** USE ONLY THE APPROVED DEVICE QUOTA AND PORTS.
> Session creation allocates a paid device and starts its declared tunnel and backend.

3. Call `app_session_create` with that artifact and one declared matrix index.

   Result: The host returns an owned session with its original finite lifetime.

4. Call `app_tunnel_status` for that session.

   Result: The response shows ready local services without credentials or provider keys.

5. Call `app_snapshot` for that session.

   Result: The response contains the native tree and session-specific element refs. Secure values remain redacted.

6. Call `app_wait` for the first target in the declared recipe.

   Result: The target reaches its declared state within the original deadline.

7. Call `app_act` for an action from the declared recipe.

   Result: The driver returns an acknowledgement. A later snapshot or assertion proves the app's result.

8. Call `app_screenshot` for that session.

   Result: The host returns a validated PNG with a private evidence handle.

9. Call `app_view` for that session.

   Result: The endpoint belongs to the exact session. Repeated calls reuse the same owned viewer.

10. Attach the endpoint with public `device_panel`.

    Result: The live-panel checks in NATIVE-MATRIX apply to this viewer.

11. Call `app_video` with `operation: status`.

    Result: The response describes recording from allocation to closure. Recording cannot pause or start later.

12. Call `app_video` with `operation: start`.

    Result: Enabled video returns the same policy. Disabled video returns a typed unavailable error.

13. Call `app_audit` with `after_sequence: 0` and a bounded `limit`.

    Result: Receipts contain a stream UUID, sequence, static action names and typed results. They contain no raw input.

> **CAUTION:** STOP ONLY THE OWNED SESSION.
> The video stop operation closes this device before its recording download.

14. Call `app_video` with `operation: stop`.

    Result: Provider closure precedes video download. Pending video has an explicit unavailable result.

15. Call `app_video` with `operation: get` after provider finalization.

    Result: The read-only request returns retained video or a typed unavailable result.

16. Call `app_logs` for each documented log kind.

    Result: Device, crash, Appium and network logs have explicit available or unavailable results.

> **CAUTION:** CLOSE ONLY THE RECORDED OWNED SESSION.
> This operation deletes that session and stops its local services.

17. Call `app_session_close` with the same handle.

    Result: The host acknowledges exact closure. A closed session does not reopen.

> **CAUTION:** CLOSE ONLY THE TASK-OWNED HOST.
> Parent closure deletes this host's owned uploads and stops its local resources.

18. Close the native host's stdin.

    Result: The host retires the owned upload and remaining local resources.

### 6.7 NATIVE-BOUNDS — Refusal and report limits

1. Run the native host regressions.

   ```bash
   cargo test -p horizon-app-host
   ```

   Result: Tests cover concurrent lanes, cancellation, evidence limits, terminal reports and typed MCP refusal.

2. Run the native driver regressions.

   ```bash
   cargo test -p horizon-app-testing --test native_driver
   ```

   Result: Tests cover gestures, app lifecycle, waits, secure values, ambiguous targets and expired or foreign refs.

3. Examine the evidence-limit results.

   Result: Oversized requests cause refusal before resource operations. Full evidence storage cannot consume the terminal report reserve.

4. Examine the ownership and output results.

   Result: Foreign handles cause refusal. Lost replies retain uncertainty. A blocked output consumer cannot retain resources indefinitely.

### 6.8 NATIVE-CAPTION — Device identity and progress

1. Attach both lane endpoints to Device panels on the candidate.

   Result: Each header shows the model, OS version, form, provider and matrix lane.
   The lane number starts at one. The header shows the app ID and a short build hash.

2. Examine each panel with `device_panel inspect`.

   Result: `server.native_session` contains the same facts as the header.
   **Connection details** contains selectable session, run and full build IDs.
   The metadata contains no credentials, upload tokens, tunnel names or provider URLs.

3. Run a recipe with more than one step.

   Result: The caption follows the current recipe and step within about one second.
   Each completed recipe has a **PASS**, **FAIL** or **BLOCKED** result.

   Use a synthetic provider fixture to close the driver session during a recipe.
   Then run one more recipe.

   Result: The failed step keeps its session error. Later steps are blocked without
   native input. The caption and **Connection details** show **BLOCKED** for each
   affected recipe. Provider diagnostics are fetched if the archive is usable.

4. Run a failing recipe and a recipe with a reset step.

   Result: The failing recipe shows **FAIL**. The replacement endpoint has its own
   session ID. It keeps the run ID, matrix lane and completed recipe results.

   Use a synthetic provider fixture to delay and refuse the replacement allocation.
   Then run one more recipe.

   Result: The original viewer keeps both recipe **FAIL** results before it disconnects.
   Native resources close before the delayed replacement result arrives.
   Cancellation or a progress callback failure releases the retained viewer.

5. Examine the panels after native cleanup.

   Result: The panels disconnect. The last metadata and recipe results remain visible.

   Close the CLI immediately after its report.

   Result: The viewer receives the final recipe result before the CLI exits.
   A client with an incomplete message cannot prevent bounded viewer cleanup.

6. Resize a panel and select **Fit**.

   Result: The caption wraps within the panel. The image keeps its aspect ratio.

7. Reconnect a panel to a synthetic VNC server without native metadata.

   Result: The old native metadata disappears. Ordinary clipboard text does not
   create native metadata or change the local clipboard.

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
