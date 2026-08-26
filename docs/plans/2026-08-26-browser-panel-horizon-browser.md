# Browser panel: first-party `horizon-browser` landing plan

- Status: accepted architecture; implementation not started
- Date: 2026-08-26
- Target base inspected: `origin/main` at `59f426e`
- New workspace crate: `crates/horizon-browser`
- Protocols: Chromium CDP, Firefox WebDriver BiDi, and Safari WebDriver with optional BiDi
- Full browser predecessor: closed PR #298
- Current implementation references: stacked PRs #302–#307
- Supersedes: `2026-08-26-browser-panel-ferridriver.md`

## Decision

Build and own the browser engine in a new `horizon-browser` crate. Do not integrate ferridriver.

The crate will expose one backend-neutral session API with explicit capabilities and three backend adapters:

- `ChromiumCdp`: the first production renderer, using change-driven CDP `Page.startScreencast` frames.
- `FirefoxBidi`: a real WebDriver BiDi transport/session/input backend, landed after the shared API and Chromium path are stable.
- `SafariWebDriver`: a macOS-only `safaridriver` backend using classic WebDriver HTTP for required commands and screenshots, plus BiDi when capability negotiation returns `webSocketUrl`.

CDP does not mean Firefox support. Firefox uses BiDi. The common API must preserve backend differences instead of reducing both protocols to a lowest-common-denominator interface.

Chromium is the first complete visible browser panel. Firefox BiDi support includes transport, process ownership, contexts, navigation, events, input, screenshots, and capability reporting. Firefox becomes a production visible panel only when its rendering lane passes the explicit gate below. Safari is built into the same engine contract before the macOS handoff, but it remains unavailable on non-macOS hosts and requires exact-machine capability and smoke proof.

## Performance answer

Owning the implementation is a sound performance and control decision, but “we stream while ferridriver takes snapshots” is only partly correct:

- Horizon's existing Chromium candidate and ferridriver's Chromium backend both use CDP `Page.startScreencast`. Streaming is not a unique Chromium advantage.
- The current Horizon candidate has useful hot-path properties worth preserving: immediate frame acknowledgement, latest-only publication, a coalesced UI wake-up, allocation-aware JPEG decode, and texture upload only when the frame sequence changes.
- ferridriver 0.5.0's Firefox path uses fixed-rate screenshot capture. A first-party implementation lets Horizon avoid that exact policy.
- Standard BiDi does not currently provide live image frames to the client. Its `browsingContext.startScreencast` command records media to a file and returns the path. Firefox's current implementation uses `MediaRecorder` and a 10-second data timeslice, so it is not suitable as Horizon's interactive frame source.
- A Firefox backend can use `browsingContext.captureScreenshot` adaptively, but that is still snapshot rendering. It must not be described as streaming.
- Safari's documented automation surface is WebDriver. Stable Safari uses isolated automation windows, allows one session at a time, and exposes screenshot capture rather than CDP-style push frames. Upstream WebKit contains partial BiDi support, but the installed Safari must prove `webSocketUrl` negotiation at runtime.

The existing Chromium measurements are promising but are not a ferridriver comparison: approximately 0.15 ms driver time at 403×252 and 2.0 ms at 1840×1000, with zero decode/upload work for an unchanged idle page. Preserve that baseline and require a workload-matched benchmark before making a broader performance claim.

## Architecture decision record

### Context

The browser feature spans protocol transport, child-process ownership, frame delivery, input, persistence, UI, agent handoff, and shutdown. The current implementation proves the vertical feature but is too large to land safely as one tree: the current stack tip differs from refreshed `origin/main` by 67 files and 13,006 changed lines.

The engine also has value outside Horizon's board model. Keeping it inside `horizon-core` would couple protocol code to panels, persistence, YAML configuration, and agent coordination. A third-party driver reduces initial code ownership but constrains the hot path, lifecycle semantics, raw protocol access, and backend rollout.

### Options considered

