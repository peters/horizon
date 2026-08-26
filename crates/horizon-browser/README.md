# horizon-browser

`horizon-browser` is Horizon's UI-independent browser engine. It owns browser
processes, CDP/WebDriver/BiDi transport, backend-neutral input, current-frame
delivery, bounded command queues, capability reporting, and deterministic
shutdown. It does not depend on Horizon, egui, winit, tokio, or an async
runtime.

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
BiDi, physical keys, clipboard, downloads, or multiple concurrent sessions.

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
})?;

let _accepted = session.send(BrowserCommand::Reload);
# Ok::<(), horizon_browser::BrowserError>(())
```

The embedder owns rendering and persistence. It may upload `FrameData::rgb` to
a GPU texture, write it to an image, or expose it through another UI toolkit.

## Agent control, steering, and audit

`BrowserCoordination` is an optional host adapter. It lets an application:

- expose live ownership and handoff signals;
- deliver validated `AgentAction` values through the same control model on
  Chromium, Firefox, and Safari;
- pause agent actions while a user is actively steering;
- persist privacy-aware `BrowserAuditEntry` records.

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
