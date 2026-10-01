# Remote browser orientation: permanent qualification procedure

Use this procedure for the complete feature and for future behavior changes.
The [feature document](../architecture/remote-browser-orientation.md) defines
expected behavior; the exact candidate's PR records what actually passed, failed
or was blocked. Keep this procedure after validation.

Run-specific qualification results are recorded on the PR. For the initial
[PR #1183](https://github.com/peters/horizon/pull/1183), the user authorized
physical/UI smoke after settled green exact-head CI, followed by a final review
approval request with the completed evidence. Resolve known actionable findings
and complete local review before testing the candidate. Future PRs follow the
repository's review and UI validation order unless the user authorizes a different
sequence.

## Choose coverage for a change

Always run the repository's complete pre-push validation matrix in the final
worktree. Deterministic mocks cover unsafe or unreliable failures; a passing mock
is not physical-device qualification.

| Changed behavior | Deterministic coverage to refresh | Physical/UI lanes to repeat |
| --- | --- | --- |
| Config, import/export, create override or provider mapping | Protocol remote config, core remote profile, host/create queue, MCP create and remote startup | Baseline, configured start, override, catalog start and release |
| Startup readiness or lifecycle | Startup pending/failure, unavailable/opposite/unsupported geometry, cancellation before/during measurement, confirmed/uncertain exact release | First snapshot on all explicit start lanes, baseline compatibility and release |
| Device/page measurement or frame verification | Allocation support discovery, committed-page measurement and driver orientation tests, stale/contradictory frames, absent APIs, deadlines | Three round trips per device, geometry samples, no-op and UI round trip |
| Refs, visual origin, scroll or document identity | Navigation and semantic invalidation, edge coordinates, delayed document commit, input during rotation | Edge clicks, scroll/fill, keyboard, navigate/reload/back/forward and fresh refs in both orientations |
| Host publication, ownership or cloud status | Coordination publication, lock consuming deadline, takeover/cancel, supersession, timeout tombstones, ledger saturation/eviction, worker queue refusal | MCP plus UI, polling after completion; two-viewer cloud lane where configured |
| UI presentation | Measured selection, progress, nonfatal error, disabled/wrapped controls | Buttons, narrow/wide panel, host resize/fit, screenshots, native video and final GIF |
| CLI transport/reporting | Direct executable measured success/refusal and durable output, create plan parity | Authorized direct CLI round trip and plan on owned candidate |

The complete qualification requested for #1182 runs every applicable section,
not just the lanes selected for a small later change. Report each unsupported
platform or unavailable environment separately; do not mark it passed from CI.

## Adding a provider or extending the feature

Use the same assertions below for every provider. The BrowserStack device matrix
is an initial qualification example; substitute the new provider's returned
catalog targets, credential references and reported device/OS evidence. Keep the
shared UI, MCP and CLI contract and exact-release assertions unchanged.

1. Document the provider's allocation capability mapping, orientation endpoint,
   supported device/browser combinations, credential requirements and unsupported
   cases in the feature document. Reuse the shared adapter and credential policy.
2. Add deterministic adapter tests for portrait, landscape, omitted orientation,
   create override and conflicting extension keys. Test supported, unsupported,
   malformed and unavailable replies without allocating a live device.
3. Exercise explicit startup agreement and rejection, committed-page measurement,
   pending navigation, cancellation, timeout and confirmed/uncertain release.
   Assert typed failures and credential/session-id redaction through each interface.
4. Qualify one available physical device per supported OS/browser combination
   through configured and catalog starts. Run the runtime, input, document, UI and
   release sections below; repeat three portrait/landscape round trips per device.
   Allocate sequentially and reconcile uncertainty before another create.
5. For an added operation or status field, extend the change-to-test matrix,
   deterministic regressions, UI controls, public MCP/CLI examples and cloud
   propagation assertions together. Check older configuration compatibility and
   unsupported providers explicitly.
6. Record exact candidate hashes, observed geometry, device evidence, interface
   outcomes and release results in the PR. Distinguish mock coverage, physical
   qualification and host-platform checks, and list unavailable lanes honestly.

## Candidate and environment

- Record PR number, candidate SHA, frozen executable paths and SHA-256 hashes.
- Build and freeze `horizon` and its public browser MCP server from that SHA.
- Use a task-owned private state directory and isolated desktop. Follow
  `scripts/device-smoke/README.md` with `--native-view`.
- Verify the actual application child PID and `/proc/<pid>/exe` hash.
- Create a native VNC Device panel in the calling agent's current workspace
  through public `device_panel`, using only the fixture's numeric loopback
  address. Keep the task-owned viewer id and fixture cleanup manifest.
- Establish presentation with at least three timestamped inspections two
  seconds apart while synthetic output changes. Confirm connected,
  image_received, image_displayed and advancing frame_sequence. Use the
  bounded recovery procedure; never restart the user's Horizon.
- Control browser pages only through public `browser_*` MCP tools associated
  with the isolated candidate host. No raw driver endpoints, browser-control
  CLI, developer-desktop input, private runtime edits or noVNC.
- Use the sequential device matrix below. Check safe provider usage before each
  allocation and after release. Do not consume capacity already in use by others.
  An unknown allocation/release blocks further creation until reconciled.
- A provider credential must already be available through authorized existing
  bindings. Never place credentials in fixtures, logs, screenshots or PR text.
- Use a generic synthetic orientation page with a responsive grid, orientation
  label, ticking counter, button counter, scroll target and input field. No
  real customer, host, repository or operational identifiers.
- Start a recorder scoped to the isolated display before interactions. Decode
  representative frames afterwards. Preserve private video and screenshots.

If the candidate MCP connection, physical tablet, provider binding or native
presentation is unavailable, record that exact lane as blocked. Do not infer
a pass from mocks or use a different browser controller.

## Sequential BrowserStack device matrix

Use one owned session at a time. Every row starts from a fresh public
`browser_list`, passive usage/catalog inspection and verified cleanup of the
previous row. A configured target is a private fixture copy; do not change the
user's target or credential binding. Log physical identity, actual model and
resolved OS version from provider evidence, without raw session ids or secrets.

| Lane | Start request | Required checks |
| --- | --- | --- |
| B0 | Configured iOS tablet, no orientation field | Default create, safe status, measurement, input, fixed-viewport refusal and exact release |
| B1 | Configured iOS tablet with landscape | First snapshot landscape; MCP round trips, no-op, navigation/history, geometry and input; UI controls and release |
| B2 | Same landscape target overridden to portrait | First snapshot portrait; override leaves saved target unchanged; UI round trip, narrow/fit checks, authorized CLI parity and release |
| B3 | Exact returned physical iOS tablet catalog reference, landscape | First snapshot landscape; three MCP round trips with full input/navigation checks and release |
| B4 | Exact returned physical Android phone/tablet catalog reference, landscape | Cross-OS first snapshot, three round trips, page/screen/visual geometry, edge input, scrolling, keyboard and release |

Select an available Android target advertised by the configured provider, with
verified physical evidence and orientation support. If the account/catalog cannot
provide it, report B4 blocked with the value-free reason. Do not invent a catalog
reference or silently substitute an emulator. The host UI lane runs on the actual
isolated host OS; iOS/Android remote coverage does not qualify macOS/Windows host
graphics. Cross-machine lanes use the repository's PR-comment handoff procedure
after local review and stable CI.

## Reproducible fixture and assertions

The synthetic page must include a mobile viewport meta tag, responsive grid,
visible orientation/dimensions, a ticking counter, uniquely named left/right and
bottom buttons, an exact click count, tall scroll region and labelled text input.
Use a second synthetic page with a different title and a link back for navigation.
Serve only public generic content through an authorized route or use a supported
data URL. Keep the source with the private run evidence so another executor can
replay it. A loopback URL on the host is not reachable from a provider device.

For each sample, use `browser_snapshot`/`browser_query` for content and input refs.
Use `browser_evaluate` only for the geometry that semantic tools cannot report:
`innerWidth`, `innerHeight`, `visualViewport` width/height/offsets/scale,
`screen.orientation.type` and `window.orientation`. Record absent APIs explicitly.
Wait for the intended committed document and dismiss the keyboard before treating
width/height as independent orientation evidence. Capture the rendered page and
chrome with the isolated native desktop controller. Use `browser_video` only when
the panel advertises that capability; the public MCP contract has no screenshot
operation. Do not capture the user's desktop or inspect private runtime files.

Accept a rotation only when its tool result, current support/applied status,
measured nonzero geometry and live native image agree. In landscape both inner
and visual width exceed height; in portrait both are smaller. Any reported screen
orientation also agrees. Record elapsed acknowledgement time for each rotation
and min/median/max over the run; do not invent a latency threshold or pass from
an average when an individual request failed. Inspect browser audit for ordering,
identity and refusal evidence before closing the owned session.

## Baseline and start orientation

1. Create a remote tablet without an orientation override. Confirm the panel
   reports provider-confirmed physical device identity and observed
   orientation/support. Verified requires device, inner viewport and visual
   viewport agreement on the committed document. Allocation discovers support without measuring its temporary page; startup
   publishes unknown applied orientation before navigation. A default pending start stays unverified until commit.
   An explicit configured or per-call orientation must fail and release when the
   first page is pending, failed or unmeasurable; allocation-page evidence must not
   make it Ready. Cover forced cases with deterministic startup tests. Missing
   config fields retain existing behavior.
2. Close that owned session and verify release. Do not retry an unknown
   allocation; reconcile its exact reference through public tooling first.
3. Configure the private tablet target with landscape. Create it, take the
   first semantic snapshot and measure `innerWidth`, `innerHeight`,
   `visualViewport` dimensions/offsets/scale and screen orientation. Require
   width greater than height and reported landscape before readiness.
4. Override the configured landscape target with portrait through
   `browser_create`. Require measured portrait and prove the saved private
   target was not changed by the per-session override.
5. Discover a physical tablet using `browser_provider_devices`, then create
   that exact returned catalog reference in landscape. Confirm identity,
   first-snapshot geometry and orientation status as in step 3.
6. Verify that orientation without target and invalid orientation strings
   fail before allocation. Local Chromium/Firefox/Safari retain their existing
   create defaults.

## Runtime round trip and geometry

1. Record current orientation and take fresh semantic refs on the fixture.
   Complete at least three portrait → landscape → portrait round trips on each
   physical catalog device. Save every returned action id and viewport measurement;
   a later status sample must not stand in for a failed or missing acknowledgement.
   Exercise the first runtime rotation immediately after startup as well as after
   scroll, fill and keyboard dismissal. Deterministic tests must distinguish a
   stale cached identity found before mutation from a document change after
   dispatch, and stop, Teach mode or timeout during the bounded baseline read.
   Exercise retained native-reference validation through rotation, resize, scroll
   and fill. Include a synthetic page button that reloads the same URL, with the
   same forged page marker on every load. Activate through `browser_act`, without
   an explicit Horizon reload, then require a new generation and old-ref refusal
   before acquiring new refs. Do this on each provider/OS/browser combination.
   In deterministic classic tests, return identical forged page markers for two same-URL roots and
   require fresh generations and stale-ref rejection. Change the native anchor
   during a scan and between URL reads: neither result may be registered.
   A typed stale event must invalidate even when a replacement reuses its ID;
   unknown errors and unsupported or malformed names must not renew the reference.
   Test delayed URL/name/find components under one original bound.
   Run `cargo test -p horizon-browser webdriver::session::document` and
   `node --test scripts/browser-smoke/*.test.cjs`. A retained root that remains
   connected after reparenting or same-Node adoption can stay valid. These are
   explicit classic-protocol limits, not qualified isolation guarantees; new
   providers must document their lifecycle semantics.
   Also replace the document after measurement and during the final ownership
   observation. No success may reuse the old viewport. Cover Stop/Teach,
   owner/handoff takeover and the original deadline during final document reads,
   as well as unavailable ownership observers. A read-only ownership observation
   must preserve queued actions and legacy handoffs, enforce host adoption and
   lease expiry, and fail closed on missing or malformed manifests.
2. Rotate portrait to landscape through `browser_orientation`. Require the
   tool's applied orientation and measured viewport to agree. Observe the
   final frame live; compare the page with the Device-panel image.
3. An old ref must be refused after rotation. Reacquire a button ref and click
   once. Require exactly one increment on the visible counter.
4. Query a button near each edge and activate using fresh refs. Verify correct
   targets after rotation, including CSS/pixel scale and visual origin. Check both
   side edges and the bottom control. Each accepted click increments exactly once;
   a rejected stale ref leaves the counter unchanged.
5. Scroll to the synthetic target and fill its input. Verify values and scroll
   position through semantics and displayed pixels. Dismiss any on-screen
   keyboard before independent orientation geometry assertions.
6. Rotate back to portrait, reacquire refs and repeat the click/scroll/input
   checks. Confirm frame proportions and panel letterboxing update.
7. Rotate to the already-applied orientation: report measured success without
   duplicating input or changing navigation.
8. Navigate to a second synthetic page after rotation. Confirm session
   orientation persists and fresh snapshot/input coordinates remain correct.
   Cover synchronous and delayed document commits, plus contradictory or missing
   geometry in deterministic tests: the new document must not inherit Verified
   from the old document.
9. Exercise reload, back and forward as well as navigation. Reacquire refs after
   each commit, remeasure the document and repeat an edge click. Verified from the
   old document must clear while the new one is loading. Cover delayed commits and
   same-URL reloads in mocks where provider timing cannot reproduce them reliably.
10. Resize the application window and canvas panel, pan/zoom and fit the canvas.
   Test narrow and wide panels in both orientations. Confirm device orientation
   remains unchanged, chrome wraps and page proportions remain correct. Resize
   the isolated desktop only if its controller advertises support; otherwise mark
   that operation unsupported and still test application/panel resize and fit.
11. Record screenshots after launch and resize/fit, with final candidate hash.

## UI buttons and direct CLI

- On the remote panel, use the Portrait and Landscape device icons beside the
  recording controls for a full round trip. Check their named tooltips and
  accessible labels, selected state and legibility in both themes, including
  while recording is active.
  Require Rotating while awaiting evidence, and change the selected button only
  after measured device/page/frame agreement. Observe a fresh responsive layout,
  reacquire semantic refs and activate an edge control after each rotation.
- Ensure controls wrap within narrow panels and remain legible after resize/fit.
  Hide remote rotation controls on local browsers, including cloud-hosted local
  Chromium/Firefox. Host requests for those browsers must return
  `orientation_unsupported` without queuing a rotation or entering pending state.
  Keep rotation available for cloud presentations with an actual provider target.
  Disable controls while starting,
  stopped, rotating, unsupported, and while Teach mode is active.
- A failed rotation must show an actionable error without discarding the browser
  frame or turning a usable panel into a fatal error. Inspect before retrying.
- Execute `horizon-browser orientation <owned-panel-id> landscape` and restore
  portrait, including `--timeout-millis` and `--output`. Require MCP-equivalent
  applied orientation/viewport and durable report evidence. This CLI lane needs an
  explicitly authorized test executor; the implementing agent continues using
  only public browser MCP tools for browser interaction.
- With two authorized cloud viewers, rotate from the second while the first waits;
  the first must receive superseded status and recover when the active rotation
  settles. Deterministic tests cover driver/worker queue refusal, expiry followed
  by unrelated or stale polls, matching acknowledgement and another viewer
  finishing after ledger eviction. Both completion histories retain their
  freshest results across repeated snapshots. Use existing configured cloud
  infrastructure only; no implicit provisioning.
- Capture the UI button round trip in the final native-viewed PR GIF.

## Refusals and recovery

- On an endpoint known not to implement orientation, require unsupported
  status and typed `orientation_unsupported`. No fallback resize or extra
  allocation is permitted.
- Require `browser_resize` on a physical remote device to continue returning
  `remote_viewport_fixed`; local resizing must still work normally.
- A start request the endpoint ignores must fail with a typed mismatch before
  readiness and attempt release. Confirm uncertain release retains its hold.
  Use a mock transport for forced failure; do not manipulate provider accounts.
- Exercise delayed page acknowledgement, navigation during rotation,
  replacement requests, ownership loss, timeout after POST, and panel close
  through deterministic mock tests. A real timeout may have changed the
  device: inspect before retrying, and preserve reported uncertainty.
- Cover a lost POST reply and expired POST/acknowledgement deadline. Require the
  original typed failure to remain unchanged while read-only device/page
  remeasurement recovers applied status after the pending request settles.
  Assert that recovery issues no second orientation POST.
- Hold a rotation POST behind a reply latch. Before releasing it, require
  pending UI status and cleared coordinated applied orientation for both agent
  and user requests. Publication that consumes the action's deadline must return
  `orientation_timeout` without sending the rotation POST.
- Activate Stop or Teach while coordination publication is latched, then release
  it before the deadline. Both user and agent requests must fail without POST,
  clear pending state, publish the terminal error and retain failed audit evidence.
- Unsupported, transient and malformed GET replies must remain distinct;
  do not advertise success from dimensions alone when device evidence differs.

## Interface and persistence parity

- Execute the same create/rotate/get/restore sequence through a CLI plan
  calling the public MCP contract on the candidate. Record measured results,
  not merely plan parsing.
- Cover the desktop host and cloud worker create override through focused
  tests. If live cloud qualification is required but unavailable, report it
  explicitly; never provision a cloud implicitly.
- Load a config predating orientation, export/import a normalized target and
  round-trip optional portrait/landscape values.
- Restart only the task-owned isolated candidate. Restored remote sessions
  must retain the documented stopped/reconnect behavior and avoid implicit
  allocation. Verify the selected target config round-trips.
- Inspect safe panel/list/audit status for both configured and catalog targets.
  No credential values, endpoint URLs or provider session ids may be exposed.

## Evidence and cleanup

Create a private machine-readable ledger and a concise human report. Each entry
records lane/step, timestamp, exact commit, executable hash, panel correlation,
requested/applied orientation, viewport and optional API measurements, elapsed
acknowledgement time, audit action id, result (`pass`, `fail`, `blocked`,
`unsupported`) and evidence file. Retain fixture source, recorder scope and
native-view health observations with it. Do not put raw provider session ids,
credentials or secret-bearing responses in the ledger or public report.

The PR report lists every matrix row, regression tier and UI/CLI/cloud/platform
lane with actual results, plus release outcomes and any unperformed operations.
Link this permanent procedure and the feature document. When reporting a completed
requested cross-machine lane, use the exact repository `SMOKE-TEST REPORT` format
and final marker. A prepared plan, process liveness or successful build is not a
completed smoke report.


- Keep timestamps, candidate SHA/hash, device evidence, measured geometry,
  support status and per-step pass/fail/blocked results in a private report.
- Copy panel-owned exports to the private evidence directory before closing
  the panel; close may delete its capture directory. Decode representative
  recorded frames to prove the round trip and movement
  were captured. Make a synthetic GIF only after the complete flow passes,
  using palettegen/paletteuse at roughly 6 fps and 1000 px width, below 10 MB.
- Inspect frames for private data, attach the final GIF through `gh --attach`
  and verify the PR body contains a user-attachments URL. Do not commit media.
- Any behavior-changing push invalidates smoke and GIF evidence. Repeat the
  affected lanes on that head after refreshed review and checks.
- Close owned browser sessions and verify release/reconcile uncertainty.
  Close only the task-owned Device panel. Close the exact candidate normally
  and clean only fixture-owned processes/state; prove children have exited.
- Do not merge, release, restart active user sessions or clean shared worktrees.