| Option | Performance control | Protocol coverage | Horizon coupling | Maintenance | Decision |
| --- | --- | --- | --- | --- | --- |
| Own `horizon-browser` crate | Highest; Horizon owns queues, allocations, acks, and frame policy | CDP and BiDi can expose exact capabilities | Low with a strict dependency boundary | Highest direct ownership | Selected |
| ferridriver adapter | Chromium streaming exists, but backend policies and public API constrain tuning | Advertises several engines but behavior differs | Medium | Shared upstream burden | Rejected |
| Keep browser code in `horizon-core` | High | Fully controllable | High; engine and product state remain mixed | Large core surface | Rejected |

### Consequences

- Horizon owns protocol compatibility, browser discovery, and backend regression testing.
- The crate can be tested without egui, sessions, speech, or board state.
- Protocol-specific capabilities remain visible to callers.
- Firefox rendering cannot be declared equivalent to Chromium until a real frame-delivery path passes latency and CPU gates.
- The existing PRs are source material, not commits to merge or cherry-pick wholesale.

## Goals

- Create a reusable, UI-independent `horizon-browser` crate.
- Support Chromium CDP, Firefox WebDriver BiDi, and Safari WebDriver through typed, tested protocol adapters.
- Preserve the existing Chromium candidate's change-driven and latest-frame behavior.
- Keep browser children, profile locks, driver threads, and shutdown ownership deterministic.
- Integrate browser panels into Horizon through a narrow `horizon-core` bridge.
- Split delivery into small, serial, independently testable outcomes.
- Make performance counters part of the engine contract rather than an afterthought.

## Non-goals for the first landing

- ferridriver or another full browser-driver dependency.
- Firefox-over-CDP or Safari-over-CDP.
- Claiming that BiDi file recording is a live frame stream.
- Tailing Firefox's growing WebM recording file as an interactive renderer.
- A browser extension, patched Firefox build, or native-messaging capture companion.
- Direct use of Safari's private Web Inspector protocol.
- Access to a user's normal Safari history, AutoFill data, cookies, or profile; Safari automation remains isolated by design.
- Hardware video decode, WebRTC tab capture, or custom wgpu texture import.
- Moving Horizon persistence, configuration, agent manifests, or egui code into `horizon-browser`.
- Carrying unrelated terminal, speech, or dependency cleanup in browser engine PRs.

## Workspace and dependency boundary

```text
horizon-ui
    |
    v
horizon-core
    |
    v
horizon-browser
```

`horizon-browser` must never depend on `horizon-core` or `horizon-ui`.

### `horizon-browser` owns

- CDP and BiDi wire envelopes, request correlation, subscriptions, sessions, and protocol errors.
- Classic WebDriver HTTP envelopes, session routes, capability negotiation, actions, screenshots, and typed errors.
- Loopback-only WebSocket and HTTP transport, deadlines, cancellation, and bounded event draining.
- Chromium, Firefox, and Safari-driver executable discovery, protected launch arguments, child handles, profiles or isolated sessions, endpoint discovery, termination, and reaping.
- Backend-neutral navigation and input intent plus protocol-specific translation.
- Encoded-frame metadata, reusable decode scratch, latest-frame publication, backpressure, and engine metrics.
- Driver thread/session lifecycle and typed commands/events.
- Capability reporting for frame delivery, input, viewport, clipboard, download, and protocol extensions.

### `horizon-core` owns

- `PanelKind::Browser`, panel identity, board orchestration, and shutdown aggregation.
- Horizon YAML configuration and migration into typed engine launch options.
- Runtime/session persistence and restore policy.
- Agent ownership/handoff manifests under the Horizon runtime directory.
- Stable mapping between engine events and panel state.
- Retry policy visible to Horizon users.

### `horizon-ui` owns

- egui texture creation/update and browser chrome.
- URL entry, navigation controls, status and error presentation.
- Translation from egui/winit keyboard, pointer, wheel, focus, clipboard, and IME events into engine input intent.
- Detached-window routing, fullscreen behavior, and accessibility labels.
- Repaint decisions based on a coalesced engine notification.

### Workspace manifest

The new crate must use workspace version, edition, Rust version, license, lints, and repository metadata. Add both the workspace member and an exact path/version workspace dependency:

```toml
[workspace]
members = [
    "crates/horizon-browser",
    "crates/horizon-core",
    "crates/horizon-cursor",
    "crates/horizon-ui",
]

[workspace.dependencies]
horizon-browser = { path = "crates/horizon-browser", version = "0.2.7" }
```

Keep the version synchronized with `[workspace.package].version`. Do not add protocol feature flags initially: CDP and BiDi share the same small wire dependencies, and compiling both avoids an unnecessary CI matrix. Reconsider only if backend-specific dependencies become material.

## Proposed crate layout

```text
crates/horizon-browser/
  Cargo.toml
  src/
    lib.rs                  public API and capability types
    error.rs                typed engine/transport/protocol/process errors
    command.rs              backend-neutral commands
    event.rs                backend-neutral events
    capabilities.rs         exact backend feature reporting
    driver/
      mod.rs                owned thread and bounded pump
      shutdown.rs           stop, deadlines, completion, forced cleanup
    transport/
      mod.rs
      websocket.rs          loopback connection, timeout, cancellation
      http.rs               bounded loopback WebDriver requests
      pending.rs            request ids, responses, orphan expiry
    cdp/
      mod.rs
      message.rs            CDP envelopes and flattened session routing
      browser.rs            target discovery and attach
      page.rs               navigation, viewport, lifecycle, screencast
      input.rs              CDP input mapping
    bidi/
      mod.rs
      message.rs            BiDi command/response/event envelopes
      session.rs            capabilities and subscriptions
      browsing_context.rs   context lifecycle, navigation, viewport, capture
      input.rs              BiDi action mapping
    webdriver/
      mod.rs
      message.rs            classic WebDriver response and error envelopes
      session.rs            new/delete session and capability negotiation
      browsing_context.rs   URL, title, viewport, history, script, screenshot
      input.rs              W3C actions and release semantics
    process/
      mod.rs
      chromium.rs           Chromium discovery and protected launch
      firefox.rs            Firefox discovery and protected launch
      safari.rs             macOS safaridriver discovery and service lifecycle
      child.rs              exact child ownership, termination, reaping
      profile.rs            task/panel-owned profile lifecycle
    frame/
      mod.rs
      encoded.rs            codec, dimensions, timestamp, sequence
      decode.rs             reusable base64/JPEG scratch and decode buffers
      slot.rs               newest-frame handoff and coalesced wake-up
      metrics.rs            produced/acked/decoded/dropped/published counters
    input/
      mod.rs                logical input intent
      key.rs                physical/logical key and modifiers
      pointer.rs            buttons, coordinates, wheel
  tests/
    support/                mock CDP/BiDi websocket peers and local pages
    cdp_contract.rs
    bidi_contract.rs
    process_lifecycle.rs
```

Keep production modules comfortably below the repository's 1,000-line hard limit and split near 600 lines. Do not recreate the existing 776-line CDP file, 908-line process file, 979-line input file, 900-line browser `mod.rs`, or 938-line session file as equally large files in a new directory.

## Public engine contract

Use concrete enums in the hot path instead of boxed async trait objects. The first implementation can retain the proven single blocking driver thread and synchronous Tungstenite pump; no Tokio runtime is required.

```rust
pub enum BackendKind {
    ChromiumCdp,
    FirefoxBidi,
    SafariWebDriver,
}

pub enum FrameDelivery {
    PushJpeg,
    AdaptiveScreenshot,
    RecordingFile,
    Unsupported,
}

pub struct BackendCapabilities {
    pub frame_delivery: FrameDelivery,
    pub viewport: bool,
    pub physical_keys: bool,
    pub ime: bool,
    pub clipboard: bool,
    pub persistent_profile: bool,
    pub max_sessions: Option<u32>,
}

pub struct BrowserSession {
    // Opaque driver handle and typed command/event boundary.
}
```

The exact final fields should follow spike evidence, but these rules are fixed:

