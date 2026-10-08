# Native app MCP reference

## Prerequisites

The native server is separate from the injected browser MCP.
Register the packaged executable as `horizon --native-mcp --client <private-client.json>`
only when server setup is part of the requested task. The client selects the
project, persistent owner UUID, private state, Horizon configuration, approved
tunnel executable, and its SHA-256. Tool arguments cannot replace these bindings.
Use the repository runbook `docs/architecture/remote-device-testing.md` and its
schema when available; do not invent a client or project contract from this summary.

The host supports Linux and macOS; Windows process execution is unavailable.
iOS builds need an approved Mac with Xcode. Android builds need the declared
SDK and JDK. Only the BrowserStack native provider is supported. Credentials
come from the configured OS store. Configure MCP timeouts for the requested
lifetime, up to 1,800 seconds, and enable progress notifications.

The app declares builds, matrix, synthetic backend, ports, and recipes in
`AGENTS.md`. Run at most two lanes within the approved App Automate quota.
A fixed backend port requires one lane at a time. No production app distribution
or production backend use is authorized by a test request.

## Matrix and interactive tools

| Tool | Use |
|---|---|
| `device_test_run` | Run the declared matrix with `lifetime_seconds`. |
| `app_upload` | Upload one existing declared `platform` artifact; retain the artifact handle. |
| `app_session_create` | Start `matrix_index` with that artifact and a bounded lifetime. |
| `app_snapshot` | Read the native tree; retain fresh refs for that session. |
| `app_act` | Run an action, assertion, gesture, or app lifecycle operation. |
| `app_wait` | Wait for a target state within the original session deadline. |
| `app_screenshot` | Get PNG pixels and a private evidence handle. |
| `app_view` | Get the owned session's read-only loopback VNC endpoint. |
| `app_logs` | Get available device, crash, Appium, or network logs. |
| `app_video` | Read policy, get video, or close and download with `stop`. |
| `app_tunnel_status` | Read redacted tunnel readiness and ownership. |
| `app_audit` | Read bounded receipts with `after_sequence` and `limit`. |
| `app_session_close` | Get exact closure acknowledgement and clean up owned services. |

Use opaque artifact and session handles, never provider IDs.
For interactive tests, build each declared artifact first, then upload once per
platform and create the selected matrix entry.
Open `app_view` and attach its returned endpoint through `device_panel`.
Attach through an authorized SSH route when the endpoint is on another host.
The viewer stream samples at most once per second and does not change native geometry.
It stops on session closure, expiry, or host exit.

Targets use identifiers, labels, refs, or finite device coordinates.
A ref belongs to one session and one fresh snapshot. Another snapshot invalidates
it. Android named-element resolution also takes a new snapshot. Input requires
one exact match and unchanged native identity. Action kinds are `tap`, `long_press`, `type`, `clear`, `swipe`, `scroll`,
`wait`, `assert`, `back`, `home`, `rotate`, `launch`, `terminate`, `reset`,
`deep_link`, and the recipe-only `screenshot`. Interactive `app_act` refuses
`screenshot`; use `app_screenshot` instead. Use the action schema for each operation.
`reset` closes the original session and services, then returns a replacement
session handle. Use that handle for subsequent input, views, evidence, and cleanup.
The original deadline and upload remain; reset does not extend the lifetime.
A target uses `by: identifier|label|ref|coordinates` and its matching `value`. Observe the result after input;
a driver acknowledgement does not prove application success.

## Evidence and recovery

`app_video` operations are `start`, `status`, `get`, and `stop`.
The provider's enabled video covers allocation through closure. `start` and
`status` report policy; they do not start a new recording mid-session.
`stop` closes the session before download. Video can be pending; only read-only
downloads can be repeated safely. Network logs need configured provider capture.
No crash can mean no crash log. Missing evidence is not a passing lane.

Keep screenshot, log, video, and audit evidence private. Secure fields are redacted,
but other application content can still be sensitive. Screenshots retain the
latest 32 captures. Audit retains 256 receipts; compare stream UUIDs before reusing
cursors after a restart. These limits do not replace a durable test report.
Record step results, artifact identity, actual concurrency, live presentation,
and provider closure separately.

After uncertainty, keep the original owner, operation identity, and private state.
Do not replay a mutating tool, change ownership, remove a journal, or allocate a
replacement to bypass admission refusal. Follow the runbook's recovery procedure.
Close only the owned session and viewer. Examine exact provider closure separately
from local cleanup. Report unresolved closure and held capacity explicitly.
