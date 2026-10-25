---
name: horizon-browser
description: Control, inspect, or audit Horizon browser panels through public browser_* MCP tools.
---

# Horizon browser control

Native VNC Device panels, simulators, and isolated desktop tests use the
`horizon-device` skill. The workflow below applies to browser pages.

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
reveal a helper panel as a workaround. You may hold one panel per browser
engine: to run the same check in another engine, call `browser_create` with an
explicit `backend` (`chromium`, `firefox` or `safari`) beside your open panel.
Only when the user explicitly requests another independent browser session may
you call `browser_create` with `allow_additional: true`; a second panel of the
same engine, a create without `backend` while you own a panel, and a remote
`target` while you own a panel need it. A create that is still pending counts
as a panel of its engine. Omit `backend` to use Horizon's
configured browser, or select `chromium`, `firefox`, or `safari` when the
platform supports it. Read `automation_disclosure` on `browser_list` and
`browser_panel`. In the UI, hover the local backend picker or the remote identity
header to read the same status. `common_signals_minimized` means the native
`navigator.webdriver` getter is still native. On Firefox that result needs
`firefox_system_access: true` and geckodriver 0.37 or newer. The option passes
geckodriver `--allow-system-access`. Mozilla documents that flag as full
system access for any local client that can reach the driver port. Leave it
false unless that privilege is acceptable. `preload_fallback` means Firefox
installed a script getter that returns false. Sign-in pages can reject that
getter. The default minimized Firefox session uses that fallback. Chromium
does not install it. A remote Firefox or Chromium session reports
`unsupported_by_backend` for minimization. Remote Firefox does not clear the
native flag or install the preload. Remote Chromium does not receive the
local automation flag. `unreported` means an older manifest omitted the
field. That is not an established result. To run at a configured remote target instead of a
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
nothing usable is listed, create a panel rather than guessing an id. On a cloud
worker, your injected actor must match a registered workspace session. For a
request addressed to the current worker, an actor outside that workspace receives `panel_outside_workspace` before a browser starts.
Use the registered session identity; do not retry an unregistered actor. Before
interacting, call `browser_snapshot` or `browser_query` and prefer its
short-lived `ref` in `browser_act`. Remote iOS clicks and field entry use
synchronous target checks before trusted native input. Verify the visible result
after each action. `browser_evaluate` works on a driver that omits its script
timeout, and it awaits a returned promise: a rejection is a `javascript_error`
and an unsettled promise is an `evaluation_timeout`.
Navigation, another snapshot or query, and
`browser_wait` can invalidate earlier refs, so reacquire a ref immediately
before an action when the page may have changed.

When the user explicitly requests another panel sharing an existing login, call
`browser_duplicate` with the source `panel_id`. The source must be a ready local
Chromium or Firefox panel in your workspace; ownership and handoff guards still
apply. The panels share cookies and persistent site storage, so logging out in
one affects the others. Navigation and input are independent; forms, history,
and live JavaScript state are not copied. This does not authorize helper panels
as a workaround for iframe, popup, dialog, or consent interactions.

Snapshots expose iframe boundaries as `iframe` nodes. On local Chromium and
Firefox, snapshots and queries also return child-frame nodes, including nodes
inside cross-origin frames. Use their returned refs with `browser_act` `click`
or `fill`. A frame navigation makes its old refs stale. If a frame changes during
a scan, the scan returns `stale_reference`; take a new snapshot or query.
Context events from unrelated pages do not invalidate this scan.
Chromium ignores destruction of isolated execution contexts during a scan.
Nested session retirement processes each tracked parent link once and preserves siblings.
A child-frame `click` hit-tests the target inside its own frame before it
dispatches: `element_obstructed` means another element covers the target and
`element_not_hittable` means the point is outside the frame viewport. After the
click the frame document must show that it took the click; `input_failed` means
it never took focus. The target's own focus is not required, because a handler
can hide or disable the control it was clicked on. A verified frame click moves
focus into the frame, so a following `press` reaches the focused element there. Direct selector actions, `browser_wait`, and `browser_evaluate` target the
top-level document. Child-frame
refs do not support `scroll` or `set_files`. Safari and remote sessions scan only
the top-level document. If these tools cannot reach the embedded frame content,
use `browser_handoff` on the original panel only when its capabilities include
`handoff`. Remote sessions do not support manual steering and return
`unsupported_backend`; report that limitation. Do not open a separate panel for
the frame.