- Callers can query capabilities before showing UI controls or promising behavior.
- Protocol JSON values and session identifiers do not escape the crate.
- Requested navigation and committed navigation remain distinct.
- Transient ports, PIDs, WebSocket URLs, protocol sessions, and driver handles are never persistable state.
- Commands are bounded or coalesced by kind. Resize and pointer motion must not build an unbounded queue.
- Events do not carry large pixel buffers through mpsc. The UI receives a lightweight sequence notification and reads the newest frame slot.

## Chromium CDP backend

### Transport and session

- Preserve the existing loopback-only endpoint enforcement, TCP no-delay, bounded handshake/write/read timeouts, cancellation checks, and orphan-response cleanup.
- Preserve flattened target-session routing: browser-level commands, page-session events, and `Page.screencastFrameAck` have different session placement requirements.
- Drain every event received during a synchronous command roundtrip. Losing a screencast frame without acknowledging it can stall the stream; losing navigation/title events can persist false state.
- Keep one exact Chromium child handle per initial panel implementation. A shared browser/multi-tab architecture is a measured follow-up, not a landing shortcut.

### Frame hot path

1. Receive `Page.screencastFrame`.
2. Send `Page.screencastFrameAck` immediately, before decode.
3. Decode base64 into reusable scratch rather than allocating a fresh `Vec` per frame.
4. Decode JPEG outside the frame-slot lock into a reusable output buffer.
5. Replace the latest frame and retire only the bounded buffers still borrowed by the UI.
6. Claim at most one outstanding UI wake-up.
7. Upload a texture only when the newest sequence changed.

Retain the current candidate's fail-open behavior: a bad frame keeps the previous visible frame and does not stop the protocol pump.

### Input and navigation

- Preserve physical and logical key identity, DOM key/code, virtual-key values, repeat, modifier masks, edit commands, and key-up symmetry.
- Preserve pointer button masks during move/release, released-outside handling, wheel magnitude/direction, focus loss, and IME commit/dismissal.
- Keep clipboard behavior behind an explicit host boundary.
- Track authoritative top-frame committed URL/title events separately from requested URL text.
- Coalesce viewport updates and capture one authoritative frame after resize if the screencast transition does not paint promptly.

## Firefox WebDriver BiDi backend

### Required protocol scope

- Session creation/status and capability negotiation.
- Browsing-context discovery, creation, activation, close, navigation, history traversal, reload, viewport, and lifecycle subscriptions.
- Input actions with correct source identity and release semantics.
- Script/preload support needed for focused-element and clipboard integration, subject to security review.
- Screenshot capture and exact error mapping.
- Deterministic Firefox child/profile ownership and bounded shutdown.

### Rendering reality

The current WebDriver BiDi specification defines both `captureScreenshot` and `startScreencast`, but `startScreencast` writes a recording on the remote end and returns `{ screencast, path }`. Current Firefox source implements this with `MediaRecorder`, writes WebM data to its downloads directory, and requests data at a 10-second timeslice.

Therefore:

- Do not use BiDi recording-file screencast for interactive rendering.
- Do not tail the file: the latency, container boundaries, remote path, cleanup, and crash semantics are unsuitable.
- Implement an adaptive `captureScreenshot` prototype behind `FrameDelivery::AdaptiveScreenshot`.
- Capture immediately after Horizon input, viewport changes, navigation commits, and relevant lifecycle events.
- Use a bounded active cadence only while page activity is detected; decay quickly to zero or a very low probe rate when static.
- Count requested, completed, superseded, decoded, and published screenshots.
- Never permit more than one capture in flight per context; newest demand wins.

This can outperform ferridriver's unconditional 15 fps polling on static and interaction-driven pages, but it remains snapshot rendering and may miss autonomous canvas/video animation without an active cadence.

### Firefox visible-panel gate

Firefox may ship as a production visible panel only if the adaptive prototype passes all of these:

- median interaction-to-visible-frame latency at or below 100 ms and p95 at or below 180 ms on local deterministic pages;
- no capture/decode work for a proven static page after the idle decay period;
- correct 30 fps capped behavior for CSS animation, canvas, and video test pages, or a documented lower cap accepted by the user;
- bounded memory and exactly one in-flight capture;
- no stale frame publication after navigation, resize, context replacement, or shutdown;
- input, focus, IME scope, and clipboard behavior meet the backend capability claims;
- Firefox process/profile cleanup passes on every advertised OS.

