# Native app tests on BrowserStack

A project declares its builds, matrix, backend and recipes in `AGENTS.md`.
The MCP tools and CLI use the same native host.
The host builds and uploads each platform once.
It runs at most two device lanes within the current App Automate quota.
Managed services give each lane its own synthetic backend and loopback port.
Contracts with any fixed service port run one lane at a time.
The report records the effective concurrency.
Only `provider: browserstack` is supported; other provider names fail contract validation.
Browser sessions use a separate quota.

Use [the test procedure](../testing/procedures/native-app-automate.md) for acceptance tests.
Use [the schema](remote-device-testing.schema.json) for the project contract.
Use [the STE rules](../style/ste-rules.md) and [technical names](../style/technical-names.md) for procedure changes.

## 1. Host requirements

| Item | Requirement |
|---|---|
| Native host | Linux or macOS. Windows process execution is unavailable. |
| iOS build | An approved Mac with Xcode and unattended SSH access. |
| Android build | The project's Android SDK and JDK. |
| Credentials | Horizon OS credential-store bindings for the selected provider. |
| Tunnel | An approved BrowserStack Local executable with a recorded SHA-256. |
| State | Private directories on the same qualified filesystem. |

The provider re-signs unsigned Debug IPA files.
This workflow does not distribute production apps.
The project supplies its build requirements.
A Windows build does not qualify process or tunnel execution.

## 2. Prepare the host

> **CAUTION:** KEEP PROVIDER CREDENTIALS IN HORIZON.
> Credentials in commands, contracts or tool arguments can expose the account.

1. Configure the `browserstack` provider's OS credential-store bindings in Horizon.

   Result: The native host uses the configured credentials without a login prompt.

2. If the OS keyring is locked, unlock it locally.

   Result: Horizon can read the configured bindings. Do not send the password to an agent.

3. Install the approved BrowserStack Local executable.

   Result: The tunnel path names a trusted executable.

4. Record its approved SHA-256.

   Result: The native host refuses changed executable bytes.

5. Create a private state directory.

   Result: The current user owns the directory. Other users cannot read it.

6. Create the machine-local client file.

   Result: The file has mode `0600`. Its absolute paths have no symlink components.

   ```json
   {
     "version": 1,
     "owner": "<fresh UUID for this project client>",
     "project": "/absolute/path/to/isolated-app-checkout",
     "state": "/absolute/path/to/private/shared-native-state",
     "provider": "browserstack",
     "tunnel_binary": "/absolute/path/to/verified/BrowserStackLocal",
     "tunnel_sha256": "<approved SHA-256>"
   }
   ```

Clients for the same provider account must use the same state directory.
Keep the owner UUID unchanged for recovery.
The host locks the canonical project directory for the actor lifetime.
Different owners, accounts and state directories cannot execute concurrently on the same directory.
Retained unfinished records still require their original owner and private state for reconciliation.
Do not replace it to bypass uncertain work.
The host retains the project's directory identity.
It reads declared files through that retained directory.
Artifact capture checks the opened inode change timestamp and verifies a second full read before upload.
A changing source returns a typed refusal.
Tunnel files retain their original file and parent descriptors for cleanup.
An observed replacement entry retains uncertainty and is never removed as the owned file.

7. Register the packaged executable as one MCP server.

   Result: The selected project and credentials come from host configuration.

   ```json
   {
     "mcpServers": {
       "horizon-native": {
         "command": "/absolute/path/to/horizon",
         "args": ["--native-mcp", "--client", "/absolute/path/to/client.json"]
       }
     }
   }
   ```

8. Set the MCP timeout to at least the requested run lifetime.

   Result: The timeout covers a run of up to 1,800 seconds.

9. Enable MCP progress notifications.

   Result: The client receives build, session and recipe events.

The standalone development executable uses `horizon-native --mcp --client <file>`.
MCP arguments cannot replace the configured project or credentials.

## 3. Examine the device catalog

1. Run the catalog command.

   ```bash
   horizon --native-catalog
   ```

   Result: The provider returns physical devices and current native capacity without allocation.

`latest-2` selects the third distinct numeric OS release in the physical-device catalog.
A missing matrix entry causes refusal before any partial matrix starts.
Android selects Appium 2.19.0 instead of the legacy default.
iOS uses the provider's OS-compatible version.

