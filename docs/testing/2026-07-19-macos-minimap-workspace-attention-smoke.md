# macOS Smoke Plan: Minimap Workspace Spotlight + Attention

> Temporary validation artifact. Keep this file and
> `docs/design/2026-07-19-minimap-workspace-attention-concepts.html` in the draft
> pull request while validation is in progress. Remove both only after the full
> macOS pass succeeds, then rebuild and repeat the decisive checks listed under
> **Final-head cleanup pass**.

## Purpose and pass criteria

Validate the minimap workspace spotlight and attention interactions on macOS at
the exact pull-request head handed off by the implementation agent. The pass is
complete only when all of the following are true:

- the active workspace remains visually distinct with a brighter fill, static
  blue/foreground double outline, and a clamped `ACTIVE` tab;
- High, recently completed, and Info attention cues are correct, static, and
  gated by the Attention Feed feature;
- counts, `9+` clamping, marker limits, tiny-map collapse, severity precedence,
  and the 30-second completed window behave as specified;
- panel markers and workspace pills navigate to the intended target without
  resolving or dismissing the attention;
- ordinary minimap panning, viewport outline, fit/resize, themes, persistence,
  and detached-window scope have no regression;
- idle traces do not show a continuous repaint loop, and matched pointer-redraw
  measurements are recorded against the branch base;
- evidence is tied to the exact tested SHA and every failure is either fixed and
  fully retested or explicitly blocks readiness/merge.

This plan does not require agent credentials or network activity. It uses local
fake agent panels controlled through named pipes. Workspace-only attention has
no public UI injection path; its behavior is covered by the branch unit tests.

## Handoff values

The implementation agent must replace or provide every placeholder below in the
PR handoff. Do not infer a SHA from a branch name after testing starts.

The handoff comment must begin with this exact request line:

```text
SMOKE-TEST REQUEST macOS — plan: docs/testing/2026-07-19-macos-minimap-workspace-attention-smoke.md — scope: full macOS/Metal lane, final-head cleanup, readiness, and squash merge
```

```bash
export PR_NUMBER="<draft PR number>"
export PR_HEAD_SHA="<40-character handoff SHA>"
export EXPECTED_BRANCH="feature/minimap-workspace-spotlight"
export SOURCE_REPO="/path/to/local/horizon"
```

Record the values in the evidence comment. If the PR head changes at any point,
stop: the previous UI, screenshot, video, and performance evidence is stale.

## Machine and permissions

- macOS 14 or newer, Intel or Apple Silicon.
- Xcode Command Line Tools and the repository's Rust stable toolchain installed.
- A real logged-in Aqua desktop session, not a headless SSH-only session.
- Terminal (or the test runner) granted Accessibility permission for exact-PID
  window inspection and Quartz pointer automation.
- Terminal granted Screen Recording permission for screenshots/video.
- Mouse or trackpad available. Test a trackpad path when the machine has one.
- Prefer a built-in Retina display. If an external display is available, include
  one scale-transition check; otherwise record that as not available.

Capture machine facts before building:

```bash
sw_vers
uname -m
rustc --version
cargo --version
system_profiler SPDisplaysDataType
```

## Exact-SHA checkout and build

Create a disposable detached worktree so another Horizon checkout and any local
edits remain untouched:

```bash
cd "$SOURCE_REPO"
git fetch origin "$EXPECTED_BRANCH"
test "$(git rev-parse FETCH_HEAD)" = "$PR_HEAD_SHA"

export SMOKE_ROOT="$(mktemp -d /tmp/horizon-minimap-macos.XXXXXX)"
export SMOKE_CHECKOUT="$SMOKE_ROOT/checkout"
git worktree add --detach "$SMOKE_CHECKOUT" "$PR_HEAD_SHA"
cd "$SMOKE_CHECKOUT"

test "$(git rev-parse HEAD)" = "$PR_HEAD_SHA"
test -z "$(git status --porcelain)"
cargo build
```

Record the complete `cargo build` result. All UI validation below must launch
`$SMOKE_CHECKOUT/target/debug/horizon`, not a binary from another checkout.

## Isolated deterministic runtime

Use an isolated home so the smoke does not read or mutate the tester's normal
Horizon sessions, config, plugins, or agent state:

```bash
export SMOKE_HOME="$(mktemp -d /tmp/horizon-minimap-home.XXXXXX)"
export SMOKE_EVIDENCE="$SMOKE_ROOT/evidence"
mkdir -p "$SMOKE_HOME/.horizon" "$SMOKE_HOME/bin" \
  "$SMOKE_HOME/control" "$SMOKE_EVIDENCE"
touch "$SMOKE_HOME/.zshrc"
```

Create the fake agent. A High or Info event writes its notification and terminal
bell in one PTY write so Horizon observes both in the same update and keeps the
item open. A Done event intentionally writes only its notification; Horizon
resolves it in that output pass, making it a deterministic recently resolved
item in the feed's existing 30-second visibility window.