If the gate fails, still land the reusable BiDi transport/session work, but keep Firefox panel rendering experimental or disabled. A true low-latency Firefox stream then needs a separate design: upstream client-frame protocol support, a reviewed Firefox-side companion, or another browser capture channel.

## Safari WebDriver backend

### Supported automation surface

Apple documents Safari automation through `safaridriver` and the W3C WebDriver REST API. Automation runs in isolated Safari windows that cannot access normal history, AutoFill data, settings, or preferences. Safari permits one active browser instance and one attached WebDriver session at a time.

Upstream WebKit at revision `54b9fc03` contains optional WebDriver BiDi transport and capability negotiation behind `ENABLE(WEBDRIVER_BIDI)`. A classic new-session request with `webSocketUrl: true` can return a BiDi endpoint when the installed driver supports it. The inspected upstream browsing-context agent implements navigation, context management, viewport, history, prompts, and node location, but does not yet expose BiDi screenshot or screencast commands. Classic WebDriver screenshot remains the required frame source.

The runtime contract is therefore:

1. Compile the Safari adapter on every platform but report `UnsupportedPlatform` outside macOS.
2. Discover stable Safari's `/usr/bin/safaridriver` and optionally Safari Technology Preview's driver without changing the system default.
3. Launch the exact driver on a task-owned loopback port and retain its child handle.
4. Create a classic WebDriver session requesting `browserName: "safari"` and `webSocketUrl: true`.
5. If the returned capabilities contain `webSocketUrl`, attach the shared BiDi transport for supported events and commands.
6. Fall back to classic WebDriver for any missing BiDi command and always use classic screenshot capture until a live client-frame capability exists.
7. Delete the exact session, wait for the isolated automation window to close, then terminate and reap only the task-owned `safaridriver` child.

### Safari constraints

- Expose `FrameDelivery::AdaptiveScreenshot`, `persistent_profile: false`, and `max_sessions: Some(1)`.
- Serialize Safari session acquisition across panels. A second Safari panel shows a clear busy state instead of replacing or corrupting the active session.
- Never run `safaridriver --enable` automatically. The macOS smoke operator performs Apple's explicit one-time enablement step if required.
- Do not attach to or close a user's normal Safari windows.
- Do not use the private Safari Web Inspector protocol as a hidden rendering path.
- Treat a manually broken Safari glass pane as a permanent session disconnect and surface Retry only after cleanup.
- Use the same newest-demand adaptive screenshot scheduler and generation guards as Firefox, with Safari-specific measurements.

### Safari visible-panel gate

Safari is advertised only after an exact-head macOS smoke pass proves:

- stable Safari version, `safaridriver --version`, and returned capabilities are recorded;
- session creation succeeds without accessing normal browsing state;
- optional BiDi negotiation is reported accurately rather than assumed;
- navigation, URL/title, viewport, screenshot, keyboard, pointer, wheel, focus, and script-backed clipboard behavior match advertised capabilities;
- one-session arbitration produces a safe busy state for a second panel;
- median and p95 interaction-to-frame latency satisfy the Firefox adaptive-rendering thresholds or a separately approved Safari threshold;
- normal close, retry, app exit, and broken-glass disconnect remove the exact automation session and task-owned driver;
- no normal Safari window, profile, history, or user session is changed.

## Reuse map for existing PRs

Use GitHub's exact PR heads as the reference. The shared checkout currently has an extra local commit on `stack/browser-2-driver`; do not treat local branch labels as authoritative PR state.

