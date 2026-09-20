# Native Rust VNC evaluation

The native approach is feasible with a small egui adapter. Horizon uses a narrowly
patched Git revision of `vnc-rs` 0.5.3; the standalone device-control crate remains
independent of the viewer. This document contains source findings and generic protocol
proof, not application captures or private test scenarios.

## Dependency selection

Crates.io metadata and downloaded package sources were checked during the
17 September 2026 evaluation; version statements below describe that snapshot.

| Candidate | Source finding |
|---|---|
| [vnc-rs 0.5.3](https://docs.rs/vnc-rs/0.5.3/vnc/) | Latest stable at evaluation time; MIT/Apache-2.0 asynchronous client with decoded rectangle events. Selected with the qualified Git revision. |
| [vnc 0.4.0](https://github.com/whitequark/rust-vnc) | Published in 2016; source also contains unsafe uninitialized buffers and older dependencies. |
| [RV](https://github.com/madeye/rv) | Evaluated session code depends on the same vnc-rs release, so it does not remove its parser concerns. |
| [IronVNC](https://github.com/hkder/ironvnc) | Full egui application, with additional session and file-transfer features; not a minimal embeddable dependency. |
| [rfbclient 0.1.0](https://docs.rs/crate/rfbclient/0.1.0/source/src/lib.rs), [rfbproto 0.1.0](https://docs.rs/crate/rfbproto/0.1.0/source/src/lib.rs) | Placeholder generated functions; no client implementation. |
| [gvnc 0.7.0](https://docs.rs/crate/gvnc/0.7.0/source/src/auto/base_framebuffer.rs) | Real native bindings, but safe framebuffer construction is unimplemented in the generated API; custom FFI and native packaging would expand scope. |
| [desktui 0.4.0](https://docs.rs/crate/desktui/0.4.0/source/Cargo.toml) | Protocol implementation is part of a binary-only crate. |
| [robost-backend 0.1.2](https://docs.rs/crate/robost-backend/0.1.2/source/src/lib.rs) | Only the local backend is implemented. |
| [rfb 0.1.0](https://docs.rs/crate/rfb/0.1.0/source/src/lib.rs), [rfb2 0.1.2](https://docs.rs/crate/rfb2/0.1.2/source/src/lib.rs) | Server/protocol libraries without a complete client state machine. |

The dependency is pinned to commit `22ad274efc9e0811f43336410871330be1e3db30`
in [peters/vnc-rs](https://github.com/peters/vnc-rs). Original licenses, provenance
and qualification limits remain in the fork
([qualification notes](https://github.com/peters/vnc-rs/blob/22ad274efc9e0811f43336410871330be1e3db30/QUALIFICATION.md)).
Horizon contains no copied decoder source. Replace the Git pin with an upstream
release once the submitted fixes land and that release passes these regressions.

## Qualified decoder subset

Upstream source contained an unchecked authentication-result transmute,
uninitialized byte vectors, unchecked allocation lengths, malformed tile panic
paths and refresh dimensions that were not updated after DesktopSize events.
Loopback-only access and read-only display do not correct parser safety issues.

The patched compiled subset forbids unsafe code. It enables Raw, CopyRect, ZRLE,
DesktopSize and LastRect, rejects unsupported/unnegotiated encodings, bounds
lengths and geometry, checks palette indexes/run lengths, and initializes buffers.
Network and decoded-event queues are bounded; shutdown interrupts blocked I/O.
Wire boolean flags normalize nonzero values, including x11vnc's true-color value
of 255, while component-mask and shift validation remains enforced.

Generic regression evidence:

- Password-result parsing accepts success/failure values and rejects malformed
  values. RFB 3.8 no-authentication failures are checked instead of ignored.
- A full password-success handshake checks an independent DES reference.
  RFB 3.3 password failure completes while the server remains connected instead
  of waiting for a nonexistent error string.
- Oversized frame/name/clipboard/compressed lengths fail before reading their
  payloads. Invalid rectangle bounds and CopyRect source bounds fail explicitly.
- Valid persistent ZRLE modes preserve expected pixels; malformed palette indexes
  and overlong runs return errors.
- A synthetic DesktopSize change from 64×64 to 128×96 produces a subsequent
  refresh request for 128×96 and accepts the new bottom-right pixel.
- Viewer worker tests cancel both a stalled greeting and an incomplete Raw frame,
  proving prompt shutdown and socket closure. The dependency backpressure test
  sends queued events before cancellation; it does not separately assert that
  the queue reached capacity at the cancellation instant.

The latest dependency-specific pass completed 14 unit and two documentation tests
and all-target Clippy with warnings denied. This is bounded source qualification,
not a completed independent security audit or exhaustive protocol certification.

## Viewer and remaining acceptance

The product path is native Rust TCP → patched decoder → egui texture. Decoder work
runs off the UI thread, only the latest application image is retained, and event
draining has a time budget. Repaint follows the current viewport. Rendering is
read-only; reconnect is manual and closing a panel closes only its connection.

Generic Horizon testing exercises live terminal output, panel movement,
resize/Fit, detach/fullscreen, read-only input isolation, connection recovery and
normal shutdown inside an isolated desktop (historically observed through noVNC).
Final-candidate testing must repeat these checks after dependency changes using
the native VNC Device panel required by `AGENTS.md`.

DesktopSize protocol coverage is an in-memory server test, not proof of live
server resolution reconfiguration. Native macOS/Windows builds, Apple ARD,
TLS/VeNCrypt, remote transport, mobile devices and clipboard integration remain
unqualified. The product MVP has no credential mode even though the dependency's
password parsing is regression-tested. The viewer must retain its connection and
operation timeouts and use polling rather than concurrently awaiting the
upstream mutex-holding `recv_event()` method.

Use the [generic smoke instructions](../../scripts/device-smoke/README.md) for
reproduction and video verification. Public evidence must contain only the
synthetic Horizon fixture and must be inspected before publication.

## Viewer rendering controls

View controls are session-local. The default maximum refresh is 20 fps (range
1–30); rendered images fit within 2048×2048 pixels by default, with independently
adjustable width/height limits from 1 to 8192. Rendering retains aspect ratio and
never upscales its source. The always-visible zoom selector chooses Fit (the
image fills the available panel area) or a scale from 25% to 400%, where 100%
is one rendered image pixel per UI point and anything larger scrolls; pinch, or
wheel with the zoom modifier, over the image zooms around the pointer. Desktop
and image dimensions are shown separately. Controls never change the target
desktop resolution.

An explicit viewport selects a nonempty rectangle inside the source desktop.
Apply refuses invalid bounds; Whole desktop clears the crop. Viewport, image
limits and zoom apply immediately to the last received full desktop image,
including after the VNC worker disconnects. If the target later shrinks outside
an active viewport, the crop is cleared and the whole desktop is shown. Settings
reset when the panel is recreated; restored panels still require manual
connection. Refresh throttling limits refresh requests and frame production, not
arbitrary unsolicited server traffic; changing only Maximum fps does not
recrop or rescale the last desktop. Image limits and cropping affect local
rendering, not negotiated VNC compression or wire bandwidth. Reconnect starts a
new worker and keeps the last presented image until a new desktop arrives.

Record native flows directly from the isolated desktop as
described in the [smoke guide](../../scripts/device-smoke/README.md#video-evidence-and-cleanup);
`browser_video` records browser pages only.