```bash
cat > "$SMOKE_HOME/bin/fake-agent" <<'ZSH'
#!/bin/zsh
set -eu

key="${1:?panel key is required}"
control_dir="$HOME/control"
fifo="$control_dir/$key.fifo"
mkdir -p "$control_dir"
rm -f "$fifo"
mkfifo "$fifo"
printf 'Fake agent %s ready\n' "$key"

while true; do
  payload=''
  if ! IFS= read -r payload < "$fifo"; then
    continue
  fi
  level="${payload%%|*}"
  message="${payload#*|}"
  case "$level" in
    high)
      wire_level='attention'
      keep_open='yes'
      ;;
    done)
      wire_level='done'
      keep_open='no'
      ;;
    info)
      wire_level='info'
      keep_open='yes'
      ;;
    *)
      printf 'Unknown smoke level: %s\n' "$level" >&2
      continue
      ;;
  esac

  if [[ "$keep_open" = 'yes' ]]; then
    printf '\033]0;HORIZON_NOTIFY:%s:%s\007\a' "$wire_level" "$message"
  else
    printf '\033]0;HORIZON_NOTIFY:%s:%s\007' "$wire_level" "$message"
  fi
done
ZSH
chmod +x "$SMOKE_HOME/bin/fake-agent"
```

Create a deterministic 2-by-2 board. Three separate Alpha panels allow marker
limit and newest-target checks. Beta can hold eleven open items on one exact
panel. Gamma separates completed and open precedence. Delta supplies a movable
Info target.

```bash
cat > "$SMOKE_HOME/.horizon/config.yaml" <<YAML
version: 8
window:
  width: 1500
  height: 950
appearance:
  theme: dark
features:
  attention_feed: true
overlays:
  attention_feed_width: 320
  attention_feed_height: 600
  minimap_width: 320
  minimap_height: 180
workspaces:
  - name: Alpha Build
    color: "#89b4fa"
    cwd: "$SMOKE_HOME"
    position: [0, 0]
    terminals:
      - name: Alpha High Old
        kind: gemini
        command: "$SMOKE_HOME/bin/fake-agent"
        args: ["alpha-high-old"]
        position: [25, 70]
        size: [300, 220]
      - name: Alpha Info
        kind: gemini
        command: "$SMOKE_HOME/bin/fake-agent"
        args: ["alpha-info"]
        position: [355, 70]
        size: [300, 220]
      - name: Alpha High New
        kind: gemini
        command: "$SMOKE_HOME/bin/fake-agent"
        args: ["alpha-high-new"]
        position: [190, 325]
        size: [300, 220]
  - name: Beta Review
    color: "#cba6f7"
    cwd: "$SMOKE_HOME"
    position: [1050, 0]
    terminals:
      - name: Beta Count
        kind: gemini
        command: "$SMOKE_HOME/bin/fake-agent"
        args: ["beta-count"]
        position: [30, 75]
        size: [620, 455]
  - name: Gamma Delivery
    color: "#a6e3a1"
    cwd: "$SMOKE_HOME"
    position: [0, 760]
    terminals:
      - name: Gamma Done
        kind: gemini
        command: "$SMOKE_HOME/bin/fake-agent"
        args: ["gamma-done"]
        position: [25, 75]
        size: [300, 390]
      - name: Gamma High
        kind: gemini
        command: "$SMOKE_HOME/bin/fake-agent"
        args: ["gamma-high"]
        position: [355, 75]
        size: [300, 390]
  - name: Delta Triage
    color: "#fab387"
    cwd: "$SMOKE_HOME"
    position: [1050, 760]
    terminals:
      - name: Delta Info
        kind: gemini
        command: "$SMOKE_HOME/bin/fake-agent"
        args: ["delta-info"]
        position: [25, 75]
        size: [300, 390]
      - name: Delta Quiet
        kind: shell
        position: [355, 75]
        size: [300, 390]
YAML
```

## Launch and exact-PID guard

The first launch creates one persistent session from the config. Keep the launch
Terminal open so `$HORIZON_PID` remains the authoritative process under test.

```bash
cd "$SMOKE_CHECKOUT"
export SMOKE_LOG="$SMOKE_EVIDENCE/horizon-debug.log"
HOME="$SMOKE_HOME" ZDOTDIR="$SMOKE_HOME" SHELL=/bin/zsh \
  RUST_LOG=horizon=info,horizon_core=info \
  target/debug/horizon --config "$SMOKE_HOME/.horizon/config.yaml" \
  --new-session >"$SMOKE_LOG" 2>&1 &
export HORIZON_PID=$!

kill -0 "$HORIZON_PID"
ps -p "$HORIZON_PID" -o pid=,ppid=,etime=,command=
```

Never replace this PID with `pgrep`, and never drive windows by the application
name alone. Before every detached-window or motion-sensitive check, enumerate
only windows owned by this PID:

