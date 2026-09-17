---
name: horizon-browser
description: Control, inspect, or audit a live browser panel inside Horizon through the Horizon browser MCP tools.
---

# Horizon browser control

Use the `browser_*` MCP tools as the only agent-facing browser contract. Do
not inspect Horizon runtime files, connect to raw CDP/BiDi/WebDriver endpoints,
or invoke a browser-control CLI. If the MCP tools are unavailable, report that
the Horizon browser MCP server is not connected.

Start with `browser_list` when the panel id is unknown. If it returns no panels,
call `browser_create`; this opens a panel in the current agent's Horizon
workspace and returns its ready panel id once the backend is ready and, when
you passed a `url`, once that page committed (`navigation: committed`). A
`navigation: pending` result means the panel is controllable but the first
page had not committed within the bounded startup wait, so use `browser_wait`
or `browser_panel` before reading it; `navigation: failed` means that page
failed to load (`navigation_error` says why) and you must navigate again or
fix the URL; `navigation: superseded` means the user navigated the panel
first, so read `panel.url` before acting. If `browser_list` returns a usable panel, reuse
that panel for iframe, popup, dialog, and consent interactions. Never create or
reveal a helper panel as a workaround. Only when the user explicitly requests
another independent browser session may you call `browser_create` with
`allow_additional: true`. Omit `backend` to use Horizon's
configured browser, or select `chromium`, `firefox`, or `safari` when the
platform supports it. To run at a configured remote target instead of a
local browser, pass `target` with its name and omit `backend`; Horizon
resolves the provider and credentials from its configuration, and a
refusal carries a typed code and at most the target, provider or credential
reference name, never a credential value. Such a panel reports
`remote_target`, `remote_device` (the model, OS version and hardware
evidence the provider itself reported, verified against the target before
the panel became ready), classic WebDriver and no network capture. A target
that requires a physical device is refused as `remote_device_rejected` unless
that evidence confirms it, after Horizon attempts to release the session; a
`remote_allocation_unknown` refusal means a device may still be held, so check
with the user before creating again; `remote_authentication_failed`,
`remote_not_entitled` and `remote_device_unavailable` say which of the
credential, the account's automation access or the device request the provider
refused, with nothing held. Cite `remote_device`, not the target
name, as real-device evidence. Set `visible: false` for background automation; use
`browser_visibility` to show or hide the live panel later without stopping its
session, capture, ownership, or MCP control. Call `browser_close` on a panel
you own when the user is done with it or a remote device session must be
released now; it stops the session, releases any remote allocation, and the
panel leaves `browser_list`. Read anything you still need from
`browser_audit` before closing: it answers only for a live panel. An optional bare-host `url`
defaults to HTTPS while explicit HTTP remains available. Use `browser_panel`
for a known panel. Discovery and control are scoped to the workspace that
contains your agent panel: `browser_list` never shows panels from other
workspaces, every other tool rejects their ids, and a panel's `visible` field
is host presentation state, not proof that the panel is in your workspace. If
nothing usable is listed, create a panel rather than guessing an id. Before
interacting, call `browser_snapshot` or `browser_query` and prefer its
short-lived `ref` in `browser_act`. Navigation, another snapshot or query, and
`browser_wait` can invalidate earlier refs, so reacquire a ref immediately
before an action when the page may have changed.

When the user explicitly requests another panel sharing an existing login, call
`browser_duplicate` with the source `panel_id`. The source must be a ready local
Chromium or Firefox panel in your workspace; ownership and handoff guards still
apply. The panels share cookies and persistent site storage, so logging out in
one affects the others. Navigation and input are independent; forms, history,
and live JavaScript state are not copied. This does not authorize helper panels
as a workaround for iframe, popup, dialog, or consent interactions.

Snapshots expose iframe boundaries as `iframe` nodes. If the current top-level
semantic tools cannot reach the embedded frame content, call `browser_handoff`
on the original panel so the user can complete the interaction; do not open a
separate panel for the frame.

`browser_navigate` returns a typed outcome: by default it waits until the
document committed and reports `committed_url`, `title` when known, `loading`,
`redirected`, and `state`. Check `completed`; a `timed_out` state carries the
latest page state so you can inspect or retry, and `wait: dom_content_loaded`
or `wait: dispatched` (handed to the backend, browser acceptance not awaited)
change how long it waits; `timeout_millis` is raised to
at least 1000 ms, and on Safari every wait returns once the page loaded or the
bound elapsed. After navigation or
interaction, verify the visible outcome with `browser_wait`, `browser_query`,
or a new snapshot. `browser_wait` is one audited engine-side action that
observes the page itself: it returns the matched nodes and `elapsed_millis`,
and fails with a typed code (`wait_timeout`, `wait_navigation_invalidated`,
`wait_ownership_lost`, `wait_handoff_pending`, `wait_superseded`,
`browser_unavailable` when the backend stops) instead of looping on queries,
so do not poll it in a tight loop; pick a `timeout_millis` that covers the
expected change. Use `browser_evaluate` only when the semantic tools cannot
answer the question.

