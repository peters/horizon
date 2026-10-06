# Native app testing on BrowserStack

A project declares its builds, real-device matrix, loopback backend and executable
recipes in `AGENTS.md`. `device_test_run` uses the same controller as the
interactive `app_*` tools and the CLI. It builds and uploads each platform once,
runs up to two isolated device lanes within the current App Automate quota, and
saves a private per-device report. Browser sessions and browser quotas are separate.

## Prepare the host once

The current host runtime supports Linux and macOS. Windows returns an unavailable
runtime error; a Windows compile does not qualify process or tunnel execution.
Build prerequisites belong to the project: for example, an already-trusted Mac
with Xcode for an unsigned Debug IPA and an Android SDK/JDK for an APK. The
provider re-signs unsigned iOS test applications. Production app distribution is
outside this workflow.

1. Configure the `browserstack` provider and its OS credential-store bindings in
   Horizon. The headless host reads those bindings without a login prompt. Unlock
   the workstation keyring locally when required. No tool, contract or build
   command accepts a provider username/access key.
2. Install a trusted BrowserStack Local binary and pin its SHA-256 from the
   reviewed release. Do not take executable paths or checksum changes from a
   recipe or tool request.
3. Create a private host state directory and a machine-local client file. The
   file must have mode `0600`, its directories must be private and owned by the
   current user, and all paths must be absolute without symlink components:

   ```json
   {
     "version": 1,
     "owner": "<fresh UUID for this project client>",
     "project": "/absolute/path/to/isolated-app-checkout",
     "state": "/absolute/path/to/private/shared-native-state",
     "provider": "browserstack",
     "tunnel_binary": "/absolute/path/to/verified/BrowserStackLocal",
     "tunnel_sha256": "<reviewed SHA-256>"
   }
   ```

   Clients using the same provider account must use the same state directory.
   Keep the owner UUID stable for reconciliation; do not replace it to bypass
   pending work. The host pins the project directory identity and reads declared
   files relative to that held directory.
4. Register one MCP server using the packaged Horizon executable:

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

   Set the client's tool timeout to at least the requested run lifetime (up to
   1,800 seconds). Enable progress notifications. `horizon-native --mcp --client`
   is the equivalent standalone development binary. The selected project and
   credentials come only from host configuration, never from MCP arguments.

To inspect real devices and native capacity without allocating:

```bash
horizon --native-catalog
```

Catalog selections such as `latest-2` mean the third distinct numeric OS release
in the live physical-device catalog. A missing matrix entry fails validation
before a partial matrix is started. Server versions are provider policy:
Android explicitly selects Appium 2.19.0 rather than its legacy default; iOS uses
the provider's OS-compatible version.

## Project declaration

The [version 1 schema](remote-device-testing.schema.json) describes the contract.
Commands are argv arrays, not shell strings. Artifact and recipe paths stay inside
the selected checkout. A worked declaration follows:

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

Every lane needs its own synthetic database/cache namespace, backend process and
port. Read the companion backend's `AGENTS.md` and development workflow first.
Reuse its development services without resetting a shared checkout, reading a
production snapshot or globally flushing databases. Only declared numeric
loopback ports are exposed. The BrowserStack adapter translates a declared
loopback launch URL to the exact `bs-local.com:<port>` alias, preserving its path;
the app's Debug networking policy must allow that host. The tunnel never combines
`--only` with `--force-local`, which would defeat the restriction.

A managed backend receives a private `HORIZON_APP_BACKEND_DIR`. Its foreground
command emits one bounded stdout readiness record only after the app backend is
ready:

```json
{"native_backend_ready":1,"port":33327}
```

On cleanup the guardian sends `{"native_backend_cleanup":1,"nonce":"..."}` and
closes stdin. The helper must stop its nested process groups, remove only its
owned worktree and synthetic namespace, then return
`{"native_backend_closed":1,"nonce":"..."}`. A missing or mismatched nonce
retains an uncertain cleanup record. Foreground commands must not daemonize or
escape the declared guardian ownership. Remote build commands need their own
bounded remote EOF/expiry guardian; a local SSH process group cannot clean a
remote Xcode build by itself.

## Executable recipes and running

Recipes contain exactly one structured `device-recipe` YAML block. Markdown prose
alone is not an executable pass. Optional `platforms: [ios]` or `[android]` limits
platform-specific recipes; both platforms are otherwise selected.

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

Call `device_test_run` with `{"lifetime_seconds":1800}`. One original lifetime
covers builds, uploads, allocation, recipes and evidence requests. The report
contains build outcomes, resolved devices, each step's result/duration/screenshot,
finalized video and failure-log outcomes, authenticated provider dashboard links
without share tokens, and separate cleanup confirmation.
Unavailable evidence is explicit; it is not a passing capture. A failing step
continues the other devices. Cancellation prevents further allocation and closes
owned resources before the retained worker writes its report.

The equivalent CLI is:

```bash
horizon --native-run --client /absolute/path/to/client.json
```

