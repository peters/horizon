# macOS Terminal Selection And Scrollback Smoke Plan

## Purpose

Validate terminal text selections while the viewport moves. The pass covers
same-frame pointer transitions, wheel remapping, timed out-of-body auto-scroll,
release behavior, scrollback boundaries, and ordinary terminal input.

## Machine And Evidence Requirements

- macOS desktop session with Swift/CoreGraphics access and screen-recording
  permission.
- Debug build from the exact PR commit under test.
- A disposable `HOME`; never mutate the tester's normal Horizon state.
- Identify the exact Horizon PID and its window before every automation lane.
- Motion-sensitive lanes require a short video or a timestamped high-frequency
  pointer/window trace. Still screenshots are supporting evidence only.

## Build And Isolated Launch

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/tmp/horizon-pr258-target
git rev-parse HEAD
cargo build
export SMOKE_ROOT="$(mktemp -d /tmp/horizon-selection-scroll.XXXXXX)"
export SMOKE_HOME="$SMOKE_ROOT/home"
mkdir -p "$SMOKE_HOME/.horizon"
cat >"$SMOKE_HOME/.horizon/config.yaml" <<YAML
version: 8
workspaces:
  - name: Selection Smoke
    cwd: $SMOKE_HOME
    terminals:
      - name: Fixture
        kind: shell
YAML
HOME="$SMOKE_HOME" RUST_LOG=horizon=debug \
  "$CARGO_TARGET_DIR/debug/horizon" \
  >"$SMOKE_ROOT/horizon.log" 2>&1 &
HORIZON_PID=$!
```

Confirm that exact PID owns one viewable Horizon window. Do not target a
window by application name when another Horizon process exists. Create this
PID-scoped Quartz lookup:

```bash
cat >"$SMOKE_ROOT/window-info.swift" <<'SWIFT'
import CoreGraphics
import Foundation

