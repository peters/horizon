# Device panel Interact — smoke test plan

Validates the Device panel's **Interact** toggle: off by default the panel is a
read-only VNC view; on, and once the image has keyboard focus, a person's
pointer and keyboard are sent to the desktop over the same VNC session.
Agent tools never turn it on: `device_panel` has no such operation and the
toggle is not persisted.

Public evidence must use only the synthetic fixtures from
`2026-09-23-device-ssh-tunnel-smoke.md`, with `x11vnc` started without
`-viewonly` for this plan only.

## Lane A — unit (no live viewer)

```sh
cargo test -p horizon-ui --bin horizon device_widget
```

Must include:

- `input::tests::*` — pointer mapping through crop and scale to desktop
  pixels, button masks, wheel notches, modifier diffing, printable keys left
  to text unless Ctrl or Command is held, a held key's release after its
  modifier went up, `release_all`, keysym naming (F-keys, digits, punctuation,
  Latin-1 and Unicode text).
- `tests::interact::read_only_by_default_sends_nothing_and_interact_forwards_pointer_and_keys`
  — nothing is queued while read-only; with Interact on, a move maps the image
  centre to the desktop centre, a click focuses and presses at the top-left
  pixel, typed `a` is sent once, Enter as its keysym, and turning Interact off
  releases what is still held.
- `tests::interact::modifiers_and_wheel_reach_the_desktop_and_escape_drops_capture`
  — wheel travel becomes button 5 clicks, Ctrl+C sends Control_L before `c`,
  Escape is delivered to the desktop and keeps capture, a click outside the
  image releases the keyboard and the held Enter.
- `tests::interact::pointer_positions_are_mapped_through_the_panel_layer_transform`
  — with the panel's layer scaled and shifted as a zoomed canvas does, global
  pointer positions still press and release the desktop centre (this test
  fails without the mapping).
- `session::tests::*` unchanged; the worker forwards queued input ahead of the
  next refresh (`forward_input_until`).

Status: **PASS** (2026-09-23, Linux x64; 52 `device_widget` tests).

## Lane B — live viewer on an isolated desktop (Linux)

Setup: fixture Xvfb `:97` with `x11vnc -localhost -rfbport 5997` (no
`-viewonly`), `xev -root -event keyboard -event button -event mouse` on `:97`
logging what the desktop receives, Horizon on Xvfb `:98` + openbox with a
private `HOME` and a config holding one `kind: device` panel for
`127.0.0.1:5997`, launched with `--config <yaml> --ephemeral`. The panel is
restored, so press **Reconnect** first.

### B1. Read-only by default

With the panel connected, click and type into the image. The header still
says `Read-only`, `xev` on `:97` logs nothing, and the fixture pointer stays
where it was.

### B2. Interact on: pointer

Click **Interact**. The header reads `Interactive: your mouse and keyboard go
to the desktop`. Move over the image and click: `xdotool getmouselocation
--display :97` follows the pointer and `xev` logs `ButtonPress`/`ButtonRelease`
at the mapped desktop coordinates.

### B3. Interact on: keyboard

With the image focused (the hint reads `Keyboard captured…`), type `hi`, press
Enter and Escape. `xev` logs KeyPress/KeyRelease for `h`, `i`, `Return` and
`Escape`, in order.

### B4. Release

Click outside the image (the header area). The hint returns to `Click the
image to type into it`; typing no longer reaches `xev`. Turn Interact off: the
header reads `Read-only` again.

Status: **PASS** (2026-09-23, Linux x64, Xvfb `:97` fixture + `:98` viewer,
debug build of this branch, evidence from `xev -root` on `:97` and `xdotool
getmouselocation --display :97`). B1: clicks and typed text with Interact off
left the fixture log empty and the pointer at its start. B2: with Interact on,
a fast click (press and release inside one repaint) at the image point that
maps to desktop (514, 336) logged `MotionNotify`, `ButtonPress` and
`ButtonRelease` at `root:(514,336)`; a later move mapped to (139, 148) and the
fixture pointer followed. B3: `h`, `i`, `Return`, `Escape` arrived as press and
release pairs in order. B4: after clicking the header, typing logged nothing,
and Interact off restored `Read-only`. Two defects were found and fixed by
this lane before the pass: pointer input sampled once per frame lost a click
whose press and release shared a repaint (now replayed per egui event), and
global pointer positions were compared against the panel's canvas-local image
rectangle (now mapped through the layer transform).

## Not covered

- macOS and Windows use the same code; only Linux was run live.
- Horizon's own global shortcuts still fire while captured (they are not
  swallowed by the panel), the same as for terminal panels.
