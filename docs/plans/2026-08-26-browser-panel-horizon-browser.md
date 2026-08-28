# Browser panel: first-party `horizon-browser` landing plan

- Status: implemented and locally validated; Linux Chromium/Firefox exact-code smoke passed; macOS Chromium/Firefox/Safari smoke pending
- Date: 2026-08-26
- Target base: `origin/main` at `4c6a69d`
- New workspace crates: `crates/horizon-browser`, `crates/horizon-browser-mcp`
- Protocols: Chromium CDP, Firefox WebDriver BiDi, and Safari WebDriver with optional BiDi
- Full browser predecessor: closed PR #298
- Current implementation references: stacked PRs #302–#307
- Supersedes: `2026-08-26-browser-panel-ferridriver.md`

## Decision

Build and own the browser engine in a new `horizon-browser` crate. Do not integrate ferridriver.

The crate exposes one backend-neutral session API with explicit capabilities and three backend adapters:

- `ChromiumCdp`: the first production renderer, using change-driven CDP `Page.startScreencast` frames.
- `FirefoxBidi`: a real WebDriver BiDi transport/session/input backend, landed after the shared API and Chromium path are stable.
- `SafariWebDriver`: a macOS-only `safaridriver` backend using classic WebDriver HTTP for required commands and screenshots, plus BiDi when capability negotiation returns `webSocketUrl`.

CDP does not mean Firefox support. Firefox uses BiDi. The common API must preserve backend differences instead of reducing both protocols to a lowest-common-denominator interface.

Chromium is the first complete visible browser panel. Firefox BiDi support includes transport, process ownership, contexts, navigation, events, input, screenshots, and capability reporting. Firefox becomes a production visible panel only when its rendering lane passes the explicit gate below. Safari is built into the same engine contract before the macOS handoff, but it remains unavailable on non-macOS hosts and requires exact-machine capability and smoke proof.

## Implementation checkpoint

The local `feature/horizon-browser` candidate now implements the vertical
slice rather than integrating the old stack:

- first-party Chromium CDP push frames, Firefox WebDriver BiDi control with
  adaptive classic WebDriver screenshots, and macOS-only Safari WebDriver with
  optional negotiated BiDi;
- browser panels, backend selection, persistence, input, resize, retry,
  deterministic child teardown, profiles, and capability reporting;
- backend-neutral agent actions on every backend, fresh-owner leases, user
  steering priority, explicit user handoff, and private append-only redacted
  audit journals;
- one stdio MCP agent contract for discovery, audited creation of a visible
  panel in the requesting agent's workspace, navigation, semantic snapshots
  and queries, ref-based actions, waits, handoff, and audit; there is no
  browser-control CLI and raw protocol/runtime endpoints stay private;
- an explicit automation-disclosure policy: Chromium and Firefox establish a
  narrow pre-document common-signal minimization contract, while Safari and
  browser-owned behavior are reported honestly through active capabilities;
- a UI-independent `horizon-browser` package boundary with crates.io metadata,
  a crate README, and no dependency on Horizon, egui, winit, Tokio, or an async
  runtime. Packaging metadata is preparation only; the crate is not published.

Linux deterministic-page smoke has passed for Chromium and Firefox, including
pixels, input, navigation, title/URL state, resize/Fit, agent steering,
handoff, redacted audit records, exact child/manifest cleanup, and a normal
close/relaunch that restored one Chromium panel and one Firefox BiDi panel.
The relaunch pass caught and fixed a missing runtime-dirty signal after backend
selection (`472451c`). A later interaction pass caught stale viewport geometry:
Firefox clicks were gated until a manual resize. Backend switches now clear
session-owned UI caches and bounded viewport retries continue until a matching
frame arrives. The same pass added working Chromium native-scrollbar drag
translation and a visible, non-injected Firefox scrollbar indicator backed by
authoritative page metrics; Firefox still receives the actual drag through
WebDriver. Firefox met the measured input/navigation latency gates and adaptive
capture decayed to zero on a static page. Safari remains capability-implemented
but not release-supported until the exact candidate is exercised on macOS after
the Linux/final-validation gate.

The final Chromium startup rerun also proved that native Client Hint metadata
is collected through a short-lived hidden internal target. That target is
closed before Horizon attaches the caller page, is absent from the live target
list afterward, and never enters caller navigation history. On the
deterministic fixture, Chromium and Firefox both exposed the narrow configured
`navigator.webdriver == false` contract in the earliest author script, current
document, and a newly created same-origin iframe; this remains common-signal
minimization, not proof that automation or AI control is undetectable.