```bash
osascript - "$HORIZON_PID" <<'APPLESCRIPT'
on run argv
  set targetPid to (item 1 of argv) as integer
  tell application "System Events"
    set matches to every application process whose unix id is targetPid
    if (count of matches) is not 1 then error "expected exactly one process for PID " & targetPid
    set targetProcess to item 1 of matches
    set report to "pid=" & targetPid & " process=" & (name of targetProcess) & " windows=" & (count of windows of targetProcess)
    repeat with currentWindow in windows of targetProcess
      set report to report & linefeed & "position=" & (position of currentWindow) & " size=" & (size of currentWindow)
    end repeat
    return report
  end tell
end run
APPLESCRIPT
```

Expected before detaching: one root window owned by `$HORIZON_PID`. If another
Horizon process exists, leave it alone and continue to scope every assertion to
the recorded PID.

## Attention controls

In a second Terminal, source these functions. Each write blocks until the exact
fake panel has opened its FIFO and sleeps long enough for a separate UI frame to
consume the event.

```bash
wait_for_panel() {
  local key="$1"
  local fifo="$SMOKE_HOME/control/$key.fifo"
  local tries=0
  while [[ ! -p "$fifo" ]]; do
    sleep 0.1
    tries=$((tries + 1))
    if (( tries > 200 )); then
      echo "timed out waiting for $fifo" >&2
      return 1
    fi
  done
}

send_attention() {
  local key="$1"
  local level="$2"
  local message="$3"
  wait_for_panel "$key"
  printf '%s|%s\n' "$level" "$message" > "$SMOKE_HOME/control/$key.fifo"
  sleep 0.55
}

seed_spotlight() {
  send_attention alpha-info info 'Alpha informational result'
  send_attention alpha-high-old high 'Alpha approval needed'
  send_attention alpha-high-new high 'Alpha newest urgent target'

  local n=1
  while (( n <= 11 )); do
    send_attention beta-count high "Beta open item $n"
    n=$((n + 1))
  done

  send_attention delta-info info 'Delta informational result'
}

seed_gamma_precedence() {
  send_attention gamma-done done 'Gamma completed successfully'
  send_attention gamma-high high 'Gamma urgent follow-up'
}
```

Run `seed_spotlight` once after all seven FIFOs exist. Run
`seed_gamma_precedence` immediately before the precedence test because the Done
cue intentionally expires 30 seconds after resolution. To reseed from a clean
state, quit the smoke process, relaunch the same persistent session without
`--new-session`, update `$HORIZON_PID`, and run the desired seed function again.

## Test 1: default minimap baseline

At the configured 320 by 180 map size:

1. Click Alpha Build in the sidebar to establish the initial active workspace,
   then confirm all four workspace frames preserve their distinct accent identity,
   labels remain readable, focused-panel fill remains visible, and the viewport
   outline remains distinct from workspace and attention outlines.
2. Confirm the active Alpha workspace has a brighter fill, two static outline
   strokes (blue plus foreground), and an `ACTIVE` tab that does not cover its
   label or escape the map bounds.
3. Switch active workspace through the sidebar in the order Alpha, Beta, Gamma,
   Delta, Alpha. The spotlight must move exactly once to the selected workspace;
   the previous workspace returns to its normal accent presentation.
4. Leave the pointer idle for at least 15 seconds. No active outline, pill,
   beacon, or tab may pulse, shimmer, or animate.
5. Capture `01-default-dark.png` with the exact SHA and PID recorded alongside it.

## Test 2: High, Info, counts, and marker limit

Run `seed_spotlight`, then verify:

1. Alpha has three open items, a red High `!` workspace pill with count `3`, and
   no more than two exact-panel beacons even though three distinct panels own
   attention. The beacons must not obscure labels or the `ACTIVE` tab.
2. Alpha's red High aggregate wins over its older blue Info item.
3. Beta has eleven open items, a red High cue whose visible count is exactly
   `9+`, and one exact-panel marker because all eleven items target the same
   panel.
4. Delta has one open Info item and a blue `i` cue, not a red or green cue.
5. Quiet Gamma has no attention cue before its dedicated seed.
6. Workspace and panel geometry, active fill, labels, focused-panel fill, and
   viewport outline remain legible under every cue.
7. Capture `02-high-info-counts.png`.

## Test 3: recently completed cue and open precedence

Run `seed_gamma_precedence` and complete steps 1 through 3 within 20 seconds:

1. Gamma has a recently resolved Done item and an open High item. Its workspace
   aggregate must be red High because open items take precedence.
   Capture `03-open-precedence.png` now, before dismissal.
2. In the Attention Feed, dismiss only `Gamma urgent follow-up` with its `x`.
   Dismissal is used here solely to expose the already resolved item.
3. With no open Gamma item left, Gamma immediately becomes a green completed
   `✓` cue. It must not inherit the original Done item's severity color if that
   differs from the completed-state rule. Immediately capture
   `04-recently-completed.png` while the cue is still inside its visibility
   window.
