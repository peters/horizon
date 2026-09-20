# Smoke plan — terminal keys survive a stale IME composition latch

Temporary validation artifact for the change in
`crates/horizon-ui/src/terminal_widget/ime.rs`. Delete it once the validation
pass is complete.

## What changed

`prepare_terminal_keyboard_events` suppresses ArrowUp/Down/Left/Right,
Backspace and key repeats while an IME composition is live. Suppression used to
be seeded purely from a latch carried over from an earlier frame, so:

1. a frame with keys and **no** IME event still filtered them, and
2. the `Preedit("")` that ends a Wayland text-input round trip could not release
   the keys it arrived with, because egui reports it after them.

Suppression now requires evidence in the same frame: a frame without IME events
passes through untouched, and a frame whose only composition news is a bare
empty preedit starts uncomposed. A `Commit` still ends its composition where it
arrives, so keys the IME consumed before it stay filtered.

## Preconditions

- A build of the candidate head: `cargo build -p horizon-ui --bin horizon`.
- An input method that drives the platform text-input path (ibus or fcitx5 on
  Linux, the built-in IME on macOS/Windows) with one CJK engine installed, for
  the composition lanes.
- Linux UI lanes run on a task-owned isolated desktop viewed through a Horizon
  native VNC Device panel, never on the developer's session.

## Lane A — Wayland regression (the reported failure)

Run on a **native Wayland** session with ibus active
(`XDG_SESSION_TYPE=wayland`, `WAYLAND_DISPLAY` set, `ibus-daemon` running).
Confirm the process really is on Wayland before trusting the lane:
`ls -l /proc/<pid>/fd | grep -c wayland` must be non-zero and the X11 socket
count zero.

| # | Step | Expected |
|---|------|----------|
| A1 | Focus a terminal panel, run a shell, press ArrowUp/ArrowDown | Shell history moves on every press |
| A2 | Hold ArrowUp | History keeps moving while held (key repeat is not filtered) |
| A3 | Type text, press Backspace repeatedly | Every Backspace deletes one character |
| A4 | Run a TUI with a list (for example `claude` and a selection prompt), navigate with arrows | Selection moves on every press |
| A5 | Repeat A1–A4 after switching panels, detaching a panel and returning focus | Unchanged |

Before the fix, A1–A4 fail intermittently and then persistently in a panel that
has seen one composition round trip.

## Lane B — composition still owns its keys

Same session, with a CJK engine selected.

| # | Step | Expected |
|---|------|----------|
| B1 | Start a composition (type `nihao` in Pinyin), press ArrowLeft/ArrowRight while the preedit is on screen | The IME moves within the preedit; the terminal receives nothing |
| B2 | Press Backspace during a live preedit | The preedit loses a character; the terminal receives nothing |
| B3 | Commit with Enter, then press arrows | Committed text reaches the terminal, arrows resume moving history |
| B4 | Start a composition, then blur the panel and refocus it | No stuck state: arrows work immediately |

## Lane C — X11 no-regression

Run the same table as Lane A on an X11 session (`XDG_SESSION_TYPE=x11`), which
is the path that already worked. On Linux use the local fixture:

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

No platform-specific code changed, but the filter is shared, so confirm the
composition lanes on each OS with the system IME (macOS: Japanese Romaji;
Windows: Microsoft IME). Run Lane B, then Lane A's A1–A3. Report each OS as
passed, failed or not run.

## Evidence

Record for each lane: OS and session type, IME and engine, candidate commit,
and a short capture of the terminal after A2/A4 and B1. A lane without an
isolated desktop or without the required IME is reported blocked, not skipped
silently.
