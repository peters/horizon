# Temporary smoke plan: #1182 remote device orientation

Status: prepared; not executed. Run only after the final candidate has an
explicit exact-head Copilot approval recommendation, as requested by the user.
Delete this temporary plan after all required smoke lanes pass unless a
cross-machine handoff still needs it.

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
- Use a configured physical iOS tablet target and a returned catalog iOS
  tablet target. Check safe provider usage without allocating extra sessions.
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

## Baseline and start orientation

1. Create a remote tablet without an orientation override. Confirm the panel
   reports provider-confirmed physical device identity and observed
   orientation/support. Verified requires device, inner viewport and visual
   viewport agreement on the committed document. A pending start page must stay
   unverified until commit; disagreement or unavailable geometry must not claim
   verified orientation. Missing config fields must retain existing behavior.
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
2. Rotate portrait to landscape through `browser_orientation`. Require the
   tool's applied orientation and measured viewport to agree. Observe the
   final frame live; compare the page with the Device-panel image.
3. An old ref must be refused after rotation. Reacquire a button ref and click
   once. Require exactly one increment on the visible counter.
4. Query a button near each edge and activate using fresh refs. Verify correct
   targets after rotation, including CSS/pixel scale and visual origin.
5. Scroll to the synthetic target and fill its input. Verify values and scroll
   position through semantics and displayed pixels. Dismiss any on-screen
   keyboard before independent orientation geometry assertions.
6. Rotate back to portrait, reacquire refs and repeat the click/scroll/input
   checks. Confirm frame proportions and panel letterboxing update.
7. Rotate to the already-applied orientation: report measured success without
   duplicating input or changing navigation.
8. Navigate to a second synthetic page after rotation. Confirm session
   orientation persists and fresh snapshot/input coordinates remain correct.
9. Resize/fit the host canvas panel and isolated desktop. Confirm device
   orientation remains unchanged and displayed page proportions stay correct.
10. Record screenshots after launch and resize/fit, with final candidate hash.

## UI buttons and direct CLI

- On the remote panel, use Portrait and Landscape buttons for a full round trip.
  Require Rotating while awaiting evidence, and change the selected button only
  after measured device/page/frame agreement. Observe a fresh responsive layout,
  reacquire semantic refs and activate an edge control after each rotation.
- Ensure controls wrap within narrow panels and remain legible after resize/fit.
  Hide remote rotation controls on local browsers. Disable controls while starting,
  stopped, rotating, unsupported, and while Teach mode is active.
- A failed rotation must show an actionable error without discarding the browser
  frame or turning a usable panel into a fatal error. Inspect before retrying.
- Execute `horizon-browser orientation <owned-panel-id> landscape` and restore
  portrait, including `--timeout-millis` and `--output`. Require MCP-equivalent
  applied orientation/viewport and durable report evidence. This CLI lane needs an
  explicitly authorized test executor; the implementing agent continues using
  only public browser MCP tools for browser interaction.
- With two cloud viewers, rotate from the second while the first waits; the first must receive superseded status and recover when the active rotation settles. Cover driver queue refusal and lost-status timeout through deterministic tests.
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

- Keep timestamps, candidate SHA/hash, device evidence, measured geometry,
  support status and per-step pass/fail/blocked results in a private report.
- Decode representative recorded frames to prove the round trip and movement
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
