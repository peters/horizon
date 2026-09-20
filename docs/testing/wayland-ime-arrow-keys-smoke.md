# Smoke plan — terminal keys and the IME composition filter

Temporary validation artifact for the change in
`crates/horizon-ui/src/terminal_widget/ime.rs`. Delete it once the validation
pass is complete.

## What changed

Nothing in how keys are routed. `prepare_terminal_keyboard_events` still
withholds ArrowUp/Down/Left/Right, Backspace and key repeats from the terminal
while an IME composition is live, and still tracks that composition per event.

The change adds the missing observability for that decision — the one place a
terminal panel drops a key it was handed — plus characterization tests that
pin the behavior on which a fix has to build:

- `tracing::debug!` when a key is withheld, naming the key and whether the
  composition was latched from an earlier frame;
- `tracing::trace!` for each preedit and commit, recording composition state
  and the committed length only, never the text itself.

## Preconditions

- A build of the candidate head: `cargo build -p horizon-ui --bin horizon`.
- An input method that drives the platform text-input path (ibus or fcitx5 on
  Linux, the built-in IME on macOS/Windows) with one CJK engine installed, for
  the composition lanes.
- Linux UI lanes run on a task-owned isolated desktop viewed through a Horizon
  native VNC Device panel, never on the developer's session.

## Lane A — diagnose a Wayland session that loses navigation

Run on a **native Wayland** session with an input method active
(`XDG_SESSION_TYPE=wayland`, `WAYLAND_DISPLAY` set, `ibus-daemon` or `fcitx5`
running). Environment variables state intent, not the backend winit picked, and
a socket file descriptor resolves to `socket:[inode]` rather than to a path, so
neither is evidence on its own. Check the loaded libraries and open files:

```sh
lsof -p <pid> | grep -iE 'wayland|X11-unix'
```

A Wayland-backed process maps `libwayland-client` and holds no `X11-unix`
socket; an X11 or XWayland-backed one holds that socket.

Start the candidate with the filter's diagnostics on:

```sh
RUST_LOG=horizon=debug target/debug/horizon 2>&1 | tee /tmp/horizon-ime.log
```

| # | Step | Expected | Reading |
|---|------|----------|---------|
| A1 | Focus a terminal panel, run a shell, press ArrowUp/ArrowDown | History moves on every press | No `terminal key held by a live IME composition` line |
| A2 | Hold ArrowUp | History keeps moving while held | As above |
| A3 | Type text, press Backspace repeatedly | Every Backspace deletes one character | As above |
| A4 | Navigate a TUI list (for example `claude` and a selection prompt) | Selection moves on every press | As above |

If a step fails **and** the log shows the withheld line, the filter is holding
keys outside a visible composition; `latched` tells whether the composition came
from an earlier frame, and the surrounding preedit/commit traces show what the
IME reported. If a step fails with **no** such line, the keys never reached this
filter and the cause is upstream — record the log and say so.

## Lane B — composition still owns its keys

Same session, with a CJK engine selected.

| # | Step | Expected |
|---|------|----------|
| B1 | Start a composition (type `nihao` in Pinyin), press ArrowLeft/ArrowRight while the preedit is on screen | The IME moves within the preedit; the terminal receives nothing |
| B2 | Press Backspace during a live preedit, including on the last remaining character | The preedit loses a character and dismisses; the terminal receives nothing |
| B3 | Commit with Enter, then press arrows | Committed text reaches the terminal, arrows resume moving history |
| B4 | Start a composition, then blur the panel and refocus it | No stuck state: arrows work immediately |

## Lane C — X11 no-regression

Run Lane A's table on an X11 session (`XDG_SESSION_TYPE=x11`). On Linux use the
local fixture:

```sh
python3 scripts/device-smoke/serve.py --horizon "$smoke_bin/horizon" \
  --native-view --state /tmp/horizon-ime-smoke
```

Attach a native VNC Device panel to the fixture's `vnc_address`, then drive the
in-fixture terminal through the device CLI/MCP act path. Check `connection`,
`image_received`, `image_displayed` and an advancing `frame_sequence` before
reading any result, and re-observe after every action — dispatch is delivery,
not application.

| # | Step | Expected |
|---|------|----------|
| C1 | Type `echo one`, Enter, `echo two`, Enter | Both lines run |
| C2 | Press ArrowUp twice, Enter | The earlier command runs again |
| C3 | Type text, press Backspace | Characters disappear one per press |

## Lane D — macOS and Windows

No platform-specific code changed, but the filter is shared, so confirm Lane B
on each OS with the system IME (macOS: Japanese Romaji; Windows: Microsoft
IME), then Lane A's A1–A3. Report each OS as passed, failed or not run.

## Evidence

Record for each lane: OS and session type, IME and engine, candidate commit,
the captured log, and a short capture of the terminal after A2/A4 and B1. A lane
without an isolated desktop or without the required IME is reported blocked, not
skipped silently.