4. Wait until 30 seconds have elapsed since the Done event, then move the
   pointer once to request a normal UI frame. The green Gamma cue must disappear
   without leaving a stale pill or marker. Do not expect a timer-driven repaint;
   the cues are intentionally static.
5. If the 30-second window is missed, call
   `send_attention gamma-done done 'Gamma completed successfully'` and repeat;
   do not accept a guessed result.

## Test 4: click targets, tooltips, and navigation

Attention navigation must not change attention state.

1. Pan so Alpha High Old is clearly away from the canvas center, then hover its
   visible panel beacon. Confirm a useful tooltip names or identifies the target
   and state, the cursor communicates clickability, and the padded hit target is
   easier to acquire than the painted symbol alone.
2. Click the beacon once. Alpha becomes active, Alpha High Old becomes focused,
   and the canvas centers that panel. Alpha's open count remains `3`; nothing is
   resolved or dismissed.
3. Move the view away again, hover Alpha's workspace pill, and click it. The
   highest-severity newest target must win: `Alpha High New`, not Alpha High Old
   or Alpha Info. The panel becomes focused and centered, while Alpha's count
   remains unchanged.
4. Click the Beta workspace pill. It must focus and center Beta Count, and its
   visible count remains `9+`.
5. Probe the outer part of each padded cue hit target with mouse and trackpad.
   Hover must remain stable and must not accidentally initiate minimap panning.
6. Capture `05-click-target-focused.png`. If hover or focus jumps under motion,
   record a short video rather than relying on the still image.

## Test 5: attention ownership after moving a panel

1. Note Alpha's count (`3`) and Delta's count (`1`).
2. Use the Alpha Info panel context menu, choose **Move to Workspace**, and move
   it to Delta Triage.
3. Confirm Alpha immediately drops to `2`, its Info panel beacon does not remain
   at the old geometry, and Delta rises to `2` with the moved panel represented
   in Delta's geometry.
4. Click the moved panel beacon. It must activate Delta, focus Alpha Info in its
   new workspace, and keep both Info items open.
5. Pan away and back and fit the active workspace. No old-workspace ghost cue may
   reappear.
6. This move will be reused by the persistence test; do not move the panel back.

## Test 6: minimum size, badge collapse, and clamping

Open Settings, General, Overlays and set Map Width to `80` and Map Height to `60`.
The renderer clamps the effective map content to its supported 120 by 120
minimum.

1. Confirm the minimap remains fully inside the root viewport and usable at the
   effective minimum.
2. Panel beacons must collapse when their geometry is too small; no partial,
   clipped, or overlapping beacon may remain. Workspace pills continue to carry
   the aggregate state.
3. Beta remains `9+`, never `10`, `11`, `9++`, or clipped text.
4. The `ACTIVE` tab adapts or clamps within the available edge. It must not
   overlap a pill, leak outside the map, or become unreadable.
5. Workspace labels either use the existing adaptive placement or collapse
   cleanly. Do not accept text painted outside its workspace/map bounds.
6. Click one remaining workspace pill and verify navigation still works at the
   minimum size.
7. Drag empty minimap space and verify panning still works at the minimum size.
8. Capture `06-minimum-map.png`.
9. Restore Map Width `320` and Map Height `180`, then save Settings.

## Test 7: minimap panning and viewport regression

Use `screencapture -V` for this motion-sensitive test:

```bash
screencapture -V 12 "$SMOKE_EVIDENCE/07-minimap-pan.mov"
```

During the recording:

1. Drag from empty minimap background horizontally, vertically, and diagonally.
2. Confirm the canvas moves continuously with the drag, in the expected
   direction, with no snap-back or oscillation.
3. Confirm the minimap viewport outline tracks the resulting view and attention
   counts do not change.
4. Repeat after clicking a panel marker, then drag from empty map space. A prior
   cue click must not leave a stuck click/drag state.
5. Resize one panel, fit the active workspace, and resize the root window. The
   minimap and viewport outline must update without stale geometry.

## Test 8: Attention Feed feature gate

1. Make Alpha active so its non-attention spotlight is obvious.
2. Open Settings, General and disable **Attention Feed**. Save.
3. Confirm the Attention Feed and every minimap attention pill/beacon disappear
   immediately, including the existing Alpha/Beta/Delta items.
4. Confirm the active Alpha brighter fill, static double outline, and `ACTIVE`
   tab remain visible. Active highlighting is not feature-gated.
5. Confirm minimap panning and workspace switching still work while disabled.
6. Capture `08-attention-disabled.png`.
7. Re-enable **Attention Feed**, save, and confirm the existing in-memory cues
   return without duplicated counts. Do not emit a new fake event while disabled;
   disabled detection intentionally does not consume its terminal notification.

## Test 9: detached workspace scope and exact-window behavior

1. Reseed Gamma Done if needed, then detach Gamma Delivery from its workspace
   toolbar or sidebar context menu.
2. Re-run the exact-PID AppleScript. It must report two windows owned by the same
   `$HORIZON_PID`: the root plus one non-root detached member. Do not identify the
   detached window by title alone.