## 4. Declare the project

Commands are argv arrays. Shell strings are not permitted.
Artifact and recipe paths stay inside the selected checkout.
This example declares both platforms and four devices.

````markdown
```yaml
remote-device-testing:
  version: 1
  provider: browserstack
  apps:
    ios:
      build: [python3, scripts/test-build.py, ios]
      artifact: build/ios/App.ipa
      bundle_id: com.example.app
    android:
      build: [python3, scripts/test-build.py, android]
      artifact: build/android/App.apk
      package: com.example.app
  launch_arguments:
    BASE_URL: "http://localhost:{tunnel.port.backend}"
  tunnel:
    ports:
      backend:
        start: [python3, scripts/test-backend.py]
        timeout_seconds: 900
  max_parallel: 2
  matrix:
    - {platform: ios, form: phone, os: latest}
    - {platform: ios, form: phone, os: latest-2}
    - {platform: ios, form: tablet, os: latest}
    - {platform: android, form: phone, os: latest}
  recipes: [docs/features/native-shell.md]
  evidence: {video: true, screenshots: true, logs_on_failure: true}
```
````

1. Read the companion backend's `AGENTS.md` and development guide.

   Result: The local services and synthetic data requirements are known.

2. Declare a separate backend for each lane.

   Result: Each device has its own database, cache namespace, process and port.

> **CAUTION:** KEEP SHARED CHECKOUTS AND DATA INTACT.
> A shared reset or database deletion can interrupt other work.

3. Use only isolated synthetic data.

   Result: The run does not use production snapshots or global database resets.

Declare between one and sixteen tunnel ports. Missing or empty port maps are invalid.
Only declared numeric loopback ports enter the tunnel allowlist.
The adapter changes the loopback launch URL to the exact `bs-local.com:<port>` alias.
It preserves the URL path.
The app's Debug network policy must permit this host.
Do not combine `--only` with `--force-local`; this combination removes the tunnel restriction.

## 5. Managed backend protocol

The foreground command receives a private `HORIZON_APP_BACKEND_DIR`.
It emits one bounded stdout record after the app backend is ready.

```json
{"native_backend_ready":1,"port":33327}
```

The guardian sends a cleanup request and then closes stdin.

```json
{"native_backend_cleanup":1,"nonce":"<host-cleanup-nonce>"}
```

The helper stops its owned process groups and removes only its owned worktree and synthetic namespace.
It returns this record after cleanup completes.

```json
{"native_backend_closed":1,"nonce":"<host-cleanup-nonce>"}
```

A missing or incorrect nonce keeps cleanup uncertain.
Foreground commands must not daemonize or escape guardian ownership.
A remote build needs its own finite remote guardian.
Local SSH process cleanup alone cannot stop a remote Xcode build.

## 6. Declare recipes

A recipe has exactly one `device-recipe` YAML block.
Markdown text alone does not declare an executable test.
Optional `platforms: [ios]` or `[android]` selects one platform.
Without this field, the recipe selects both platforms.

````markdown
```yaml
device-recipe:
  version: 1
  id: NATIVE-SHELL-01
  steps:
    - {id: ready, action: wait, target: {by: identifier, value: menu.open}, state: visible, timeout_millis: 60000}
    - {id: open-menu, action: tap, target: {by: identifier, value: menu.open}}
    - {id: menu, action: wait, target: {by: identifier, value: menu.content}, state: visible, timeout_millis: 10000}
    - {id: evidence, action: screenshot}
```
````

## 7. Run the matrix

> **CAUTION:** USE ONLY THE APPROVED DEVICE QUOTA AND PORTS.
> The run allocates paid devices and deletes its owned uploads after completion.

1. Call `device_test_run` with the approved lifetime.

   ```json
   {"lifetime_seconds":1800}
   ```

   Result: One original lifetime covers builds, uploads, allocation, recipes and evidence.

2. Examine the per-device report.

   Result: The report lists build results, devices, step durations, screenshots, media results and cleanup results.

3. Examine the backend request records.

   Result: Actual app requests prove access to each assigned loopback backend.

Unavailable evidence is an explicit failure, not a successful capture.
A failed step does not stop the other devices.
Cancellation prevents further allocation.
The retained worker saves the report after it closes its owned resources.
Provider dashboard links require provider login and contain no share token.