If a page presents HTTP Basic or Digest authentication, call
`browser_http_auth` with `operation: set`, the username and password the user
supplied, and `origin` (`http://host[:port]` or `https://host[:port]`) when
known, before `browser_navigate`, or set then reload if the protected page is
already open. If origin is omitted, it binds to the current page origin and
fails when the page has none. If the user has not supplied credentials, ask for
a username and password instead of guessing. The engine provides those
credentials only to matching server challenges for that origin on local
Chromium and Firefox. Do not put the password in
`browser_evaluate` or audit commentary. Safari and remote sessions return
`unsupported_backend`. Call `operation: clear` to drop live-session credentials
for later intercepted challenges; it does not revoke Authorization values the
browser already cached, so open a new panel for a clean unauthenticated
session.

For HTTP or WebSocket observation, first inspect the panel's
`network_capture` field from `browser_list` or `browser_panel`. When supported,
call `browser_network` with `operation: start` **before navigation** so open,
frames, errors, and close are all observed. Use URL filters and payload/file
limits for busy streams. To capture HTTP response content, set both
`include_http: true` and `include_http_bodies: true`, and check
`http_response_body_transport` first. Bodies appear as bounded
`http_response_body` records; they may contain sensitive page data and never
belong in the action audit. The result returns live connection counters and one
private NDJSON export path. Prefer `browser_network_watch` for event-driven
monitoring: filter by URL and event kind, leave payloads excluded unless needed,
then pass the returned `capture_id` and `next_sequence` into the next call. It
reports timeout, capture stop/replacement, gaps, drops, truncation, file limits,
and writer failure explicitly. For sustained local analysis, it is also safe to
inspect the exact path returned by `browser_network` with read-only tools such
as `tail -f`, `jq`, or `rg`; never infer or inspect another Horizon runtime
path. Call `operation: stop` to flush the capture.

For page-pixel recording, inspect `video_capture` then call `browser_video`
with `operation: start`. Optional start-only knobs: `quality` (1-100),
`compression_level` (0-10, higher is slower/smaller), `fps` (1-30),
`max_width` (320-1920, caps the longest encoded side), `max_file_bytes`.
Omitted options keep the host `browser.video` settings. The host defaults are
quality 90 and source-frame sizing with codec-block alignment, bounded by a
3840-pixel longest side and 8,294,400 pixels (4K); larger frames are downscaled
proportionally. An explicit
host size cap remains active when a recording omits `max_width`. These
encoding settings do not resize the page viewport. Pause skips time in the file;
resume continues the same WebM; stop finalizes a private `.webm` path.
Page pixels never enter the action audit. The recording samples the existing
decoded frame slot on Chromium, Firefox, and Safari.

Chromium HTTP bodies and WebSocket frames are protocol-native, but CDP cannot
return a `fetch()` body the page drained with `response.blob()`; that
`http_response_body` record carries an `error` and no `payload`, so when the
bytes matter, read `text()` or `arrayBuffer()` or leave the body unread. A
top-level navigation to a PDF is different: it captures the viewer's HTML
shell as a normal successful body, never the PDF bytes. Firefox HTTP
bodies are native WebDriver BiDi, while WebSocket frames use page
instrumentation because standard BiDi does not expose them; the panel
advertises both distinctions. Safari network capture is currently unsupported.
Do not describe Firefox WebSocket instrumentation as undetectable.

When the user must steer, announce what they need to do in a progress message,
then call `browser_handoff` with a concise reason. Keep this turn active until
the user selects **Done — hand back to agent**. Omit `timeout_millis` for the
15-minute human wait; do not substitute a short page-action timeout such as
60000 ms. Leave `wait` true (the default) and stop issuing page actions while
the user steers. Set `wait: false` only for an explicitly nonblocking script.

A yielded or backgrounded tool invocation is still running: keep awaiting that
same invocation using the client's wait mechanism until its result arrives.
Do not send a final response saying you are waiting: ending the turn leaves no
pending call for the Done button to resume.