3. The root minimap must exclude Gamma workspace geometry and Gamma attention.
   The detached minimap must scope itself to Gamma only; Alpha, Beta, and Delta
   counts must not leak into it.
4. Activate Gamma in the detached window and confirm its active spotlight is
   drawn there. Activate Alpha from the root and confirm active state changes do
   not corrupt either scoped minimap.
5. Hover and click a Gamma cue in the detached minimap. It must focus/center the
   Gamma target inside the detached window and must not pan the root canvas or
   resolve attention. After the target reaches center, leave the pointer idle
   for 15 seconds and confirm navigation settles with no continued repaint loop.
6. Drag empty space in each minimap. Each drag affects only its own canvas view.
7. Resize and move the detached native window while sampling only the recorded
   PID. A motion trace is mandatory; in a second Terminal, run this exact-PID
   sampler and move/resize the detached window throughout its 12-second run:

   ```bash
   osascript - "$HORIZON_PID" \
     > "$SMOKE_EVIDENCE/10-detached-position-trace.txt" <<'APPLESCRIPT' &
   on run argv
     set targetPid to (item 1 of argv) as integer
     tell application "System Events"
       set matches to every application process whose unix id is targetPid
       if (count of matches) is not 1 then error "expected exact PID " & targetPid
       set targetProcess to item 1 of matches
       set report to "pid=" & targetPid
       repeat with sampleIndex from 1 to 600
         set report to report & linefeed & "sample=" & sampleIndex
         repeat with currentWindow in windows of targetProcess
           set report to report & " position=" & (position of currentWindow) & " size=" & (size of currentWindow)
         end repeat
         delay 0.02
       end repeat
       return report
     end tell
   end run
   APPLESCRIPT
   export DETACHED_TRACE_PID=$!
   # Move and resize only the detached member for the next 12 seconds.
   wait "$DETACHED_TRACE_PID"
   ```

   Confirm the trace is monotonic during each gesture and shows no snap-back,
   alternating coordinates, or repeated saved restore position. A 12-second
   `screencapture -V` video may be added as supporting evidence, but does not
   replace the exact-PID trace.
8. Capture `09-root-detached-scope.png` and
   `10-detached-minimap-scope.png`.
9. Reattach Gamma and confirm it returns to the root minimap exactly once with
   no duplicate or stale detached cue. Detach it again before the persistence
   test so detached restore is exercised.

## Test 10: root resize, fullscreen, Retina, and themes

1. Resize the root window from approximately 1500 by 950 down to its minimum,
   then back up. At both extremes, verify map containment, label placement,
   `ACTIVE` tab clamping, pill clamping, and clickable hit targets.
2. Enter and exit native macOS fullscreen. Verify the minimap relocates cleanly,
   remains clickable, and keeps the correct viewport outline.
3. Switch away with Cmd-Tab and back. Repeat one pill click and one minimap pan.
4. On a Retina display, inspect the double outline, one-pixel-style strokes,
   marker shapes, icons, and text for blur, unequal edges, or half-pixel clipping.
5. If an external display is available, move the exact-PID root and detached
   windows across displays with different scale factors and repeat one click and
   resize. Otherwise record `external scale transition: not available`.
6. Immediately before the theme comparison, run
   `send_attention gamma-done done 'Gamma theme completed'` so the green cue is
   inside its 30-second window. In Settings, force Light. Verify High red, Done green, Info blue, accent
   identity, foreground outline, focused-panel fill, labels, and viewport outline
   remain distinguishable without relying on color alone. Capture
   `11-light-retina.png`.
7. Force Dark again and capture `12-dark-retina.png`. Save Settings.

## Test 11: persistence and relaunch

This smoke intentionally uses a persistent isolated session.

1. Leave Alpha Info moved to Delta, leave Gamma detached, select a known active
   workspace/panel, and leave the minimap at 320 by 180, Dark, Attention Feed on.
2. Quit Horizon cleanly with Cmd-Q. Confirm the exact `$HORIZON_PID` exits.
3. Relaunch the same config and isolated home without `--new-session`:

   ```bash
   cd "$SMOKE_CHECKOUT"
   HOME="$SMOKE_HOME" ZDOTDIR="$SMOKE_HOME" SHELL=/bin/zsh \
     RUST_LOG=horizon=info,horizon_core=info \
     target/debug/horizon --config "$SMOKE_HOME/.horizon/config.yaml" \
     >>"$SMOKE_LOG" 2>&1 &
   export HORIZON_PID=$!
   kill -0 "$HORIZON_PID"
   ```

4. Re-run the exact-PID window inventory. Confirm the expected detached member,
   root window, active selection, panel move, board positions, and canvas views
   restore without a stale native-window position replay.
5. Attention items themselves are not a persisted schema. Before reseeding,
   confirm no stale minimap cue was serialized from the previous process.
6. Wait for the recreated FIFOs, then send:

   ```bash
   send_attention alpha-info info 'Moved panel after relaunch'
   send_attention gamma-done done 'Detached done after relaunch'
   ```

