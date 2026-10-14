---
procedure: tailnet-api-status
feature: nonsecret desktop tailnet API credential status parity
platforms: [linux]
cost: none
destructive: yes       # exact synthetic bindings and fixture cleanup
secrets: [synthetic auth keys and API credentials in an isolated OS store]
owner: peters
---

# Tailnet API credential status test procedure

## 1. Purpose

Test the same nonsecret API credential observations through Settings, CLI and MCP.
Use synthetic local state. Send no Tailscale request and allocate no worker.

## 2. Applicability

- The candidate includes the merged API storage foundation and this status change.
- This procedure tests status; owner removal is covered by [tailnet-api-removal](tailnet-api-removal.md).
- Status returns no saved values and accepts no secret import.
- Stored presence does not prove remote validity, scope or tailnet access.
- Pending, unavailable and missing are separate observations.
- Cached status is not continuous OS-store monitoring.
- OAuth exchange, node receipts and remote node deletion are outside this procedure.
- This native lane qualifies Linux only. Other OS adapters need their own evidence.
- No native fixture implementation or unmerged PR is a product prerequisite.

## 3. Safety

> **CAUTION:** USE ONLY TASK-OWNED STATE AND SYNTHETIC CREDENTIALS.
> Do not mount or copy the operator's home, desktop bus, keychain or provider configuration.

> **CAUTION:** REMOVE ONLY THE EXACT BINDINGS AND RESOURCES CREATED BY THIS RUN.
> A failed or uncertain cleanup requires reconciliation with the original ownership record.

Status reads can finish a previously recorded local storage recovery. Keep the
state root and OS-store namespace isolated from the operator's state.
Never pass an API secret to an MCP tool, worker, argv, environment or evidence log.

## 4. Equipment and preconditions

- A candidate checkout, its Rust toolchain and a source-owned build target.
- A private evidence directory outside the checkout.
- Frozen production `horizon`, `cloud_deploy` and `horizon-device` executables.
- A build manifest that records their hashes, commit, dirty consumed inputs,
  dependency/toolchain versions and flags. A binary hash alone is not source proof.
- A task-owned isolated Linux X11 desktop and private OS credential service.
- A public live Device viewer and the exact owned `horizon-device` local X11 target.
- Three synthetic saved IDs with missing, personal-token and OAuth-client API kinds.
  Prepare them through the candidate's public `Store::save` and
  `Store::save_api_credential` APIs described in the
  [storage boundary](../../architecture/tailnet-api-credentials.md).
  Keep the preparation source, build inputs, helper hash and nonsecret result with
  the run evidence. Only trusted preparation runs before the candidate starts.
- A retained exact-ID ownership record outside candidate-writable state.
  Bind every setup/cleanup action to this record; names are not deletion authority.

An owned private fixture can supply these inputs, but it is test equipment, not a
shipped Horizon capability. Its setup, process/listener ownership and cleanup need
separate evidence. No preparation result replaces public MCP or native UI results.

## 5. Setup

1. Record the candidate source, complete consumed-input inventory and build commands.
   Freeze the three production executables from those inputs.

   Result: Source and artifact evidence identify the exact first or final candidate.

2. Prepare the private state/service and three synthetic saved rows. Record their
   exact IDs and kinds without credential values. Confirm the native target with
   the frozen `horizon-device` doctor command before using it.

   Result: The candidate and CLI use the same declared cloud state root.
   No host credential service or paid resource is part of this lane.

## 6. Tasks

### 6.1 S01 — Shared state and adapter tests

1. Run the affected tests in the candidate checkout.

   ```sh
   cargo test -p horizon-cloud tailnet
   cargo test -p horizon-core --features cloud-workspaces api_observations
   cargo test -p horizon-core --features cloud-workspaces --example cloud_deploy tailnets
   cargo test -p horizon-ui --features cloud-workspaces tailnet
   ```

   Result: Kind results contain no secret. Pending, unavailable and missing remain
   distinct. Blocked reads do not block rendering; stale replies cannot restore a
   removed ID. Open-Settings completion updates the shared MCP snapshot. Settings
   close transfers unfinished barriers before a fresh catalog read.

2. Examine the production bootstrap and root-mismatch tests.

   Result: Polling warms discovery with Settings closed. A cached catalog from
   another cloud state root is refused. MCP/render paths perform no OS-store IO.
   Ordinary ignored real-store tests do not count as native qualification.

### 6.2 S02 — Bundled guidance

1. Run the skill coverage check and compare both cloud bundles.

   ```sh
   python3 scripts/check-horizon-mcp-skills.py
   ```

   Result: Both bundles describe the actual status interfaces and their limits.
   Neither promises an API editor, authenticated remote authority or node cleanup.

### 6.3 S03 — Settings-closed MCP, actual CLI and visible rows