The CLI uses the same controller.

> **CAUTION:** USE ONLY THE APPROVED DEVICE QUOTA AND PORTS.
> The CLI allocates paid devices and deletes its owned resources.

4. Run the CLI command.

   ```bash
   horizon --native-run --client /absolute/path/to/client.json
   ```

   Result: Stderr contains finite NDJSON progress. Stdout contains the terminal JSON report.

The host saves the private report before output delivery.
SIGINT and SIGTERM cancel the run.
A stalled output consumer cannot retain resources indefinitely.
Exit code 2 identifies failed or cancelled execution or uncertain cleanup.
Examine the saved report before any repeat of a mutation.

## 8. Observe live panels

1. Create a public `device_panel` for each `session_created` endpoint.

   Result: The viewer connects to that session's read-only loopback RFB endpoint.

2. Save three public inspections at least two seconds apart.

   Result: Displayed pixels and advancing frames prove live presentation during app activity.

3. If a panel was displayed and the person moves away, keep the test active.

   Result: Background inspection continues without another reveal request.

4. Close only this run's viewers after the test.

   Result: Unrelated panels remain intact.

Connection alone does not prove live presentation or app interaction.
Backend readiness does not prove app network access.
The viewer uses a fixed 768×1536 presentation and preserves the device's aspect ratio.
It does not change device geometry.

## 9. Interactive MCP reference

Read `tools/list` for the complete typed argument schemas.
Session and artifact arguments use opaque handles, not provider identifiers.
The table names all 13 tools.

| Tool | Result or behavior |
|---|---|
| `app_upload` | Uploads one declared platform artifact. Returns an opaque hash/size handle. |
| `app_session_create` | Starts one matrix entry with its own backend and tunnel. |
| `app_snapshot` | Returns the native accessibility tree. Redacts secure field values. |
| `app_act` | Runs a native gesture, element action, assertion or app lifecycle action. |
| `app_wait` | Waits for an element state within the original deadline. |
| `app_screenshot` | Returns a validated PNG and private evidence handle. |
| `app_view` | Opens or reuses that session's read-only native viewer. |
| `app_video` | Returns recording policy or video. The `stop` operation closes the session before download. |
| `app_logs` | Returns redacted device, crash, Appium or network logs when available. |
| `app_tunnel_status` | Returns redacted readiness and ownership state. |
| `app_session_close` | Gets exact provider closure acknowledgement, then stops the owned local services. |
| `app_audit` | Returns receipts with a stream UUID and sequence cursor. |
| `device_test_run` | Runs the complete declared matrix through the same controller. |

Targets use `identifier`, `label`, `ref` or finite device `coordinates`.
Each ref belongs to one session and one fresh snapshot.
A new snapshot invalidates old refs.
Android named-element resolution also takes a new snapshot.
Input requires one exact match and unchanged native identity.
An ambiguous target, changed identity or expired ref causes refusal.
A driver acknowledgement does not prove the app's expected result.

The provider records video from allocation to session closure when video is enabled.
The `start` and `status` operations return this policy.
Recording cannot pause or start later.
Pending video returns a typed unavailable error.
Only read-only downloads can be repeated without mutation replay.
Network logs require provider support and enabled capture.
The current contract does not enable network capture by default.
Crash logs can be absent when no crash occurred.

Audit retains the latest 256 receipts for the host lifetime.
Receipts contain static action names, opaque IDs, results, durations and typed errors.
Compare stream UUIDs before cursor reuse after a restart.
Capture polling does not fill the audit.
Raw input, provider IDs, upload tokens, URLs and credentials do not enter receipts.

## 10. Evidence limits

Interactive screenshots retain the latest 32 captures for the host lifetime.
Run reports and exported media survive normal host exit in private archives.
At most eight archives are admitted across restarts.
Each archive permits 1,024 evidence files within 120 MiB.
One separate terminal report has an 8 MiB reserve.
The complete archive limit is 128 MiB.

Before builds or allocations, the host rejects runs that exceed the file budget.
The estimate counts screenshots and possible failure logs and videos for every initial or reset allocation.
Evidence above the byte limit remains explicitly unavailable in the report.
Full storage refuses a new archive.
The host does not automatically delete retained evidence.
Provider evidence also remains at BrowserStack under its retention policy.