7. Alpha Info must appear under persisted Delta ownership, not Alpha. Gamma's
   recent completed cue must appear only in its detached scope.
8. Confirm config version remains `8`; this feature must not add a migration,
   persisted field, public config key, dependency, or version change.
9. Capture `13-persistence-relaunch.png` and retain the relevant runtime/config
   excerpts with secrets reviewed and removed.

## Matched pointer-redraw profiling

Profile the exact PR head and its branch base with the same window size, board,
signals, point sets, binary profile, trace mode, durations, and isolated runtime.
Keep generated harnesses and traces under `$SMOKE_ROOT`, not in the repository.

### Build both exact revisions

```bash
cd "$SOURCE_REPO"
git fetch origin main
export BASE_SHA="$(git merge-base "$PR_HEAD_SHA" origin/main)"
export BASE_CHECKOUT="$SMOKE_ROOT/base-checkout"
git worktree add --detach "$BASE_CHECKOUT" "$BASE_SHA"

cd "$BASE_CHECKOUT"
cargo build --profile profiling --features trace-profiling
cd "$SMOKE_CHECKOUT"
cargo build --profile profiling --features trace-profiling
```

Record `BASE_SHA`. Do not compare a moving `origin/main` binary or a release
binary against the PR profiling binary.

### Create the exact-PID pointer driver

The helper rejects point sets outside the largest on-screen window owned by the
supplied PID and posts native Quartz mouse-move events at a fixed 16 ms cadence.

```bash
cat > "$SMOKE_ROOT/pointer-sweep.swift" <<'SWIFT'
import ApplicationServices
import Foundation

func mousePoint() -> CGPoint {
    CGEvent(source: nil)?.location ?? .zero
}

func largestWindow(pid: Int) -> CGRect? {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    guard let raw = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] else {
        return nil
    }
    return raw.compactMap { info -> CGRect? in
        guard let owner = info[kCGWindowOwnerPID as String] as? Int, owner == pid,
              let bounds = info[kCGWindowBounds as String] as? CFDictionary else {
            return nil
        }
        return CGRect(dictionaryRepresentation: bounds)
    }.max { left, right in
        left.width * left.height < right.width * right.height
    }
}

if CommandLine.arguments.count == 2 && CommandLine.arguments[1] == "point" {
    let point = mousePoint()
    print("\(Int(point.x)),\(Int(point.y))")
    exit(0)
}

guard CommandLine.arguments.count >= 5,
      CommandLine.arguments[1] == "sweep",
      let pid = Int(CommandLine.arguments[2]),
      let cycles = Int(CommandLine.arguments[3]),
      let window = largestWindow(pid: pid) else {
    fputs("usage: pointer-sweep.swift point | sweep PID CYCLES X,Y [X,Y ...]\n", stderr)
    exit(2)
}

let points = CommandLine.arguments.dropFirst(4).compactMap { value -> CGPoint? in
    let parts = value.split(separator: ",")
    guard parts.count == 2, let x = Double(parts[0]), let y = Double(parts[1]) else { return nil }
    return CGPoint(x: x, y: y)
}
guard !points.isEmpty && points.allSatisfy(window.contains) else {
    fputs("every point must be inside the exact PID's largest on-screen window\n", stderr)
    exit(3)
}

for _ in 0..<cycles {
    for point in points {
        guard let event = CGEvent(
            mouseEventSource: nil,
            mouseType: .mouseMoved,
            mouseCursorPosition: point,
            mouseButton: .left
        ) else { continue }
        event.post(tap: .cghidEventTap)
        usleep(16_000)
    }
}
SWIFT
xcrun swiftc "$SMOKE_ROOT/pointer-sweep.swift" \
  -o "$SMOKE_ROOT/pointer-sweep"
```

With the PR profiling binary open at 1500 by 950, collect and save three point
sets. Move the pointer to each intended location, then run
`"$SMOKE_ROOT/pointer-sweep" point` and copy the reported
coordinate. Use exactly six points per set so 125 cycles at the fixed 16 ms
cadence produces a 12-second sweep:

- `EMPTY_POINTS`: empty canvas outside all workspace frames;
- `CHROME_POINTS`: visible panel titlebars/workspace chrome;
- `MINIMAP_POINTS`: empty minimap background, workspace edge, panel geometry,
  active tab, a panel beacon, and a workspace pill.

Reuse the exact coordinate strings for every comparison run. Do not recapture
them between base and PR runs unless the window geometry changed; if it did,
restart the entire matched series.

### Run the matched matrix

Quit the debug smoke process first. Never run two Horizon processes against the
same smoke home or FIFO directory. Before each matrix row, remove stale FIFO
nodes with this zsh command and let the new fake agents recreate them:

