# horizon-browser

`horizon-browser` is Horizon's UI-independent browser engine. It owns browser
processes, CDP/WebDriver/BiDi transport, backend-neutral input, current-frame
delivery, bounded command queues, capability reporting, and deterministic
shutdown. It does not depend on Horizon, egui, winit, tokio, or an async
runtime.

Serialized backend, input, action, result, network, and audit values live in
the much smaller `horizon-browser-protocol` crate and are re-exported here for
source compatibility. Contract-only clients can use that crate without
compiling browser transport, process ownership, or frame decoders; applications
that launch a browser in-process use this engine crate.

The crate is prepared for a future crates.io release, but publishing remains a
separate release decision. No crate is published by building or packaging the
workspace.

## Backend contract

| Backend | Control transport | Pixel delivery |
| --- | --- | --- |
| Chromium | first-party flattened CDP | change-driven JPEG screencast |
| Firefox | WebDriver BiDi for navigation/events/input | adaptive classic WebDriver PNG screenshots |
| Safari (macOS) | classic WebDriver, optional negotiated BiDi | adaptive classic WebDriver PNG screenshots |

Capability reporting is exact for the active session. Consumers should check
`ActiveBackendCapabilities` instead of assuming that a browser name implies
BiDi, physical keys, clipboard, downloads, disclosure minimization, or
multiple concurrent sessions.

## Automation disclosure policy

`BrowserConfig::default()` uses
`AutomationDisclosurePolicy::MinimizeCommonSignals`. Chromium avoids
automation-only launch switches, suppresses Blink's standard automation flag,
and removes only the `HeadlessChrome` user-agent token and Client Hint brand
while preserving Chromium-owned platform, architecture, versions, and GREASE
brands. It does not install a script-defined `navigator.webdriver` getter,
because that accessor is itself a detection signal. The engine reads native
Client Hint values on a network-free temporary target and closes that target
before attaching the caller's `about:blank` page, so it cannot enter caller
history or frames. Title updates use protocol target metadata rather than a
page-JS binding. Firefox installs a narrow `navigator.webdriver` value shim
with WebDriver BiDi `script.addPreloadScript` before the initial navigation;
startup fails instead of silently downgrading when that required BiDi command
is rejected. Chromium panels minimizing common signals also use a reserved
nonzero loopback DevTools port so Chromium does not enable its port-zero
`AutomationControlled` behavior. Startup retries with a fresh reservation when
another local process wins the required socket handoff. Callers that need the
browser's unmodified behavior can select
`AutomationDisclosurePolicy::BrowserDefault` on any backend; Chromium then
retains its native port-zero automation signal.

Safari's public automation surface cannot currently establish the same
pre-document contract, so an active Safari session reports
`AutomationDisclosureStatus::UnsupportedByBackend`.

This policy minimizes a small set of common script-visible disclosures; it is
not an undetectability or anti-bot guarantee. Pages can still infer automation
from timing, behavior, graphics, browser bugs, environment, network, or future
signals. The crate intentionally does not spoof broad fingerprint surfaces or
claim that a particular site cannot identify automation.

## Embedding boundary

Create a `BrowserSessionConfig`, call `start_session`, drain `BrowserEvent`s,
and read the latest decoded RGB image from the shared `FrameSlot`. Commands use
the backend-neutral `BrowserCommand` and `BrowserInput` types. A synchronous
driver thread owns each browser session, so embedders do not need to adopt a
specific executor.

```rust,no_run
use std::sync::Arc;

use horizon_browser::{
    BrowserCommand, BrowserConfig, BrowserSessionConfig, FrameSlot,
    start_session,
};

let frames = Arc::new(FrameSlot::new());
let session = start_session(BrowserSessionConfig {
    browser: BrowserConfig::default(),
    panel_local_id: "example-panel".to_string(),
    initial_url: Some("https://example.com".to_string()),
    width: 1280,
    height: 800,
    frame_slot: Arc::clone(&frames),
    coordination: None,
    capture_directory: None,
})?;

let _accepted = session.send(BrowserCommand::Reload);
# Ok::<(), horizon_browser::BrowserError>(())
```

The embedder owns rendering and persistence. It may upload `FrameData::rgb` to
a GPU texture, write it to an image, or expose it through another UI toolkit.
`FrameMetrics` includes comparable interaction-to-published-frame samples for
both CDP push frames and adaptive WebDriver screenshots.

## Agent control, steering, and audit

`BrowserCoordination` is an optional host adapter. It lets an application:

- expose live ownership and handoff signals;
- deliver validated `AgentAction` values through the same control model on
  Chromium, Firefox, and Safari;
- pause agent actions while a user is actively steering;
- persist privacy-aware `BrowserAuditEntry` records.