1. Find the archive through the report's `report_path`.

   Result: The archive UUID is separate from the run UUID.

2. Export the completed evidence with its paths and file hashes.

   Result: A private export receipt identifies the original and destination paths.

> **CAUTION:** RETIRE ONLY COMPLETED OWNED EVIDENCE.
> A premature deletion can remove recovery or test evidence.

3. If the hashes match, retire the completed owned archive.

   Result: A new archive can use the released storage slot.

Downloads use trusted origins, finite redirects, body limits and one remaining timeout.
Credentials do not reach video CDNs.
The host removes known secrets and sensitive log lines before export.
Other app content can remain private.
Do not publish app screenshots, recordings or logs in public PRs.

At most two media downloads run per controller.
A third request returns `app_media_busy` before provider access.
Downloads do not hold action locks.
Cold app launches receive up to 60 seconds within the original deadline.
Other commands retain the normal 15-second limit.
Horizon does not retry mutations.
Video finalization polls for up to 30 seconds within the run deadline.
Pending video remains an explicit failure after this limit.

Reports name every allocation from reset steps.
Each media entry names its allocation.
The final dashboard link names the final allocation.
The normal provider-reference history has at most 32 entries, including both live lanes.
An exclusive validated run retains its references through final media export.
Its limit is 1,088 entries: 32 prior references, 32 initial allocations and 1,024 reset steps.
Every exit, cancellation or panic restores the 32-entry history.
This retention does not renew resource leases.
Original expiry timestamps apply after the run releases its references.
Other expired references return explicit media errors.
Reset does not silently replace earlier evidence.

## 11. Recover uncertain operations

Use the original provider state and owner after a lost reply, host crash or uncertain cleanup.

> **CAUTION:** RECONCILE ONLY THE RECORDED OWNED OPERATIONS.
> Recovery deletes the recorded sessions and uploads and stops their local services.

1. Run exact reconciliation with the same client file.

   ```bash
   horizon --native-reconcile --client /absolute/path/to/client.json
   ```

   Result: Provider acknowledgements and local receipts release only the recorded owned resources.

2. If cleanup remains uncertain, preserve the original state and owner.

   Result: The uncertainty keeps its quota hold and original finite lifetime.

Absence from a provider list does not prove release.
Repeated creation is not recovery.
Unknown or missing ownership causes refusal.
Reconciliation stops sessions and services before it retires uploads.

Completed provider history retains 32 unreferenced records per owner, ordered by creation time.
Active controller entries and unfinished cleanup remain protected.
Local guardian history retains its separate directory-first retirement rule.
Uncertain records and all retained resources remain protected.
At the shared 512-record admission limit, the host retires the oldest confirmed, resource-free record across inactive owners and credential realms.
A retained nonblocking execution lease protects the selected owner until that history edit is durable.
Active owners and unverified execution bindings remain protected.
This global history policy does not stop a foreign resource or remove its files.
A full journal of unfinished operations still refuses admission.
A setup failure reports confirmed cleanup only after its exact cleanup acknowledgement.
This rule also applies to the replacement session during reset.
After confirmed cleanup, the original setup error stays in the step result.
The runner does not retry a failed reset.

Keep the shared journal and registry on the same qualified filesystem.
The registry records device and inode identities.
Copied state, changed device numbers or a missing root causes refusal.
Preserve the original state and evidence.
Do not recreate the registry or stamp new identities to bypass refusal.
Filesystem migration and registry repair require separate qualification.

## 12. Report the evidence

Report local tests, displayed-frame proof, app requests, decoded video and cleanup separately.
Authenticated workflows, payments and feature parity require their own recipes.
An anonymous shell test does not qualify those workflows.

`app_act` refuses the recipe-only screenshot action.
Use `app_screenshot` to receive the image and retained evidence.
A failed CLI progress sink cancels the shared run before later allocation.

Launch URLs support `localhost` and `127.0.0.1` with declared forwarded ports.
IPv6 loopback URLs are refused during contract validation because provider forwarding
and managed services use IPv4 loopback. Closed native sessions synchronously signal
their live views, so later matrix lanes do not wait for the capture polling interval
to reclaim the two-stream capacity.