| Source | Current scope | Reuse | Do not carry wholesale |
| --- | ---: | --- | --- |
| Closed #298 | 68 files, +12,250/−272 | End-to-end behavior contract, macOS smoke scenarios, process/profile security, persistence and IME regressions | The monolithic diff or old-head validation claim |
| #302 `8ee11557` | 9 files, +2,022/−10 | CDP loopback transport, child ownership, frame slot, unit-test cases | Three large engine modules in `horizon-core` |
| #303 `15333c16` | 19 files, +5,786/−9 | Driver state machine, command/event drain, input semantics, profile and handoff cases | 900-line `mod.rs`, 938-line session, 979-line input, Horizon-specific manifest inside the engine |
| #304 `e022854f` | 23 files, +1,154/−95 | Panel kind, committed-state persistence, bounded board shutdown tests | Engine and product-domain changes in one PR |
| #305 `1c4c69e1` | 28 files, +3,410/−199 | Change-driven UI rendering, one-scan input path, pointer ownership, IME, detach, performance measurements | Browser UI, lifecycle, README, research, and speech in one PR |
| #306 `042feb1f` | 5 files, +309/−50 | Browser URL-submit fixes as focused tests | PTY detach/reattach hardening in the browser delivery train; split it independently if still needed |
| #307 `cbe84dd2` | 6 files, +148/−46 | Owner-chip width, handoff layout, forced-shutdown test, `#[must_use]` notification fix | Convergence commit and unrelated lock/dependency drift |

PR #298 is the relevant closed browser-panel predecessor found in GitHub history. Earlier horizon-server PRs concern streaming Horizon terminals to a web client and are not browser-engine implementations.

## Small-PR landing train

Every branch starts from the then-current `origin/main` after its prerequisite has merged. Do not open another six-deep stack. At most one dependent follow-up may be prepared locally; the pushed PR under review must be independently testable.

Before opening each PR, measure source/test file count and non-generated changed lines. If it exceeds 10 files or 500 changed source/test lines, split it or request explicit approval before proceeding.

### Track A — Establish `horizon-browser`

1. `feat/browser-crate-scaffold`
   - Add workspace member/dependency, typed errors, backend kind, capabilities, command/event shells, and compile-only contract tests.
   - No socket, process, panel, or UI code.
2. `feat/browser-websocket-transport`
   - Add the loopback-only WebSocket pump, deadlines, cancellation, request table, and mock-peer tests.
   - Protocol-neutral envelopes only.
3. `feat/browser-cdp-protocol`
   - Add CDP envelope/session routing, target attach, response/event drain, and screencast acknowledgement tests.
   - Split page commands from message routing if the line limit is crossed.

### Track B — Chromium engine

4. `feat/browser-chromium-discovery`
   - Executable candidates, protected arguments, endpoint parsing, and validation tests.
5. `feat/browser-child-lifecycle`
   - Exact child handle, profile ownership, normal close, deadline termination, reap, and lock-release tests.
6. `feat/browser-frame-pipeline`
   - Encoded frame, reusable base64/JPEG buffers, latest slot, coalesced wake-up, and metrics.
7. `feat/browser-chromium-session`
   - Attach, context/target binding, navigation, viewport, screencast, crash/disconnect, retry boundary, and bounded shutdown.
8. `feat/browser-input-model`
   - Backend-neutral keyboard/pointer/wheel/IME/clipboard intent and invariant tests.
9. `feat/browser-cdp-input`
   - CDP mapping and deterministic DOM-event integration tests.

### Track C — Horizon product integration

10. `feat/browser-panel-core`
    - `PanelKind::Browser`, core adapter, create/focus/close, and shutdown aggregation.
11. `feat/browser-panel-persistence`
    - Config/migration, committed URL/profile state, restore, and session-store tests.
    - Sync `~/.horizon/config.yaml` during implementation because config code changes.
12. `feat/browser-agent-handoff`
    - Horizon-owned manifest, ownership TTL, handoff generation, atomic permissions, and recovery tests.
13. `feat/browser-panel-render`
    - Texture/body rendering, URL/nav chrome, status/errors, accessibility, and temporary UI smoke plan.
14. `feat/browser-panel-input`
    - Keyboard, pointer, wheel, focus, IME, clipboard, detached-window routing, and live input proof.
15. `feat/browser-panel-lifecycle`
    - Retry, session switching, app exit, profile-lock release, and exact-process cleanup.

Keep speech/dictation integration and unrelated PTY detach hardening as separate follow-ups after the browser panel is stable.