guard CommandLine.arguments.count == 2,
      let rawPid = Int32(CommandLine.arguments[1]) else {
    exit(64)
}
let pid = pid_t(rawPid)
let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
guard let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID)
        as? [[String: Any]] else {
    exit(1)
}
let candidates = windows.compactMap { info -> (Int, CGRect)? in
    guard (info[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid,
          (info[kCGWindowLayer as String] as? NSNumber)?.intValue == 0,
          let number = (info[kCGWindowNumber as String] as? NSNumber)?.intValue,
          let boundsDictionary = info[kCGWindowBounds as String] as? NSDictionary,
          let bounds = CGRect(dictionaryRepresentation: boundsDictionary) else {
        return nil
    }
    return (number, bounds)
}
guard let window = candidates.max(by: {
    $0.1.width * $0.1.height < $1.1.width * $1.1.height
}) else {
    exit(2)
}
print("\(window.0) \(Int(window.1.minX)) \(Int(window.1.minY)) " +
      "\(Int(window.1.width)) \(Int(window.1.height))")
SWIFT
read WINDOW_ID WIN_X WIN_Y WIN_W WIN_H < <(
  swift "$SMOKE_ROOT/window-info.swift" "$HORIZON_PID"
)
printf 'pid=%s window=%s bounds=%s,%s %sx%s\n' \
  "$HORIZON_PID" "$WINDOW_ID" "$WIN_X" "$WIN_Y" "$WIN_W" "$WIN_H"
```

Create the native event helper below. Coordinates are global Quartz
coordinates. Set `ANCHOR_X/Y`, `RELEASE_X/Y`, and `OUTSIDE_X/Y` after
inspecting a PID-scoped screenshot; `OUTSIDE` must remain inside the Horizon
window but outside the terminal body.

```bash
cat >"$SMOKE_ROOT/pointer-events.swift" <<'SWIFT'
import ApplicationServices
import Foundation

func number(_ index: Int) -> Double {
    guard CommandLine.arguments.count > index,
          let value = Double(CommandLine.arguments[index]) else {
        exit(64)
    }
    return value
}

func point(_ index: Int) -> CGPoint {
    CGPoint(x: number(index), y: number(index + 1))
}

func post(_ type: CGEventType, at point: CGPoint,
          flags: CGEventFlags = []) {
    guard let event = CGEvent(
        mouseEventSource: nil,
        mouseType: type,
        mouseCursorPosition: point,
        mouseButton: .left
    ) else {
        exit(1)
    }
    event.flags = flags
    event.post(tap: .cghidEventTap)
}

func scroll(lines: Int32) {
    guard let event = CGEvent(
        scrollWheelEvent2Source: nil,
        units: .line,
        wheelCount: 1,
        wheel1: lines,
        wheel2: 0,
        wheel3: 0
    ) else {
        exit(1)
    }
    event.post(tap: .cghidEventTap)
}

switch CommandLine.arguments.dropFirst().first {
case "batch-outside":
    let anchor = point(2)
    let release = point(4)
    let outside = point(6)
    post(.mouseMoved, at: anchor)
    post(.leftMouseDown, at: anchor)
    post(.leftMouseDragged, at: release)
    post(.leftMouseUp, at: release)
    post(.mouseMoved, at: outside)
    post(.leftMouseDown, at: outside)
    usleep(80_000)
    post(.leftMouseUp, at: outside)
case "batch-restart":
    let first = point(2)
    let release = point(4)
    let second = point(6)
    let end = point(8)
    post(.mouseMoved, at: first)
    post(.leftMouseDown, at: first)
    post(.leftMouseDragged, at: release)
    post(.leftMouseUp, at: release)
    post(.mouseMoved, at: second)
    post(.leftMouseDown, at: second)
    post(.leftMouseDragged, at: end)
    usleep(80_000)
    post(.leftMouseUp, at: end)
case "drag-wheel":
    let anchor = point(2)
    let end = point(4)
    let lines = Int32(number(6))
    post(.mouseMoved, at: anchor)
    post(.leftMouseDown, at: anchor)
    post(.leftMouseDragged, at: end)
    scroll(lines: lines)
    post(.leftMouseUp, at: end)
case "scroll":
    let target = point(2)
    let lines = Int32(number(4))
    post(.mouseMoved, at: target)
    scroll(lines: lines)
case "shift-then-alt":
    let anchor = point(2)
    let release = point(4)
    let target = point(6)
    post(.mouseMoved, at: anchor)
    post(.leftMouseDown, at: anchor, flags: .maskShift)
    post(.leftMouseDragged, at: release, flags: .maskShift)
    post(.leftMouseUp, at: release, flags: .maskShift)
    post(.mouseMoved, at: target)
    post(.leftMouseDown, at: target, flags: .maskAlternate)
    usleep(80_000)
    post(.leftMouseUp, at: target, flags: .maskAlternate)
default:
    exit(64)
}
SWIFT
```

## Deterministic Terminal Fixture

In a shell panel, generate enough uniquely numbered lines to exceed the
viewport:

```bash
i=1; while [ "$i" -le 300 ]; do printf 'HZNSEL-%03d abcdefghijklmnopqrstuvwxyz\n' "$i"; i=$((i+1)); done
```

Resize the panel so at least 20 rows remain visible. Record the first and last
visible markers before each lane.

## Baseline Selection

1. Drag within the terminal body across several rows and release.
2. Copy and verify the text starts and ends at the visible pointer rows.
3. Click without dragging and confirm the primary selection is not replaced by
   a spurious multi-row selection.
4. Start a new drag after a completed selection and confirm the new anchor is
   used.

## Wheel During Held Selection

For both wheel directions:

1. Press and hold on a visible marker.
2. Move to a second visible row without releasing.
3. Scroll one wheel step while still held.
4. Release over the terminal body.
5. Copy the result and verify its endpoint matches the row under the pointer
   after the viewport movement, with no one-row overshoot.

Repeat with wheel-before-press, press-before-wheel, wheel-before-release, and
release-before-wheel ordering. Verify only events that occur before selection
completion affect the final selection mapping.

## Same-Frame Transition Regression

Use `pointer-events.swift batch-outside` with no intentional delay before the
second press to send:

1. primary press inside the terminal body;
2. primary release inside the body;
3. a second primary press outside the body;
4. keep the second press held while the next frame is rendered;
5. release outside.

Verify the first local selection completes at its inside release. The outside
press must not restart or extend the local selection, keep auto-scroll alive,
or replace copied text. Repeat at least 20 times and record failures.

Then send release-inside followed by press-inside in one batch. Verify that
this valid local restart remains extendable on the next frame:

```bash
swift "$SMOKE_ROOT/pointer-events.swift" batch-outside \
  "$ANCHOR_X" "$ANCHOR_Y" "$RELEASE_X" "$RELEASE_Y" \
  "$OUTSIDE_X" "$OUTSIDE_Y"
swift "$SMOKE_ROOT/pointer-events.swift" batch-restart \
  "$ANCHOR_X" "$ANCHOR_Y" "$RELEASE_X" "$RELEASE_Y" \
  "$SECOND_X" "$SECOND_Y" "$EXTENDED_X" "$EXTENDED_Y"
```

OS event delivery cannot guarantee that these transitions land in one egui
frame. Treat the repeated native run as stress evidence; the focused Rust
tests are the decisive same-frame evidence.

## Timed Auto-Scroll Cadence

Start a selection inside the terminal body, hold the pointer just above the
body, and record for at least two seconds. Repeat just below the body.

During each run:

1. Generate unrelated repaints by producing terminal output from a second
   panel or by moving the held pointer horizontally within the same outside
   band.
2. Record the visible top/bottom marker progression before and during that
   extra activity.
3. Verify the extra repaints do not visibly accelerate auto-scroll.
4. Verify the endpoint follows each eligible viewport step.
5. Release outside and confirm scrolling stops on the next frame.

The exact 16 ms gate is asserted by deterministic tests with explicit
`RawInput.time`; a 60/120 fps recording is qualitative motion evidence, not a
timer measurement. Capture the exact PID's Quartz window:

```bash
screencapture -v -V 10 -l"$WINDOW_ID" \
  "$SMOKE_ROOT/selection-auto-scroll.mov"
```

## Boundary And Re-entry Checks

1. Auto-scroll upward until the oldest available history row is reached.
2. Keep holding outside and verify no repaint loop or viewport movement
   continues at the boundary.
3. Auto-scroll downward to live output and verify the same behavior.
4. Move the held pointer back into the body and verify outside auto-scroll
   stops immediately while normal drag extension continues.
5. Leave and re-enter the Horizon window, then release; no drag state remains
   stuck.

## Terminal Input Modes

1. In a normal shell, verify selection, copy, typing, wheel scrollback, panel
   resize, and fit-to-content still work.
2. Run `vim -Nu NONE +'set mouse=a' /tmp/hzn-mouse.txt` and verify its mouse
   events still route normally without Shift.
3. Hold Shift to create a local selection while mouse reporting is enabled;
   verify scroll remapping and release behavior match the normal shell lane.
4. Stress a Shift-local press/release followed immediately by an Alt+primary
   PTY-mouse press using `pointer-events.swift shift-then-alt`; verify Vim
   receives the later press and its release.
5. Switch focus between two panels during separate selections and confirm drag
   completion is scoped to the originating panel.

## Persistence And Cleanup

1. Quit cleanly, relaunch the exact binary with the disposable home, and repeat
   one wheel-during-selection lane.
2. Confirm no selection drag resumes from persisted state.
3. Stop only the exact smoke PID and verify no child Horizon processes remain.
4. Remove the disposable runtime and generated capture after the PR report has
   been posted.

## Report

Include:

- exact commit SHA and macOS version;
- build and focused test results;
- pass/fail for baseline, wheel ordering, same-frame transitions, cadence,
  boundary/re-entry, terminal modes, and relaunch;
- exact PID and window identifier;
- video or timestamped trace path plus representative marker samples;
- focused-test evidence for exact same-frame ordering and the 16 ms cadence;
- any lane that could not be automated and the reason.
