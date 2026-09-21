# macOS panel zoom smoke test

Temporary validation artifact for the browser/device panel zoom pull request.
macOS is the platform this change cannot be fully qualified on elsewhere: the
zoom modifier is Command rather than Control, and trackpad pinch arrives as a
native gesture instead of the X11 XInput bridge used on Linux. Run every lane
against the exact PR head, on a task-owned isolated desktop.

## Safety contract

- Run the interactive lanes on a **task-owned isolated macOS desktop** exposed
  through a task-owned loopback VNC server and watched live in a Horizon
  native Device panel, as `scripts/device-smoke/README.md` requires. Never
  drive, screenshot, or record the developer's active desktop. If no isolated
  desktop with a VNC endpoint and a supporting native panel is available,
  report lanes B, C and D as **blocked** rather than running them anywhere
  else; the unit lane still applies.
- Do not stop, signal, reuse, or automate a pre-existing Horizon process.
  Record existing Horizon PIDs first and exclude them from every command.
- Launch only a task-owned build with an isolated home, config, and session
  (`--config <file> --ephemeral`).
- Point device panels only at a task-owned loopback VNC server. Never at a
  developer desktop or a third-party machine.
- Browser lanes use `about:blank` and a local file page only. Do not load
  third-party sites, sign in anywhere, or capture unrelated windows.
- Close the task-owned window normally and verify only its PID exits.

## 1. Exact-head preflight

```bash
git fetch origin
pr_sha='REPLACE_WITH_EXACT_PR_SHA'
git worktree add /tmp/horizon-macos-zoom-smoke "$pr_sha"
cd /tmp/horizon-macos-zoom-smoke
git rev-parse HEAD && git status --short
sw_vers && uname -m && rustc --version
cargo build --bin horizon
smoke_bin=$(mktemp -d /tmp/horizon-macos-zoom-bin.XXXXXX)
cp target/debug/horizon "$smoke_bin/horizon"
shasum -a 256 "$smoke_bin/horizon" | tee "$smoke_bin/SHA256SUMS"
```

Confirm `HEAD` equals the PR SHA and the worktree is clean. After launching
the frozen copy, find the actual Horizon child PID in the fixture's own
process tree and prove it is the candidate: resolve its executable and hash
it, rather than trusting the process name or the build output.

```bash
horizon_pid=12345  # replace with the fixture's actual Horizon child PID
python3 - "$horizon_pid" "$smoke_bin" <<'PY'
import ctypes
import hashlib
import os
import sys
from pathlib import Path

pid = int(sys.argv[1])
frozen = Path(sys.argv[2]) / "horizon"
expected = (frozen.parent / "SHA256SUMS").read_text().split()[0]
libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
libproc.proc_pidpath.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32]
libproc.proc_pidpath.restype = ctypes.c_int
buffer = ctypes.create_string_buffer(4096)  # PROC_PIDPATHINFO_MAXSIZE
if libproc.proc_pidpath(pid, buffer, len(buffer)) <= 0:
    error = ctypes.get_errno()
    raise OSError(error, os.strerror(error))
exe = Path(os.fsdecode(buffer.value))
if not exe.is_absolute() or not exe.samefile(frozen):
    raise SystemExit(f"PID {pid} is not running the frozen candidate: {exe}")
digest = hashlib.sha256(exe.read_bytes()).hexdigest()
if digest != expected:
    raise SystemExit(f"Executable hash mismatch: {digest} != {expected}")
print(f"PID={pid} executable={exe} SHA256={digest}")
PY
```