The integrated diff is intentionally over the repository's normal PR scope
limit. The user authorized pushing the implementation branch for cross-machine
smoke, but not opening a PR. Do not open it as one PR without explicit scope
approval; the landing-train section remains the recommended review split.

## Performance answer

Owning the implementation is a sound performance and control decision, but “we stream while ferridriver takes snapshots” is only partly correct:

- Horizon's existing Chromium candidate and ferridriver's Chromium backend both use CDP `Page.startScreencast`. Streaming is not a unique Chromium advantage.
- The current Horizon candidate has useful hot-path properties worth preserving: immediate frame acknowledgement, latest-only publication, a coalesced UI wake-up, allocation-aware JPEG decode, and texture upload only when the frame sequence changes.
- Chromium is no longer forced into software rendering by Horizon. The engine
  lets the browser negotiate its renderer by default while preserving an
  explicit `--disable-gpu` escape hatch. An isolated Xvfb smoke can still
  select SwiftShader, so renderer-process/GPU diagnostics on a real desktop are
  required before attributing a speed difference to hardware acceleration.
- ferridriver 0.5.0's Firefox path uses fixed-rate screenshot capture. A first-party implementation lets Horizon avoid that exact policy.
- Standard BiDi does not currently provide live image frames to the client. Its `browsingContext.startScreencast` command records media to a file and returns the path. Firefox's current implementation uses `MediaRecorder` and a 10-second data timeslice, so it is not suitable as Horizon's interactive frame source.
- A Firefox backend can use `browsingContext.captureScreenshot` adaptively, but that is still snapshot rendering. It must not be described as streaming.
- Safari's documented automation surface is WebDriver. Stable Safari uses isolated automation windows, allows one session at a time, and exposes screenshot capture rather than CDP-style push frames. Upstream WebKit contains partial BiDi support, but the installed Safari must prove `webSocketUrl` negotiation at runtime.

The existing Chromium measurements are promising but are not a ferridriver comparison: approximately 0.15 ms driver time at 403×252 and 2.0 ms at 1840×1000, with zero decode/upload work for an unchanged idle page. `FrameMetrics` now records comparable command-to-published-frame latency for both CDP push frames and adaptive WebDriver screenshots. Preserve the baseline and require a workload-matched, same-display benchmark before claiming that CDP, Firefox, or this crate is faster overall.

## Architecture decision record

### Context

The browser feature spans protocol transport, child-process ownership, frame delivery, input, persistence, UI, agent handoff, and shutdown. The integrated candidate proves the vertical feature but is too large to land safely as one PR: it changes more than 78 files and 15,000 lines, well above the repository's explicit review gate.

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
- Let agents steer every backend through one validated action contract while
  user activity and explicit handoffs always take priority.
- Expose that action model to agents only through MCP; keep filesystem
  coordination and backend protocols as private adapter details.
- Preserve a privacy-aware audit trail of user, agent, and system actions.
- Minimize the common explicit automation disclosures that Chromium and
  Firefox can remove coherently before page scripts, and report the active
  result instead of inferring it from the selected browser.
- Keep a future crates.io release straightforward without publishing the crate
  as part of this work.

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
- Publishing `horizon-browser`, creating a registry token, or cutting a crate
  release in this implementation task.
- A second browser-control CLI or a documented direct-manifest agent API.
- An undetectable, anonymous, or anti-bot browser. Page behavior, graphics,
  timing, network, browser defects, and future signals remain observable.
- Site-specific evasion, CAPTCHA bypass, broad fingerprint spoofing, or using
  a public website such as a search engine as the correctness oracle.

## Workspace and dependency boundary

```text
horizon-ui
    |                horizon-browser-mcp
    v                       |
horizon-core <--------------+
    |                        |
    +------------------------+
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
- Backend-neutral external control actions, steering signals, and redacted
  audit values; the embedding application owns authentication, storage, and
  retention.
- Encoded-frame metadata, reusable decode scratch, latest-frame publication, backpressure, and engine metrics.
- Driver thread/session lifecycle and typed commands/events.
- Capability reporting for frame delivery, input, viewport, clipboard, download, and protocol extensions.

### `horizon-core` owns

- `PanelKind::Browser`, panel identity, board orchestration, and shutdown aggregation.
- Horizon YAML configuration and migration into typed engine launch options.
- Runtime/session persistence and restore policy.
- Locked agent ownership/handoff/action manifests under the Horizon runtime
  directory and private append-only audit journals under the Horizon audit
  directory.
- Stable mapping between engine events and panel state.
- Retry policy visible to Horizon users.

### `horizon-browser-mcp` owns

- The sole agent-facing `browser_*` tool contract over stdio MCP.
- Safe panel summaries, automatic ownership heartbeats, bounded result waits,
  semantic wait composition, handoff requests, and redacted audit reads.
- Structured MCP schemas that never expose raw browser endpoints or Horizon
  runtime paths.

### `horizon-ui` owns

- egui texture creation/update and browser chrome.
- URL entry, navigation controls, status and error presentation.
- Translation from egui/winit keyboard, pointer, wheel, focus, clipboard, and IME events into engine input intent.
- Detached-window routing, fullscreen behavior, and accessibility labels.
- Repaint decisions based on a coalesced engine notification.

### Workspace manifest

The new crates must use workspace version, edition, Rust version, license,
lints, and repository metadata. Add each workspace member and exact
path/version workspace dependency:

```toml
[workspace]
members = [
    "crates/horizon-browser",
    "crates/horizon-browser-mcp",
    "crates/horizon-core",
    "crates/horizon-cursor",
    "crates/horizon-ui",
]