The same contract also supports semantic request/results without exposing raw
CDP, BiDi, or WebDriver session identifiers. `Snapshot` and `Query` return
bounded visible DOM summaries with short-lived `g…s…e…` references. `Click`,
`Fill`, and `Scroll` accept either one of those references or an explicit CSS
selector; `Evaluate` returns a size-bounded JSON value. A new snapshot replaces
the previous reference set, and navigation invalidates it, so callers must
ground an action in fresh page state instead of reusing stale handles.

`BrowserControlAction::Network` lets a host start, inspect, and stop a bounded
network export without coupling the engine to the host's filesystem layout.
The embedder opts in with `BrowserSessionConfig::capture_directory`; successful
start/status/stop results contain an explicit NDJSON path, aggregate writer
health, and known WebSocket lifecycle/counters. HTTP response bodies are an
explicit opt-in and become size-bounded `HttpResponseBody` records. Chromium
uses native CDP for HTTP and WebSocket observation. Firefox uses native
WebDriver BiDi for HTTP metadata and response bodies, and disclosed
pre-document page instrumentation only for WebSocket frames because standard
BiDi does not currently provide them. Safari returns a typed
unsupported-backend failure. Payload and file limits, URL filters, bounded
queues, and a dedicated buffered writer keep busy streams off the render path.
Body data is intentionally absent from audit records and should be treated as
sensitive by embedders even though the export itself is private.

Persistent embedders should implement
`BrowserCoordination::prepare_network_capture` to apply their own age, count,
and aggregate-byte policy before each file is created. Horizon keeps exports
inside the persistent panel profile, reserves the requested file budget, and
removes the exports with that profile. The reusable crate deliberately does
not impose those product storage choices on another application.

Hosts publish `AgentActionResult` values through `BrowserCoordination`. A
terminal `Completed` audit record means the engine finished handling the
request. A `Navigate` action carries a `NavigationWait` (`dispatched`,
`commit`, or `dom_content_loaded`, default `commit`) and a bounded
`timeout_millis`; the engine dispatches at once and settles a typed
`NavigationOutcome` (requested and committed URL, title when known, loading,
redirected, elapsed, and a `state` of `dispatched`, `committed`,
`dom_content_loaded`, `timed_out`, or `superseded`) from the backend's own
commit, `DOMContentLoaded`, load, and failure signals. On Firefox BiDi a commit
is observed at `DOMContentLoaded`; classic WebDriver blocks until the document
loaded. A destination that fails after dispatch is a failed action, not a
state. Other asynchronous page behavior (reload, history traversal, input)
still needs an explicit later snapshot, query, or wait. Selectors, scripts,
filled text, and returned page data are excluded from audit records.
Only redacted shape, bounded character counts, actor, action identity, and
status are retained.

Audit records include ordered action identity, actor, dispatch state, and
pointer/navigation/input shape. Printable text is represented only by length,
and URL user-info, query values, fragments, `data:`, and script payloads are
redacted. `Dispatched` means the adapter received the command; callers should
use later URL/title/frame/error state when they need page-level completion
proof. The crate deliberately does not choose an IPC protocol, audit store,
retention policy, authentication mechanism, or user interface.

Horizon implements this boundary with private locked manifests and append-only
JSONL journals in `horizon-core`. A separate application can instead use a
local socket, database, or in-memory coordinator without importing Horizon.
Horizon's queue claims an action at most once: a crash after claim but before
dispatch is visible as a `queued` record without a matching `dispatched`
record, and the caller must decide whether it is safe to submit a new action.
The JSONL journal is application-append-only and private, not tamper-evident;
an application needing compliance-grade audit should implement the same typed
sink with authenticated durable storage.

## Dependency policy

The runtime dependency set is intentionally small:

- `tungstenite` for loopback WebSocket transport, without TLS;
- `serde`/`serde_json` for protocol envelopes;
- `zune-jpeg`, `png`, and `base64` for browser frame decoding;
- `thiserror` and `tracing` for typed errors and host-selected diagnostics.

Backend code shares these dependencies, so feature-gating individual browsers
would currently add a build matrix without materially reducing the graph. The
`test-support` feature exposes only cross-crate shutdown test construction and
is not needed by normal consumers.

## Future publication (not performed)

The package inherits Horizon's synchronized version, Rust version, MIT license,
repository, and lints while keeping its own description, keywords, categories,
README, and crates.io allow-list. A future release should first validate the
standalone archive with `cargo package -p horizon-browser --locked`, inspect its
contents, and only then run an explicitly authorized
`cargo publish -p horizon-browser --locked`. Neither command publishes during a
normal Horizon build, and this rollout does not execute the publish step.

## Validation

Horizon integration changes use the durable
[`browser-panel-gate.md`](https://github.com/peters/horizon/blob/main/docs/testing/browser-panel-gate.md),
committed loopback fixtures, and MCP-only smoke harness. The harness stays
outside this crate so a future package does not ship Horizon UI, MCP, or
test-site assets.