This requires Python 3 and uses macOS
[`proc_pidpath`](https://github.com/apple-oss-distributions/xnu/blob/main/libsyscall/wrappers/libproc/libproc.h).
Stop on any lookup, path or digest failure. Record the PID and hash with the
evidence; a mismatch invalidates every lane below.

## 2. Lane A — unit tiers

```bash
cargo test -p horizon-ui --bin horizon -- panel_zoom
cargo test -p horizon-ui --bin horizon -- device_widget
cargo test -p horizon-ui --bin horizon -- browser_widget
cargo test --workspace
```

All must pass on the Mac toolchain; no lane below substitutes for them. Each
filter runs as its own command so the lane does not depend on a libtest that
accepts several positional filters.

## 3. Lane B — device panel (trackpad)

Start a task-owned VNC server on loopback and open a Device panel pointing at
it, then:

1. **Dropdown.** The panel header shows a zoom selector reading `Fit`. Choose
   `100%`: one presented image pixel per point, scrollbars when the desktop is
   larger than the body. Choose `Fit` again: the image letterboxes back.
2. **Pinch.** Two-finger pinch out on the trackpad over the image. The
   percentage rises, the pixel under the cursor stays put, and the canvas does
   **not** zoom (panel frame, sidebar, and toolbar keep their size). Pinch in
   below `Fit` and confirm the percentage falls.
3. **Command+wheel.** With a mouse (or two-finger scroll), hold Command and
   scroll over the image: same result as pinch. Release Command and scroll:
   the zoomed image pans instead of zooming.
4. **Isolation.** Record the task-owned VNC server's desktop geometry before
   and after every step above; it must remain unchanged. The current
   `horizon-device` input backend supports local Linux X11 only, so its
   `doctor` command cannot qualify this macOS desktop.
5. **Retained frame.** Stop the VNC server. With the panel Disconnected, the
   dropdown and gestures must still rescale the last desktop image.

## 4. Lane C — browser panel

Open a Browser panel on a local `file://` page that prints
`window.innerWidth × window.innerHeight` and echoes `event.clientX/Y` from a
button click.

1. At `100%`, note the reported viewport.
2. Choose `150%`: the page reflows, text and boxes grow, and the reported
   viewport is the previous size divided by 1.5 (±1 px rounding).
3. Click the button at `150%`: it must register, and the reported page
   coordinates must match the pointer position mapped through the zoom.
4. Command+scroll over the page: the percentage follows the gesture and the
   page must not zoom twice (one step per gesture, not a compounded jump).
5. Release Command and scroll: the page scrolls normally.
6. Resize the panel at a non-100% zoom: the emulated viewport follows the new
   body size at the same scale, and pointer input stays aligned.
7. On the board, pin a viewport through the public browser MCP. The selector
   shows disabled `Fit`: a native pinch over the page zooms the canvas, while
   Command+scroll reaches the page without changing the canvas. Switch between
   these inputs rapidly and confirm each keeps its own behavior. In fullscreen,
   a native pinch must keep the fixed content fitted and leave the saved board
   zoom unchanged; Command+scroll remains browser-owned. Clear the pin and
   confirm responsive panel zoom returns.
8. Open a native select menu in a responsive or pinned browser. New pinch and
   Command+scroll gestures inside the menu must move neither page nor canvas;
   ordinary menu scrolling must still work. Outside the menu, responsive content
   must dismiss it and zoom; pinned content must dismiss it and retain the
   wheel/pinch behavior from step 7. Repeat in fullscreen, after changing the
   chrome height, and while moving the pointer across the menu during one gesture.

## 5. Lane D — canvas interaction

With both panels on the canvas, pinch over empty canvas: the board zooms as
before. Pinch over each panel: only that panel's content zooms. Neither case
may move the canvas pan offset unexpectedly.

## 6. Visual evidence

Zoom is a visible, motion-sensitive change, so lanes B, C and D each require
artifacts captured from the isolated desktop (never from the developer's
screen):

- A screenshot after launch, and one at every zoom level listed in the lane.
- One short video per interactive lane covering the gesture itself — the
  percentage changing and the content scaling under the pointer. Verify the
  recording has usable frames before accepting it; if recording is
  unavailable or produces no frames, report the lane as blocked.

Evidence stays private unless publication is separately authorized, and only
synthetic fixture content is eligible.

## Pass criteria

| Lane | Required |
|---|---|
| A | Every cargo command passes on macOS |
| B | Dropdown, pinch, Command+wheel and pan all work; target desktop unchanged; controls still work on a retained frame |
| C | Viewport scales inversely with zoom; clicks land; page scroll intact |
| D | Panels own the gesture over their body; empty canvas and panel titlebars still zoom the board |

A lane passes only with the artifacts from section 6 attached; without them
it is reported as blocked, not done. Include the tested SHA, verified child
PID and executable hash, macOS version, architecture, and each lane's result
before the completion marker. Only after every requested lane passes, end the
report with this exact final line, with no text after it:

```text
SMOKE-TEST: DONE
```

Lanes B, C and D require a real trackpad or mouse on a task-owned isolated
desktop; a headless runner and the developer's own desktop are both
unacceptable substitutes.