```bash
for fifo in "$SMOKE_HOME"/control/*.fifo(N); do rm -f "$fifo"; done

export ATTENTION_ON_CONFIG="$SMOKE_HOME/.horizon/config.yaml"
export ATTENTION_OFF_CONFIG="$SMOKE_ROOT/config-attention-off.yaml"
cp "$ATTENTION_ON_CONFIG" "$ATTENTION_OFF_CONFIG"
/usr/bin/sed -i '' 's/attention_feed: true/attention_feed: false/' \
  "$ATTENTION_OFF_CONFIG"
```

For each row below, use a fresh `--ephemeral` process, record `$!` as the exact
PID, wait for every FIFO, run `seed_spotlight`, wait 3 seconds, perform the
workload for 12 seconds, wait 2 seconds, then terminate that exact PID and save
its complete trace log.

| Revision | Attention Feed | Workload |
|---|---:|---|
| `BASE_SHA` | on | idle, no pointer movement |
| `BASE_SHA` | on | `EMPTY_POINTS` sweep |
| `BASE_SHA` | on | `CHROME_POINTS` sweep |
| `BASE_SHA` | on | `MINIMAP_POINTS` sweep |
| `PR_HEAD_SHA` | on | idle, no pointer movement |
| `PR_HEAD_SHA` | on | `EMPTY_POINTS` sweep |
| `PR_HEAD_SHA` | on | `CHROME_POINTS` sweep |
| `PR_HEAD_SHA` | on | `MINIMAP_POINTS` sweep |
| `PR_HEAD_SHA` | off | idle, no pointer movement |
| `PR_HEAD_SHA` | off | `MINIMAP_POINTS` sweep |

For the off rows, copy the config outside the repo and change only
`features.attention_feed` to `false`. Keep all workspaces and fake-panel output
identical. Select `$ATTENTION_ON_CONFIG` or `$ATTENTION_OFF_CONFIG` to match the
row. A representative Attention Feed on traced launch is:

```bash
HOME="$SMOKE_HOME" ZDOTDIR="$SMOKE_HOME" SHELL=/bin/zsh \
  HORIZON_TRACE_SPANS=1 RUST_LOG=info \
  "$SMOKE_CHECKOUT/target/profiling/horizon" \
  --config "$ATTENTION_ON_CONFIG" --ephemeral \
  > "$SMOKE_EVIDENCE/pr-minimap-pointer.log" 2>&1 &
PROFILE_PID=$!

# After FIFO readiness, seeding, and the fixed 3-second settle:
"$SMOKE_ROOT/pointer-sweep" sweep "$PROFILE_PID" 125 \
  "${MINIMAP_POINTS[@]}"
sleep 2
kill -TERM "$PROFILE_PID"
wait "$PROFILE_PID" || true
```

Use the trace summarization logic in `scripts/profile.sh` (its embedded
`summarize_trace_spans` routine) or an equivalent parser. For every row report
call count and normalized `avg_us`, not only total wall time, for at least:

- `horizon::app::update`;
- `horizon::app::lifecycle::render_active_view`;
- `horizon::app::panels::render_panels`;
- `egui::context::pass`.

Interpretation and rerun rules:

- The PR idle trace must settle; static attention cues must not introduce a
  continuous repaint/animation loop. Compare call counts as well as average cost.
- Compare base and PR only within the same workload. Do not compare an idle row
  with a pointer row or totals from different frame counts.
- The Attention Feed off rows must still render the active spotlight and must
  not retain attention-cue hover work.
- If a pointer cost looks materially worse, rerun the same row twice with a
  denser/longer sweep before deciding. Save all runs and report variance rather
  than selecting the best result.
- Capture a live screenshot after the minimap pointer workload; a faster trace
  is invalid if culling or layering made content disappear.

## Evidence to post on the draft PR

Post one full-run comment tied to `PR_HEAD_SHA` using this template. This first
report is evidence for the pre-cleanup head; do not post the exact
`SMOKE-TEST: DONE` completion marker until the temporary artifacts are removed
and the final-head decisive pass succeeds.

```text
SMOKE-TEST REPORT (macOS)

PR head SHA:
Base SHA used for perf:
macOS version / build:
Architecture:
Rust / Cargo:
GPU and display(s), scale/Retina:
Input devices:
Exact Horizon PID(s) used:

Build: PASS/FAIL
Default active spotlight: PASS/FAIL
High / Info / Done / precedence: PASS/FAIL
Counts, 9+, marker limit, tiny collapse: PASS/FAIL
Panel beacon navigation: PASS/FAIL
Workspace pill newest-target navigation: PASS/FAIL
Move-to-workspace attention ownership: PASS/FAIL
Minimap pan / viewport / fit: PASS/FAIL
Attention disabled, active still visible: PASS/FAIL
Detached scope and exact-PID windows: PASS/FAIL
Resize / fullscreen / Retina / themes: PASS/FAIL
Persistence / relaunch: PASS/FAIL
Static idle / no continuous repaint: PASS/FAIL

Perf table: workload, revision, feature state, calls and avg_us for key spans
Performance interpretation and rerun variance:

Screenshots:
Videos:
Trace logs / summaries:
Failures or deviations:
External display transition: PASS/FAIL/NOT AVAILABLE
Overall result: PASS/FAIL
```

