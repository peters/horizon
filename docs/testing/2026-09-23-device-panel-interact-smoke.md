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
- `tests::interact::a_drag_that_leaves_the_window_or_a_hidden_viewer_releases_what_is_held`
  — `PointerGone` ends a drag; a frame where the viewer is not drawn releases
  held keys.
- `tests::interact::a_click_on_ui_covering_the_image_does_not_reach_the_desktop`
  — a foreground area over the image takes the click (fails without the
  routing gate).
- `tests::interact::a_discarded_pass_does_not_send_its_input_twice` — one
  wheel line is one notch and text is sent once even when egui re-runs a pass.
- `tests::interact::clipboard_chords_and_ime_commits_reach_the_desktop_without_the_local_clipboard`
  and `input::tests::clipboard_chords_hold_the_command_modifier_and_swallow_their_release`
  — egui-winit's Copy, Cut and Paste become the command chord for c, x, v;
  the matching key release is swallowed; a V release with no V press is an
  empty-clipboard paste; IME commits are sent as Unicode keysyms and the
  input method is enabled while captured.
- `session::tests::queued_input_reaches_the_server_before_the_next_refresh`
  — against the fake RFB server, a queued key and pointer event arrive as RFB
  KeyEvent and PointerEvent before the next FramebufferUpdateRequest (fails
  when the worker's forwarding is replaced by a sleep).

Status: **PASS** (2026-09-23, Linux x64; 59 `device_widget` tests).

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

Review-round rerun (head with event-routing gates, raw wheel events and
fractional wheel accumulation): B1 logged nothing; B2 logged `MotionNotify`,
`ButtonPress`/`ButtonRelease` button 1 at `root:(514,336)` and wheel
button 5 press/release pairs at the same point; B3 logged `h`, `i` in order.
Synthetic XTest wheel clicks reach winit inconsistently on Xvfb (one run
delivered several notches for two clicks, three later single clicks delivered
none to egui at all while the viewer was connected and interactive), so this
lane shows direction only; the notch count per wheel line is covered by
`scroll_travel_becomes_clicks_of_the_wheel_buttons` and
`a_discarded_pass_does_not_send_its_input_twice`, which drive real egui wheel
events.

Fourth review-round rerun (press ownership by the top layer at the press
position, scroll remainder cleared on release): read-only logged nothing; a
click, a drag released outside the Horizon viewer, and a second click each
logged a `ButtonPress`/`ButtonRelease` pair at `root:(514,336)` with no button
left held; typed `ok` arrived. Lane A adds
`a_fast_drag_that_ends_outside_the_image_keeps_its_press_and_release`.

### B5. Clipboard chords and typing of the same letters (review round six)

With the image captured, press Ctrl+C, Ctrl+V (empty local clipboard) and
Ctrl+X, then type `vcx`. The fixture logs `+Control_L +c -c -Control_L`,
`+Control_L -Control_L +Control_L +v -v -Control_L` (winit on X11 reports
Ctrl's release before V's, and egui-winit reports nothing else for an
empty-clipboard Ctrl+V, so the chord is rebuilt from that release),
`+Control_L +x -x -Control_L`, then `+v -v +c -c +x -x`. The local clipboard
text carried by egui's Paste event is never typed into the desktop.

Status: **PASS** (2026-09-23, Linux x64). The first attempt of this lane found
two defects that the unit test had missed (Control_L released before `c`;
Ctrl+V lost entirely); both are fixed and the widget test now replays the
event order winit produced.

## Not covered

- macOS and Windows use the same code; only Linux was run live.
- Horizon's own global shortcuts still fire while captured (they are not
  swallowed by the panel), the same as for terminal panels.