### Track D — Firefox BiDi

16. `feat/browser-bidi-protocol`
    - BiDi envelopes, capability negotiation, subscriptions, contexts, and mock-peer tests.
17. `feat/browser-firefox-session`
    - Firefox discovery/launch/profile lifecycle, navigation, viewport, events, input, screenshots, and cleanup.
18. `feat/browser-firefox-frames`
    - Adaptive screenshot scheduler, generation/staleness rules, metrics, matched performance tests, and the visible-panel go/no-go decision.

### Track E — Safari WebDriver

19. `feat/browser-webdriver-http`
    - Bounded loopback HTTP transport, classic WebDriver envelopes, capabilities, session routes, W3C actions, screenshots, and mock-server tests.
20. `feat/browser-safari-session`
    - macOS driver discovery, one-session lease, classic session lifecycle, optional `webSocketUrl` attachment, navigation, input, and cleanup.
21. `feat/browser-safari-frames`
    - Shared adaptive screenshot scheduler, Safari staleness and busy-state tests, metrics, and exact-head macOS go/no-go evidence.
22. `docs/browser-backends`
    - Exact support matrix, limitations, troubleshooting, and final performance evidence.

This is a responsibility map, not a requirement to create artificial PRs. Adjacent items may combine only when the measured result remains within repository limits and still has one independently testable outcome.

## Performance validation

Instrument the engine from the first frame PR:

- protocol messages read/written;
- frames received and acknowledged;
- base64 bytes and decode duration;
- frames decoded, superseded, dropped, and published;
- UI wake-ups claimed/coalesced;
- texture uploads and bytes uploaded;
- command queue depth and coalesced commands;
- driver wake-ups, thread count, child CPU/RSS, and cleanup duration;
- input/navigation-to-visible-frame latency.

### Workload matrix

Compare exact builds using the same browser version, profile, page, viewport, scale, quality, duration, and interaction script:

- unchanged static page;
- clock/cursor blink;
- DOM/CSS animation;
- canvas animation;
- video playback;
- scroll storm;
- pointer-only motion over empty canvas, panel chrome, and content;
- rapid input and navigation;
- repeated resize and fullscreen;
- hidden workspace, off-screen panel, and detached panel;
- one, three, and five simultaneous panels.

For Chromium, preserve or improve the predecessor baseline without breaking rendering. For Firefox, compare adaptive capture against fixed 15 fps polling and report both correctness and energy/CPU trade-offs. For Safari, record classic-only versus BiDi-assisted capability mode and compare its adaptive capture on the same macOS workload. Do not publish “faster than ferridriver” unless an exact matched comparison demonstrates it.

## Required validation

Run the complete repository matrix on the exact commit in every implementation PR:

```bash
cargo fmt --all -- --check
./scripts/check-maintainability.sh
RUSTFLAGS="-D warnings" cargo test --workspace
RUSTFLAGS="-D warnings" cargo test --workspace --features speech
cargo clippy --all-targets --features speech,trace-profiling -- -D warnings
cargo clippy --workspace --lib --bins --examples --features speech -- -D warnings -D clippy::unwrap_used -D clippy::expect_used
cargo clippy --workspace --all-targets --features speech -- -D warnings -W clippy::pedantic
```

Add crate-focused contract tests to shorten iteration, but never substitute them for the full pre-push matrix.

### Protocol contract tests

- Fragmented, coalesced, malformed, late, orphaned, and error responses.
- Events arriving during a command roundtrip.
- Request-id wrap/expiry and wrong-session events.
- Disconnect during write/read/decode/shutdown.
- CDP flattened session IDs and mandatory frame acknowledgement.
- BiDi subscriptions, context replacement, navigation generations, and one-capture-in-flight.
- Classic WebDriver HTTP errors, new/delete session, returned `webSocketUrl`, W3C actions, screenshot decoding, and one-session arbitration.
- Cancellation resolves within the socket-read bound.
- Non-loopback endpoints and protected launch-argument overrides are rejected.

### UI and lifecycle smoke