Local fills support contenteditable elements, textareas, and input types `text`,
`search`, `tel`, `url`, `email`, `password`, and `number`. Other controls and
read-only fields return `element_not_editable`; use `set_files` for file inputs.
A local fill types the value as trusted key events, so a widget that enables its
control from `keydown` sees typing. It types at most 256 characters that way; a
longer remainder, and characters a key cannot type without another effect, are
inserted as trusted text. A fill then reads the field back and returns
`input_failed` when the page did not keep the requested value. An editable host
is compared by its rendered text, so a line break or a repeated space that the
editor stored as an element or a no-break space still matches.
If an onfocus handler redirects focus or an inert ancestor prevents focus, the
fill returns `element_not_focused` before it clears the value or sends an input
event. If an input handler moves focus during clearing, the fill stops before
it sends the requested text to the field that receives focus. The check retains
the original element even if the handler transfers its selector to another field.
The focus and clearing checks wait for queued microtasks, including nested microtasks.
If a focus or clearing handler disables the target, fill returns `element_disabled`.
These checks cover native and ARIA disabled state, including queued changes.

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

`browser_http_auth` operations are `set` and `clear`.
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

Passkeys: when the user configured `browser.webauthn_command`, local
Chromium and Firefox panels send a page's passkey request to that external
authenticator command, and the person approves it there. Click the site's
passkey button as usual, then wait for the signed-in page with `browser_wait`.
A human approval can take up to two minutes, so wait again after a
`wait_timeout`. Meanwhile `browser_panel` reports `passkey_pending: true`
and the panel chip shows **passkey request waiting**. Do not retry in a
loop: the bridge allows six requests a minute per panel. Without the setting,
or on Safari and remote sessions, the browser's own passkey behavior applies,
so ask the user to sign in with `browser_handoff` instead.

`browser_network` operations are `start`, `status`, and `stop`.
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
path. Call `operation: status` to inspect the active or last capture without restarting it.
Call `operation: stop` to flush the capture.

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
Use `operation: status` to inspect the active or last recording without changing it.
`browser_video` operations are `start`, `pause`, `resume`, `status`, and `stop`.
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

When the user must steer, first check that the panel advertises `handoff`.
Remote sessions return `unsupported_backend` without starting a handoff or
wait. For supported local sessions, announce what the user needs to do in a
progress message, then call `browser_handoff` with a concise reason. Keep this turn active until
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


For remote devices, set `orientation: portrait` or `orientation: landscape` on
a configured target, or supply `orientation` with `target` in `browser_create`
for a session-only override. Configured and catalog targets use the same option.
An explicit configured or per-call orientation requires matching measured geometry
on the first committed document before readiness. A pending, failed or unmeasurable
first page is rejected with a typed orientation error and exact-session release
attempt; default creates without an explicit orientation keep the pending contract
above.
Check `orientation_support` (`supported`, `unsupported`, or `unverified`) and
`remote_orientation`, then call `browser_orientation` with `panel_id` and
`orientation` to rotate a supported session. The tool waits for the device and
measured page geometry and returns requested/applied orientation and CSS viewport
dimensions. Reacquire refs after rotation. An unsupported endpoint returns
`orientation_unsupported`; an ignored start request returns
`remote_orientation_mismatch` after Horizon attempts release. Inspect uncertain
release before creating again, and inspect the page before retrying a runtime
timeout because the device may already have rotated. Remote resize remains
`remote_viewport_fixed`; orientation does not emulate arbitrary dimensions.

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