[workspace.dependencies]
horizon-browser = { path = "crates/horizon-browser", version = "0.2.7" }
horizon-browser-mcp = { path = "crates/horizon-browser-mcp", version = "0.2.7" }
```

Keep the version synchronized with `[workspace.package].version`. Do not add protocol feature flags initially: CDP and BiDi share the same small wire dependencies, and compiling both avoids an unnecessary CI matrix. Reconsider only if backend-specific dependencies become material.

The crate manifest may be publishable and contain package metadata, but this
task must not run `cargo publish` or create a registry release. Its direct
runtime dependencies stay limited to protocol serialization/transport, image
decode, typed errors, and tracing. A separate application can implement
`BrowserCoordination` with its own socket, database, or in-memory adapter
without importing Horizon.

## Implemented crate layout

```text
crates/horizon-browser/
  Cargo.toml
  README.md
  src/
    lib.rs                  public API and capability types
    control.rs              validated backend-neutral agent actions
    audit.rs                privacy-aware action records
    coordination.rs         optional host steering/audit boundary
    error.rs                typed engine/transport/protocol/process errors
    cdp.rs                  flattened CDP envelopes and request pump
    websocket.rs            loopback-only JSON WebSocket transport
    frames.rs               newest-frame decode, handoff, and metrics
    input.rs                backend-neutral input model
    input/cdp.rs            CDP input translation
    paths.rs                executable discovery candidates
    profile.rs              confined per-panel profiles
    process.rs              browser launch and process ownership
    process/control.rs      bounded exact-child teardown
    session.rs              Chromium driver orchestration and public API
    session/                focused command, event, lifecycle, startup,
                            shutdown, clipboard, queue, and host leaves
    webdriver/
      mod.rs                shared Firefox/Safari adapter entry
      http.rs               bounded classic WebDriver HTTP
      actions.rs            W3C/BiDi action translation
      service.rs            driver services and Safari arbitration
      session.rs            Firefox/Safari sessions and adaptive frames
      session/coordination.rs
```

The repository maintainability guard covers this crate as well as core/UI.
Keep production modules below the 1,000-line hard limit and split near 600
lines when one file starts accumulating another independent responsibility.

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
    pub downloads: bool,
    pub persistent_profile: bool,
    pub automation_disclosure_minimization: bool,
    pub max_sessions: Option<u32>,
}

pub enum AutomationDisclosurePolicy {
    BrowserDefault,
    MinimizeCommonSignals,
}

pub enum AutomationDisclosureStatus {
    BrowserDefault,
    CommonSignalsMinimized,
    UnsupportedByBackend,
}

pub struct BrowserSession {
    // Opaque driver handle and typed command/event boundary.
}
```

The implemented fields follow the measured backend behavior; these rules are
fixed:

- Callers can query capabilities before showing UI controls or promising behavior.
- Protocol JSON values and session identifiers do not escape the crate.
- Requested navigation and committed navigation remain distinct.
- Transient ports, PIDs, WebSocket URLs, protocol sessions, and driver handles are never persistable state.
- Commands are bounded or coalesced by kind. Resize and pointer motion must not build an unbounded queue.
- Events do not carry large pixel buffers through mpsc. The UI receives a lightweight sequence notification and reads the newest frame slot.
- A static capability says whether a backend can establish the minimization
  contract; `ActiveBackendCapabilities` reports what the current session
  actually established. No status is an undetectability claim.

### Host coordination and audit

`BrowserCoordination` is optional so standalone embedders do not inherit a
Horizon filesystem or IPC choice. When present, it supplies live ownership,
handoff, validated `AgentAction` batches, user-activity state, and an audit
sink. The engine uses the same command translation for agent and user actions
on every backend, but stamps user activity only for user-originated page
controls and input.