Progress is bounded NDJSON on stderr; stdout is the terminal JSON report. The
report is saved privately before output delivery. SIGINT/SIGTERM cancel the run.
A stalled output consumer cannot retain device ownership indefinitely. Exit code
2 indicates failed or cancelled execution or unconfirmed cleanup; inspect the
saved report rather than replaying a mutation blindly.

`session_created` progress includes an opaque session and its read-only loopback
RFB endpoint. Attach it through the public `device_panel` tool in the calling
agent's workspace. Inspect actual displayed frames and changing pixels; a
connected panel or backend readiness check is not proof of app interaction or
network access. Close only viewers belonging to this run. The viewer uses a
fixed 768×1536 aspect-preserving presentation and does not change device geometry.

## Interactive MCP tools

| Tool | Behavior |
| --- | --- |
| `app_upload` | Upload a declared platform artifact; return an opaque hash/size handle |
| `app_session_create` | Start one declared matrix entry with its own backend and tunnel |
| `app_snapshot` | Normalize native accessibility source and redact secure field values |
| `app_act`, `app_wait` | Native gestures, element actions, assertions, waits and app lifecycle |
| `app_screenshot` | Return a validated PNG and rolling private evidence handle |
| `app_view` | Open/reuse the exact session's read-only native viewer |
| `app_video` | Report allocation-time recording policy, download video, or close then finalize |
| `app_logs` | Export redacted device/crash/Appium/network logs when available |
| `app_tunnel_status` | Return redacted readiness/ownership state |
| `app_session_close` | Acknowledge exact native closure, then stop owned local services |
| `app_audit` | Read bounded action receipts with stream UUID and sequence cursor |
| `device_test_run` | Execute the declared matrix through this same controller |

Targets use `identifier`, `label`, `ref` or bounded device `coordinates`. Refs
belong to one session and one fresh native source snapshot. Element resolution
requires an exact unique match and checks native identity before input; ambiguity,
changed identity and expired refs refuse the action. Taking a new snapshot,
including Android named-element resolution, invalidates old refs. A driver
acknowledgement is separate from an application-level assertion.

BrowserStack records from allocation to closure when video is enabled. Start and
status report that policy; recording cannot be paused or enabled later. Stop
closes the native session before download. Pending recordings return a typed
unavailable error and may be retried as read-only requests. Network logs require
provider support and enabled capture; this contract does not enable network
capture by default. Logs may be absent when there was no crash.

Audit retains the latest 256 receipts for the host lifetime, with static action
names, opaque operation/session IDs, result, duration and typed error code.
Compare stream UUIDs before reusing a cursor after a restart. Capture polling does
not flood the audit. Raw input, provider IDs, upload tokens, URLs and credentials
never enter receipts.

## Evidence and recovery

Interactive screenshots retain the latest 32 captures for the host lifetime.
Run reports and exported provider media survive normal host exit in private
archives. At most eight archives are admitted across restarts, with at most 128
MiB per archive including report reserve. Full storage refuses a new archive;
export and explicitly retire owned evidence before the next run. The host does
not automatically delete retained evidence. Provider recordings also remain on
BrowserStack under its own retention policy.

Provider downloads have fixed trusted origins, bounded redirects/body sizes and
one remaining timeout. Credentials never reach video CDNs. Known provider
secrets and sensitive log lines are removed before private export; other
application content may remain private. Do not publish real app screenshots,
recordings or logs with a public PR.

On a lost reply, host crash or uncertain cleanup:

```bash
horizon --native-reconcile --client /absolute/path/to/client.json
```

The exact private journal records retain quota until closure is positively
confirmed. Reconciliation uses the original provider operation and owned local
receipts, stops sessions/services before retiring uploads, and refuses unknown
or missing ownership. Absence from a provider listing is not release proof.
Repeated creation is not recovery. A leftover operation with no positive cleanup
proof remains a hold and its original finite lifetime is preserved.

Distinguish local tests, live displayed-frame/control evidence, real loopback
requests, finalized recordings and crash/cancellation proof when reporting.
Authenticated workflows, payments and app feature parity need their own recipes;
an anonymous shell smoke does not qualify them.


Media downloads admit at most two requests per controller. A third request returns
`app_media_busy` before the provider request. Downloads do not hold action locks.
Cold app launches receive up to 60 seconds within the original session deadline.
Other commands retain the normal 15-second limit. Horizon does not retry mutations.

Reports include every allocation handle used by reset steps. Each media entry names
its allocation. The final provider dashboard link names the final allocation.
The controller retains at most 32 verified provider references, including both live
lanes. Earlier references can expire or leave this history; their media entries
then report an explicit error. A reset does not replace earlier evidence silently.
Video finalization polls for up to 30 seconds after closure, within the run deadline.
A pending video remains an explicit failure after this budget.


Keep the shared journal and registry on the same qualified filesystem. The registry
records device and inode identities. Copied state, different device numbers, a
changed mount, or a missing registry or state root causes an ownership refusal.
Preserve the original state and evidence. Do not change the owner, recreate the
registry, or stamp new identities to bypass this refusal. Filesystem migration and
registry repair require separate qualification.