1. Launch the frozen candidate in the owned desktop with Settings closed. Open a
   public Device viewer for its actual VNC endpoint. Inspect three times, two
   seconds apart, while visible output changes.

   Result: The panel reports connected, displayed and advancing real frames.
   The viewer is read only. Follow the
   [Device skill](../../../assets/plugins/codex/skills/horizon-device/SKILL.md)
   for bounded native input through the exact local X11 target.

2. Start the frozen `horizon --browser-mcp` from a normal candidate **AGENT** panel.
   Initialize the public transport, list tools, then read `cloud_list` and
   `cloud_companions`. Do not supply actor/host-instance values manually.

   Result: Timestamped public responses show the prepared IDs and
   `tailnet_api_credentials` ready kinds after bootstrap, with Settings closed.
   `cloud_list` may be empty. Pending/unavailable never prove missing; a null
   catalog proves no absence. Private queues, stamps and cache files are not proof.

3. Run the frozen CLI against that same declared state root.

   ```sh
   /ABSOLUTE/FROZEN/cloud_deploy tailnets /ABSOLUTE/OWNED/CLOUD_STATE_ROOT
   ```

   Result: Saved IDs/names and the status map agree with public MCP. The command
   returns nonsecret status; it imports no secret or new credential mutation.

4. Use a fresh screenshot and bounded native input to open **Settings > Tailnets**.

   Result: The same IDs show **Not stored**, **Personal API token stored** and
   **OAuth client stored**. There is no API editor. Enrollment-key editing remains
   separate. Preserve public responses, CLI output and native evidence separately.

5. Resize the owned application window through native input, then use workspace
   **Fit**. Capture a fresh screenshot after each action.

   Result: The status rows stay readable and the owned window remains usable.
   The native desktop geometry stays separate from the application window size.

### 6.4 S04 — Unavailable service and explicit refresh

1. Stop only the exact owned credential service. Request **Refresh** through
   native Settings input, then observe public MCP and rerun the frozen CLI.

   Result: Failed background reads yield **Status unavailable** and unavailable
   observations, not missing. Stopping a service alone need not change a cached
   ready value. Record pending only if it was actually observed.

2. Restore only that service and request **Refresh** again.

   Result: UI, MCP and CLI report the original kinds while Settings remains open.
   Close Settings and read MCP again; discovery still works without the panel.

### 6.5 S05 — Open-Settings completion and close transfer

1. With Settings open, use **Replace key** on the synthetic personal-token row.
   Change its synthetic name, enter a synthetic enrollment auth key and click
   **Save tailnet**. The existing form requires an auth key to save a name.

   Result: Background completion preserves the saved ID and API kind. The native
   row, public MCP catalog/map and actual CLI agree before Settings closes.

2. Remove only the original missing synthetic row through native **Remove**.
   Leave Settings open for MCP and CLI observations.

   Result: That exact ID disappears from rows, legacy id/name items and the API
   map. Delayed replies cannot restore it. No remote Tailscale node is deleted.

3. Request **Refresh**, close Settings, read MCP, then reopen Settings.

   Result: Completed observations remain available and the removed ID stays
   absent. The focused blocked-reader test supplies deterministic unfinished-barrier
   evidence; native evidence reports only the phases actually observed.

4. Record a short native GIF through the public Device viewer. Inspect decoded
   frames before publishing and use synthetic names only.

   Result: The GIF shows actual status rows and the tested visible transitions.
   It does not replace timestamped public MCP or CLI results.

## 7. Pass criteria

- S01/S02 and the repository's applicable matrix pass on the current candidate.
- S03–S05 have actual production/native evidence before the visible status PR opens.
- UI, CLI and MCP distinguish stored, missing and unavailable; MCP also has pending.
- No real credential, provider allocation, policy change or node deletion occurs.
- Setup, source/artifacts, direct-store inputs, public routes and cleanup have separate proof.

## 8. Cleanup

1. Close the owned candidate normally through native input and close its viewer.

   Result: No other target or active Horizon process is stopped.

2. Remove only the retained synthetic saved IDs through the shared store. Verify
   their catalog/API bindings absent and the private service empty.

   Result: Unknown or changed ownership refuses deletion. Native row names are not
   authority. Credential cleanup is distinct from closing a panel or stopping a bus.

3. Stop and verify absence of the exact owned processes, listeners and any container
   on the same healthy daemon. Remove only their recorded private files.

   Result: Frozen artifacts/logs/ownership remain. Uncertainty retains state for
   exact reconciliation. No broad prune or host-bus fallback is permitted.

## 9. Record of results

Repeat real inputs on the first reviewable and final head. Reuse only with exact
consumed-input comparison. Record source/artifacts, commands, public responses,
actual native frames/GIF and exact cleanup in the
[report template](../reports/TEMPLATE.md).