For a zoom or reflow check (for example "zoom to 200 percent, nothing
clipped"), inspect the panel's `zoom` capability, then call `browser_zoom` with
`panel_id` and `percent` (25-500), or `reset: true` for 100. Local Chromium and
Firefox support it; Safari and remote sessions return `unsupported_backend`.
The page lays out in a CSS viewport of the panel size, or the `browser_resize`
pin, divided by the zoom, and the panel magnifies the frame, as a browser zoom
level does; `devicePixelRatio` does not change, so resolution media queries and
`srcset` do not follow the zoom. The result gives `requested_percent`,
`applied_percent` and the measured `css_viewport`. Resize and zoom combine: pin
1280x800 and zoom 400 for a 320 CSS pixel reflow check; `browser_resize` then
reports `zoom_percent`. A zoom whose CSS viewport would exceed 8000 pixels per
axis is refused with `invalid_input`; if the panel grows later, the zoomed
viewport stops at 8000 pixels on that axis. The zoom survives navigation until reset
or session replacement. Judge clipping from `browser_screenshot` and from
`document.documentElement.scrollWidth` against its `clientWidth`.

`browser_remote_allocations` operations are `list` and `reconcile`.
For capacity retained after a remote panel disappears, use
`browser_remote_allocations` with `operation: list`, then `operation: reconcile`
and one returned `reference`. This checks only the exact retired allocation
at its original provider. Active, unidentified, or uncertain sessions retain
their holds. Repeated reconciliation is safe; never infer release from an
empty panel list or account-wide session counts. The user can also reconcile
in Settings > Remote browsers.

## Provider discovery and usage

Call `browser_provider_devices` with a configured provider and optional search
words. Read at most 50 returned combinations per page, then use `next_offset`
for another page. Pass the returned target reference to `browser_create` with
`backend` omitted. A catalog entry proves neither entitlement nor capacity.
Call `browser_provider_usage` without a provider for all profiles, or with a
configured provider name. Read sample time, running/allowed counts, queues, and
per-profile errors. Profiles use their own credential bindings; names alone do
not provide isolation. Usage does not reserve a device. Never pass credentials
or raw provider capabilities.

Outside Horizon, `horizon-browser mcp --standalone --config <host-file>` serves
the same tools for the remote targets of a host file. It starts no local
browser, so `browser_create` needs a `target`. Close each session when the task
ends; the host also releases every session when the MCP client disconnects.

A profile host (`horizon-browser host --config <host-file>`) keeps named,
persistent browser profiles on its computer for `horizon-browser mcp --connect`
clients. There, `browser_create` needs a profile name as `target` and takes no
`backend`; `browser_duplicate` opens your page again in the same profile. As for
any target, a second `browser_create` needs `allow_additional: true`. The pages
of one profile share its cookies and sign-ins with every agent of the host, but
`browser_list` shows only your own pages. One agent holds at most 4 pages, and a
page with no action for 60 minutes closes. Close each page when the task ends;
the profile and its sign-ins stay. `browser_screenshot` returns the path of a
private PNG on the host's computer; a client started with
`mcp --connect --inline-screenshots` also gets the PNG as an image block. A
screenshot counts as an action of the page.

## Actions, attachments, and screenshots

`browser_act` operations are `click`, `fill`, `press`, `scroll`, `reload`, `back`,
`forward`, `set_files`, and `drop_files`. A click accepts `count: 1..3`, including
a trusted double-click with 2. Use a fresh ref or selector and examine the visible
result.

For keyboard checks, `press` sends trusted key presses to the element that has
focus: `key` is a named key (`Tab`, `Enter`, `Escape`, `Space`, `Backspace`,
`Delete`, `Home`, `End`, `PageUp`, `PageDown`, arrows such as `ArrowDown`,
`F1`..`F12`) or one printable US-layout character, optionally with `Shift`,
`Control`, `Alt` or `Meta` (`Shift+Tab`, `Control+a`), and `count: 1..100`
repeats it. It takes no ref or selector: click or fill a field first to move
focus there. Local Chromium uses CDP key events, local Firefox WebDriver BiDi
actions, and Safari and remote targets classic WebDriver actions; a real mobile
device may ignore focus keys such as `Tab`. Audit records name non-printable
keys and record printable ones and `Space` only as `printable`; use `fill` for
text. After
the presses, read `document.activeElement` (and `:focus-visible`) with
`browser_evaluate` or take a `browser_screenshot` to judge focus order and the
focus ring.

For `set_files`, target the actual `input[type=file]`, even when hidden.
Use its `file_input` metadata for accept and multiple-file policy.
For `drop_files`, target a visible drop element on local Chromium or Firefox.
Supply 1..32 absolute regular-file paths under the configured agent work root
or an explicitly permitted attachment root. These paths are on the server host.
Do not widen roots or upload private files without task authorization.
Read attached names and sizes for `set_files`, then examine page acceptance.
A dispatched drop does not prove that an application accepted the files.
On a cloud worker, these paths are worker paths. The files of a person's own
computer reach a cloud browser only through Horizon's file picker or a drop on
the panel. A click on an upload button opens that picker for the person who
views the panel, and page actions return `file_chooser_pending` until that
person answers, so use `set_files` on the input instead of clicking the button.

`browser_screenshot` returns retained viewport pixels as a private PNG path and
original dimensions. It requires a ready panel and a live supporting Horizon
host or profile host. It takes no fresh navigation or full-page capture. Optional
`copy_to_clipboard` defaults false; `clipboard_requested` proves host dispatch,
not OS acknowledgement. Capture claims ownership and refuses another live owner,
human steering, or pending handoff. It changes no focus, visibility, or canvas.
Copy needed evidence before panel close or host exit; only eight exports remain.

Casting uses `horizon-cast`. Cloud offers and companions use `horizon-cloud`.
Native iOS and Android sessions use `horizon-app-testing`.