If the handoff times out, call `browser_panel` once. If `handoff_pending` is
still true, call blocking `browser_handoff` again
with `resume_request_id` from the timeout and the default timeout. This resumes
that request without undoing a concurrent Done click; it renews an expired lease
only if the recorded owner and request still match. Keep waiting in this turn.
If handoff completed,
or the call returns `handoff_pending: false`, take a fresh snapshot and resume
the task without requiring another chat message. Stop on explicit cancellation,
panel closure, lost ownership, or an unrecoverable connection failure and report
the actual condition. Do not poll `browser_list` for hand-back.

Use `browser_audit` to review the
redacted ordered action history or to verify a specific action id. The default
page is the newest matching records (`limit` 1-500, default 100). To iterate
every retained record, call with `from_start: true` and reuse `next_event_id`
as `after_event_id` until `has_more` is false. Treat `cursor_lost`,
`malformed_records`, and `older_records_dropped` as explicit loss.

For responsive layouts, inspect the panel's `resize` capability, then call
`browser_resize` with `panel_id`, `width` and `height` (320-8000 CSS pixels per
axis). Chromium and local Firefox support this; Safari returns
`viewport_unsupported` and remote devices return `remote_viewport_fixed`.
The result contains `requested` and browser-measured `applied` width/height.
The pin survives host layout, visibility changes and navigation in that live
session; the canvas panel letterboxes it. Call `browser_resize` with
`reset: true` and no dimensions to resume the latest host panel size; its
`requested` is null and `applied` is measured too. Session replacement/restart
clears the pin. A timeout/failure may follow a backend mutation: inspect the
page or retry rather than assuming no change. `browser_video` max_width and
codec alignment affect encoding only. Reacquire semantic refs after resizing.

For capacity retained after a remote panel disappears, use
`browser_remote_allocations` with `operation: list`, then `operation: reconcile`
and one returned `reference`. This checks only the exact retired allocation
at its original provider. Active, unidentified, or uncertain sessions retain
their holds. Repeated reconciliation is safe; never infer release from an
empty panel list or account-wide session counts. The user can also reconcile
in Settings > Remote browsers.

## Isolated native application testing

For native application work, use a task-owned virtual desktop observed through a
read-only noVNC page in Horizon. Keep browser interactions on these public
`browser_*` tools; use explicitly configured device CLI/MCP tools only for input
into the native test desktop. A native Device panel is a read-only Rust VNC
viewer, not a browser and not a device-input API. noVNC provides the outer test
view; it is not required by the native panel itself.

Run independent applications in parallel when requested, with a separate unused
display, private application configuration/home, expiring device target, loopback
VNC/web ports, and owned process tree for each fixture. Record which exact target
and process each agent owns. Keep input and screenshot geometry scoped to that
target; never reuse another fixture's coordinates or fall back to the developer's
desktop. A request for independent test viewers permits the corresponding
`browser_create` calls with `allow_additional: true`; it does not permit unrelated
helper sessions. The Horizon source checkout's `scripts/device-smoke/README.md`
describes its fixture and optional project-local device registration. Other native
applications need equivalent isolation supplied by their own test launcher.

Build the intended checkout to completion before launching. Avoid sharing a
Cargo target directory between different source trees during qualification.
Copy candidate executables into a new task-owned directory, record their hashes,
and launch those frozen copies. Verify the executable and hash of the actual
application child; the fixture's recorded launcher PID may be `bwrap`. Preserve
existing application processes. A later rebuild does not update an already
running process; close only the owned candidate normally before replacing it.

Inspect outer noVNC page readiness with `browser_snapshot` or `browser_query`.
Observe native pixels with `device_screenshot` and recorded `browser_video`
frames; drive the native target through `device_doctor`, `device_screenshot` and
`device_act`, or their CLI counterparts. Check actual application output after input: a dispatched action
is not an assertion. For a nested native-panel test, keep the viewer fixture and
the viewed target fixture distinct, with separate device targets. Exercise view
resize, Fit and detach without expecting the read-only image to forward input.

Use existing `browser_resize` for the outer browser viewport and `browser_video`
encoding options for evidence quality. These do not resize the native desktop,
change native VNC frame quality, or provide native screenshot cropping. Native
frame sizing and quality controls are follow-up work; use only capabilities the
connected tools actually expose.

Record a short representative flow with `browser_video`, stop it, and copy the
returned WebM export to private evidence **before `browser_close`** removes its
profile and capture exports. Inspect representative frames and correlate them
with the frozen candidate hash and observed result. Keep internal application
names, scenarios, screenshots, recordings and operational identifiers local;
public examples use only generic Horizon fixtures and synthetic content. Close
only owned browser panels and test applications, then stop their owned fixtures
and verify target expiry and child cleanup. Parallel work must not interrupt the
developer's desktop or another agent's fixture.