Attach or link the numbered screenshots, required motion video and exact-PID
position trace, exact-PID inventory, debug log, trace summaries, and raw logs.
Screenshots are supporting evidence only; the pan and detached movement verdicts
must use their required motion evidence even when the UI looks stable.

## Failure, correction, and rerun policy

- A crash, stale cue, wrong target, clipped badge/tab, lost accent, scope leak,
  continuous repaint, pan regression, or exact-PID window anomaly is a failure.
- Preserve the log, precise interaction sequence, screenshot/video, window
  inventory, machine facts, and SHA before changing code.
- Any code correction creates a new head. Rerun formatting and every repository
  gate affected by the correction, then rerun this entire macOS smoke plan—not
  only the step that originally failed—against the new exact SHA.
- This checkout is intentionally detached. Before publishing a correction,
  guard against overwriting a newer remote head and push the new commit
  explicitly to the feature branch:

  ```bash
  git fetch origin "$EXPECTED_BRANCH"
  test "$(git rev-parse FETCH_HEAD)" = "$PR_HEAD_SHA"
  # Apply the correction and commit it in this detached worktree.
  export PR_HEAD_SHA="$(git rev-parse HEAD)"
  git push origin HEAD:"refs/heads/$EXPECTED_BRANCH"
  ```

  A rejected push means the remote head changed; stop, fetch the new head, and
  restart the validation rather than forcing the branch.
- Documentation-only evidence corrections do not require rerunning UI behavior,
  but the posted SHA and artifact names must remain exact.
- Do not mark the PR ready and do not merge while any result is missing,
  ambiguous, tied to an old SHA, or failed.

## Final-head cleanup pass

Only after the full pass and evidence comment are complete:

1. Remove both temporary artifacts from the PR branch:

   ```bash
   git fetch origin "$EXPECTED_BRANCH"
   test "$(git rev-parse FETCH_HEAD)" = "$PR_HEAD_SHA"
   git rm docs/design/2026-07-19-minimap-workspace-attention-concepts.html \
     docs/testing/2026-07-19-macos-minimap-workspace-attention-smoke.md
   git commit -m "docs: remove minimap validation artifacts"
   export PR_HEAD_SHA="$(git rev-parse HEAD)"
   git push origin HEAD:"refs/heads/$EXPECTED_BRANCH"
   ```

2. Treat the pushed cleanup commit as the final head. If the push is rejected,
   do not force it; restart from the current remote head.
3. Verify, rebuild, and launch a clean final-head process. Do not reuse a PID,
   FIFO, process, or screenshot from the pre-cleanup or profiling runs:

   ```bash
   test "$(git rev-parse HEAD)" = "$PR_HEAD_SHA"
   cargo build
   for fifo in "$SMOKE_HOME"/control/*.fifo(N); do rm -f "$fifo"; done

   export FINAL_SMOKE_LOG="$SMOKE_EVIDENCE/horizon-final-head.log"
   HOME="$SMOKE_HOME" ZDOTDIR="$SMOKE_HOME" SHELL=/bin/zsh \
     RUST_LOG=horizon=info,horizon_core=info \
     target/debug/horizon --config "$SMOKE_HOME/.horizon/config.yaml" \
     --ephemeral >"$FINAL_SMOKE_LOG" 2>&1 &
   export HORIZON_PID=$!
   kill -0 "$HORIZON_PID"
   ps -p "$HORIZON_PID" -o pid=,ppid=,etime=,command=

   for key in alpha-high-old alpha-info alpha-high-new beta-count \
     gamma-done gamma-high delta-info; do
     wait_for_panel "$key"
   done
   seed_spotlight
   seed_gamma_precedence
   ```

   Re-run the exact-PID window inventory and record this new PID with the final
   SHA before interacting.
4. Against the final head, repeat at minimum:
   - active switching and the static double outline/`ACTIVE` tab;
   - one High, one Info, and one recent Done cue plus open precedence;
   - Alpha marker click and workspace-pill newest-target click with counts intact;
   - default and minimum map resize/clamping;
   - one minimap pan and viewport-outline update;
   - Attention Feed off with active spotlight still present;
   - detached-scope cue navigation and exact-PID window count;
   - root resize and one Dark/Light check.
5. Post the final cleanup SHA and decisive-pass result to the PR using a new
   comment headed `SMOKE-TEST REPORT (macOS)`. Only when every final-head item
   passes, refresh current-head CI, mergeability, and unresolved review threads,
   then end that comment with the exact standalone line
   `SMOKE-TEST: DONE`. Any code change after cleanup requires the complete plan
   again; do not reuse pre-cleanup proof or its completion marker.

The macOS agent may mark the PR ready and squash-merge only when the final head
is mergeable, current-head CI is green, all review threads are resolved, the
macOS result is PASS, and the two temporary artifacts are absent. Use a squash
merge only; the implementation/Linux agent does not merge this PR.