Horizon's adapter serializes manifest updates with an adjacent OS lock, permits
only the fresh owner to queue work, caps the queue, pauses it during user
activity or handoff, and atomically claims actions once. Audit entries record
queued, dispatched, rejected, and system actions. `dispatched` is a transport
boundary, not a claim that the page completed the action. Text content is not
stored; URL user-info, query values, and fragments are redacted while the path
is retained. The queue is at-most-once: a crash between claim and dispatch
leaves a queued-only record and requires a new caller decision rather than an
unsafe automatic retry. The local JSONL journal is private and
application-append-only, but intentionally not presented as tamper-evident or
compliance-grade storage.

### Sole agent contract: MCP

`horizon-browser-mcp` is a stdio-only adapter over Horizon's private host
coordination. Its public surface is the `browser_*` MCP tool set: list or read
a panel, create a normal visible panel in the requesting agent's workspace,
navigate, snapshot, query, act, evaluate, wait, request user handoff, and read
the redacted audit. It automatically claims and heartbeats a panel for the
stable identity injected into a Horizon agent panel. Creation is routed only
to a Horizon host containing that exact agent identity, is bounded, and writes
the same queued/dispatched/terminal audit lifecycle as later browser actions.
No MCP result or tool schema exposes raw browser endpoints, manifest paths, or
result paths.

The standalone binary and Horizon's private `--browser-mcp` bootstrap mode run
the same server. The latter lets bundled agent integrations start MCP from the
exact Horizon executable without shipping a second release artifact. This
bootstrap switch is transport plumbing, not a browser-control CLI. Codex gets
an in-memory launch override that pre-approves only the transient
`horizon-browser` server's tools; Claude gets a generated entry in Horizon's
existing plugin and already launches in its automatic permission mode. Neither
path edits the user's persistent MCP configuration or weakens approval policy
for unrelated tools.

## Chromium CDP backend

### Transport and session

- Preserve the existing loopback-only endpoint enforcement, TCP no-delay, bounded handshake/write/read timeouts, cancellation checks, and orphan-response cleanup.
- Preserve flattened target-session routing: browser-level commands, page-session events, and `Page.screencastFrameAck` have different session placement requirements.
- Drain every event received during a synchronous command roundtrip. Losing a screencast frame without acknowledging it can stall the stream; losing navigation/title events can persist false state.
- Keep one exact Chromium child handle per initial panel implementation. A shared browser/multi-tab architecture is a measured follow-up, not a landing shortcut.
- Do not force `--disable-gpu`; use Chromium's normal renderer negotiation and
  retain explicit extra arguments as an operator-controlled fallback.
- Under `MinimizeCommonSignals`, reject launch arguments that override the
  engine-managed disclosure switches, avoid the Blink automation flag, install
  the common-signal script with `Page.addScriptToEvaluateOnNewDocument` before
  navigation, and apply it to the initial document.
- If headless Chromium reports a `HeadlessChrome` user-agent token, remove only
  that token with `Emulation.setUserAgentOverride` and supply required
  browser-owned user-agent metadata so Client Hint brands, platform,
  architecture, and versions remain coherent. Read those values from a
  network-free temporary target, close it, and attach the caller's untouched
  `about:blank` target before any caller URL can execute. Fail startup rather
  than claim minimization after a rejected required command.

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
- Translate Chromium presses in the measured native scrollbar gutter because
  CDP page-input dispatch cannot operate headless browser chrome; preserve
  ordinary content clicks at the right edge and audit the original physical
  pointer actions.
- Keep clipboard behavior behind an explicit host boundary.
- Track authoritative top-frame committed URL/title events separately from requested URL text.
- Coalesce viewport updates, clear session-owned geometry on backend changes,
  and retry at a bounded cadence and count until an authoritative matching frame
  arrives after resize if the screencast transition does not paint promptly.

## Firefox WebDriver BiDi backend

### Implemented session scope

- Session creation/status and capability negotiation.
- Top-level browsing-context discovery, navigation, history traversal, reload,
  viewport, and lifecycle subscriptions. Multi-tab context creation/activation
  is outside Horizon's one-panel/one-page contract.
- Input actions with correct source identity and release semantics.
- Focused text input through WebDriver/BiDi actions. Firefox does not advertise
  clipboard or physical-key capabilities that the adapter cannot preserve.