Any UI-related PR creates an extensive temporary plan under `docs/testing/`. Test only a task-owned candidate process and isolated config/profile; never signal or reuse an existing Horizon process.

- Launch, initial frame, navigation, URL commit/failure, back/forward/reload.
- Keyboard, pointer, wheel, focus loss, clipboard, IME, and accessibility.
- Resize/fit/fullscreen and normal/high-DPI texture correctness.
- Detach, move, resize, close, restore, and seeded relaunch.
- Browser crash, driver disconnect, retry, session switch, and app exit.
- No surviving child, listener, driver thread, manifest, or disposable profile.
- Screenshots after launch and interaction; motion trace/video for motion-sensitive behavior.

Cross-machine validation uses PR smoke request/report comments and exact-head evidence. Linux completes Chromium and Firefox smoke first. The exact same candidate head then moves to macOS for Chromium, Firefox, and Safari smoke; Safari cannot be advertised from Linux-only evidence. Windows must pass for Chromium and Firefox before those backends are advertised there.

## Existing PR disposition

No PR mutation is part of this plan update.

1. Keep #302–#307 and closed #298 available as read-only references while the new crate contracts and tests are extracted.
2. Build replacement branches from fresh `origin/main`; do not rebase the six-deep stack into the new architecture.
3. Link each replacement PR to the source PR/files/tests it supersedes.
4. Carry focused review fixes and smoke scenarios, not convergence commits or old lockfiles.
5. Once every useful behavior and finding is mapped to a replacement or explicit rejection, request user authorization before closing the open stack.
6. Never merge or force-push the old branches as cleanup.

## Source references

- [CDP `Page.startScreencast`](https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-startScreencast)
- [WebDriver BiDi `browsingContext.startScreencast`](https://w3c.github.io/webdriver-bidi/#command-browsingContext-startScreencast)
- [Firefox `startScreencast` implementation at `600bd212`](https://searchfox.org/firefox-main/rev/600bd2128b2ba435b9698ec8b61394aa27c7c93f/remote/webdriver-bidi/modules/root/browsingContext.sys.mjs#1905)
- [Apple Safari WebDriver documentation](https://developer.apple.com/documentation/safari-developer-tools/webdriver)
- [WebKit BiDi capability negotiation at `54b9fc03`](https://github.com/WebKit/WebKit/blob/54b9fc03d8cb650626fcab77a275117cf99cc11f/Source/WebDriver/WebDriverService.cpp#L1198)
- [WebKit BiDi browsing-context surface at `54b9fc03`](https://github.com/WebKit/WebKit/blob/54b9fc03d8cb650626fcab77a275117cf99cc11f/Source/WebKit/UIProcess/Automation/BidiBrowsingContextAgent.h)
- [Closed full browser-panel PR #298](https://github.com/peters/horizon/pull/298)
- [Engine PR #302](https://github.com/peters/horizon/pull/302)
- [Driver PR #303](https://github.com/peters/horizon/pull/303)
- [Core panel PR #304](https://github.com/peters/horizon/pull/304)
- [UI PR #305](https://github.com/peters/horizon/pull/305)
- [PTY/browser fixes PR #306](https://github.com/peters/horizon/pull/306)
- [Convergence PR #307](https://github.com/peters/horizon/pull/307)

## Accepted and deferred decisions

- Accepted: own the implementation; no ferridriver dependency.
- Accepted: put protocol/process/frame/session code in `horizon-browser`.
- Accepted: support Chromium CDP, Firefox BiDi, and Safari WebDriver with optional BiDi as explicit backends.
- Accepted: use Chromium CDP streaming for the first production visible panel.
- Accepted: preserve the high-performance pieces from the existing implementation and prove claims with matched measurements.
- Deferred pending evidence: whether Firefox adaptive screenshots meet the production visible-panel gate.
- Deferred pending exact-head macOS evidence: whether the installed Safari exposes BiDi and whether adaptive screenshots meet the Safari visible-panel gate.
- Deferred: true low-latency Firefox client streaming if standard BiDi does not expose live frames.

No implementation branch, dependency change, PR mutation, merge, browser download, or live Horizon process action is authorized by this document alone.