- Adaptive classic WebDriver PNG screenshot capture and exact error mapping.
- Validated root-page scroll geometry for an embedder-owned visual scrollbar
  indicator when WebDriver screenshots omit native scrollbar pixels. Page
  content is not injected or modified, and WebDriver remains the input owner.
- Deterministic Firefox child/profile ownership and bounded shutdown.
- Under `MinimizeCommonSignals`, install the same narrow callable function with
  BiDi `script.addPreloadScript` before subscriptions and initial navigation.
  Treat rejection as a startup error instead of silently reporting success.

### Rendering reality

The current WebDriver BiDi specification defines both `captureScreenshot` and `startScreencast`, but `startScreencast` writes a recording on the remote end and returns `{ screencast, path }`. Current Firefox source implements this with `MediaRecorder`, writes WebM data to its downloads directory, and requests data at a 10-second timeslice.

Therefore:

- Do not use BiDi recording-file screencast for interactive rendering.
- Do not tail the file: the latency, container boundaries, remote path, cleanup, and crash semantics are unsuitable.
- Implement adaptive classic WebDriver PNG capture behind
  `FrameDelivery::AdaptiveScreenshot`; measured Firefox builds return it more
  quickly than their BiDi screenshot command.
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
- Report `AutomationDisclosureStatus::UnsupportedByBackend` when minimization
  is requested. Safari's public WebDriver surface used here cannot establish
  the same pre-document contract; do not imply otherwise.

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
    - Horizon-owned manifest, ownership TTL, validated action queue, user
      steering priority, handoff generation, private redacted JSONL audit,
      atomic permissions, and recovery tests.
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

Use the durable [`browser-panel-gate.md`](../testing/browser-panel-gate.md) to
select the required backend, MCP, lifecycle, and performance lanes. A
UI-related PR may add a temporary delta plan for feature-specific steps, but it
must link the durable gate instead of copying its fixture or baseline checks.
Test only a task-owned candidate process and isolated config/profile; never
signal or reuse an existing Horizon process.

- Launch, initial frame, navigation, URL commit/failure, back/forward/reload.
- Keyboard, pointer, wheel, focus loss, clipboard, IME, and accessibility.
- Resize/fit/fullscreen and normal/high-DPI texture correctness.
- Detach, move, resize, close, restore, and seeded relaunch.
- Browser crash, driver disconnect, retry, session switch, and app exit.
- No surviving child, listener, driver thread, manifest, or disposable profile.
- Screenshots after launch and interaction; motion trace/video for motion-sensitive behavior.

Cross-machine validation normally uses PR smoke request/report comments and
exact-head evidence. For this authorized pre-PR handoff, Linux completes
Chromium and Firefox smoke first and pushes `feature/horizon-browser`. The
macOS tester checks out the exact remote branch head and reports its SHA before
running Chromium, Firefox, and Safari. Safari cannot be advertised from
Linux-only evidence. Windows must pass for Chromium and Firefox before those
backends are advertised there.

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
- [CDP `Emulation.setUserAgentOverride`](https://chromedevtools.github.io/devtools-protocol/tot/Emulation/#method-setUserAgentOverride)
- [WebDriver BiDi `browsingContext.startScreencast`](https://w3c.github.io/webdriver-bidi/#command-browsingContext-startScreencast)
- [WebDriver BiDi `script.addPreloadScript`](https://w3c.github.io/webdriver-bidi/#command-script-addPreloadScript)
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
- Accepted from Linux evidence: Firefox adaptive screenshots meet the current
  visible-panel latency and idle-decay gate; cross-platform claims still need
  exact-machine evidence.
- Accepted from Linux evidence: backend switching and resize converge input
  geometry without a manual-resize workaround; Chromium and Firefox expose a
  visible, draggable vertical scrollbar through backend-specific handling.
- Accepted: expose auditable backend-neutral agent controls and explicit user
  handoff through an optional host coordination boundary.
- Accepted: MCP is the sole agent-facing control contract; do not add a CLI or
  document direct runtime-file/raw-protocol access for agents.
- Accepted: prepare `horizon-browser` for a future crates.io release with a
  minimal UI-independent dependency boundary, without publishing it now.
- Accepted: default to narrow, pre-document common-signal minimization on
  Chromium and Firefox; expose browser-default opt-out and active status; do
  not promise undetectability or add site-specific fingerprint spoofing.
- Deferred pending exact-head macOS evidence: whether the installed Safari exposes BiDi and whether adaptive screenshots meet the Safari visible-panel gate.
- Deferred: true low-latency Firefox client streaming if standard BiDi does not expose live frames.

No PR mutation, merge, crate publication, tag, GitHub Release, or support claim
beyond recorded exact-machine evidence is authorized by this document alone.
