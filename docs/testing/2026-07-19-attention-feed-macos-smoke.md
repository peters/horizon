# macOS Attention Feed Regression Smoke Plan

## Purpose

Validate the complete attention-feed fix on macOS from a clean Horizon runtime.
This plan covers explicit notifications, prompt heuristics, lifecycle timing,
navigation, overlay geometry, input isolation, workspace changes, and restore
failures. Run it against the exact commit that will be pushed or merged.

The feed is enabled by default. Do not use an environment override to enable it
for the baseline lanes; that would hide a default-configuration regression.

## Required machine and permissions

- macOS 14 or newer on Apple Silicon or Intel.
- Xcode Command Line Tools.
- A mouse and trackpad, or a trackpad capable of wheel, pinch, and drag input.
- Accessibility permission for Terminal if the exact-size AppleScript is used.
- Screen Recording permission for Terminal so `screencapture` can record motion.
- Only one Horizon process running. Scope automation to the PID captured below.

## Build and record the revision

From the PR checkout:

```bash
git fetch origin
git status --short
git rev-parse HEAD
cargo build
```

Create an isolated runtime and record the revision/platform:

```bash
export ATTENTION_SMOKE_ROOT="$(mktemp -d /tmp/horizon-attention-smoke.XXXXXX)"
export ATTENTION_SMOKE_HOME="$ATTENTION_SMOKE_ROOT/home"
export ATTENTION_SMOKE_BIN="$ATTENTION_SMOKE_ROOT/bin"
export ATTENTION_SMOKE_ARTIFACTS="$ATTENTION_SMOKE_ROOT/artifacts"
mkdir -p "$ATTENTION_SMOKE_HOME/.horizon"
mkdir -p "$ATTENTION_SMOKE_BIN"
mkdir -p "$ATTENTION_SMOKE_ARTIFACTS"

git rev-parse HEAD > "$ATTENTION_SMOKE_ARTIFACTS/commit.txt"
git status --short > "$ATTENTION_SMOKE_ARTIFACTS/status.txt"
sw_vers > "$ATTENTION_SMOKE_ARTIFACTS/sw-vers.txt"
uname -m > "$ATTENTION_SMOKE_ARTIFACTS/architecture.txt"
```

Every launch below passes the isolated directory through `HOME=...`; do not use
the tester's normal Horizon home.

## Install deterministic fixtures

The interactive fixture accepts commands typed into its terminal. It emits the
same prompt strings and OSC notifications as real agent adapters, without
network access or credentials.

```bash
cat > "$ATTENTION_SMOKE_BIN/attention-fixture" <<'FIXTURE'
#!/bin/zsh
set -u

osc_notify() {
  printf '\033]0;HORIZON_NOTIFY:%s:%s\007' "$1" "$2"
}

replace_screen() {
  printf '\033[2J\033[H%s' "$1"
}

# Appears and disappears inside startup grace. It must not create an item.
replace_screen '❯ startup-grace-control'
sleep 0.2
replace_screen 'fixture starting'
sleep 11
printf '\nFIXTURE_READY HORIZON=%s\n' "${HORIZON:-unset}"

while IFS= read -r action; do
  case "$action" in
    approval)
      replace_screen 'Allow deployment? [y/N]'
      ;;
    question)
      replace_screen 'What should I do next?'
      ;;
    ready)
      replace_screen '❯ '
      ;;
    clear)
      replace_screen $'working\nline 2\nline 3\nline 4'
      ;;
    fast)
      replace_screen '❯ '
      replace_screen 'working immediately'
      ;;
    attention)
      osc_notify attention 'one-shot attention'
      ;;
    done)
      osc_notify done 'one-shot done'
      ;;
    info)
      osc_notify info 'one-shot info'
      ;;
    burst)
      osc_notify attention 'queued-1-attention'
      osc_notify done 'queued-2-done'
      osc_notify info 'queued-3-info'
      ;;
    many)
      integer item_number=1
      while (( item_number <= 12 )); do
        osc_notify attention "open-$item_number"
        (( item_number += 1 ))
      done
      ;;
    unicode)
      osc_notify attention '审查这个变更是否可以合并 ✅'
      ;;
    long)
      osc_notify attention 'This is a deliberately very long attention summary with emoji 🚦 and non-ASCII text åpen handling that must truncate safely without crossing the titlebar controls'
      ;;
    scroll-ready)
      replace_screen ''
      integer history_line=1
      while (( history_line <= 80 )); do
        printf 'history line %02d\n' "$history_line"
        (( history_line += 1 ))
      done
      printf '❯ '
      ;;
    quit)
      exit 0
      ;;
    *)
      printf '\nunknown fixture action: %s\n' "$action"
      ;;
  esac
done
FIXTURE
chmod +x "$ATTENTION_SMOKE_BIN/attention-fixture"
```

The one-shot fixture emits either one explicit notification or one startup
approval prompt and then remains silent. It catches delivery that is incorrectly
gated on later terminal output. It scans all arguments because agent
integrations may prepend their own arguments.

```bash
cat > "$ATTENTION_SMOKE_BIN/attention-one-shot" <<'FIXTURE'
#!/bin/zsh
set -u

severity=attention
mode=notification
for candidate in "$@"; do
  case "$candidate" in
    approval)
      mode=approval
      break
      ;;
    attention|done|info)
      severity="$candidate"
      break
      ;;
  esac
done

if [[ "$mode" == approval ]]; then
  printf 'Allow startup action? [y/N]'
else
  printf '\033]0;HORIZON_NOTIFY:%s:startup-one-shot-%s\007' \
    "$severity" "$severity"
fi
sleep 120
FIXTURE
chmod +x "$ATTENTION_SMOKE_BIN/attention-one-shot"
```

## Create the isolated config

```bash
cat > "$ATTENTION_SMOKE_HOME/.horizon/config.yaml" <<YAML
version: 8
window:
  width: 1280
  height: 860
overlays:
  attention_feed_height: 600
  attention_feed_width: 320
  minimap_height: 180
  minimap_width: 320
features:
  attention_feed: true
presets:
  - name: Fixture Codex
    alias: fixture-codex
    kind: codex
    command: $ATTENTION_SMOKE_BIN/attention-fixture
    resume: fresh
  - name: Fixture Claude
    alias: fixture-claude
    kind: claude
    command: $ATTENTION_SMOKE_BIN/attention-fixture
    resume: fresh
  - name: One-shot Attention
    alias: one-attention
    kind: codex
    command: $ATTENTION_SMOKE_BIN/attention-one-shot
    args: [attention]
    resume: fresh
  - name: One-shot Done
    alias: one-done
    kind: codex
    command: $ATTENTION_SMOKE_BIN/attention-one-shot
    args: [done]
    resume: fresh
  - name: One-shot Info
    alias: one-info
    kind: codex
    command: $ATTENTION_SMOKE_BIN/attention-one-shot
    args: [info]
    resume: fresh
  - name: Startup Approval
    alias: startup-approval
    kind: codex
    command: $ATTENTION_SMOKE_BIN/attention-one-shot
    args: [approval]
    resume: fresh
  - name: Shell
    alias: sh
    kind: shell
workspaces:
  - name: Attached Agents
    cwd: $ATTENTION_SMOKE_ROOT
    position: [0, 0]
    terminals:
      - name: Codex Fixture
        kind: codex
        command: $ATTENTION_SMOKE_BIN/attention-fixture
        resume: fresh
        position: [40, 60]
        size: [580, 360]
      - name: Claude Fixture
        kind: claude
        command: $ATTENTION_SMOKE_BIN/attention-fixture
        resume: fresh
        position: [660, 60]
        size: [580, 360]
      - name: Shell Control
        kind: shell
        position: [40, 460]
        size: [580, 280]
  - name: Move Target
    cwd: $ATTENTION_SMOKE_ROOT
    position: [1500, 0]
    terminals:
      - name: Target Shell
        kind: shell
        position: [40, 60]
        size: [580, 360]
YAML
```

The agent fixtures must print `FIXTURE_READY HORIZON=1`. If either does not,
report setup failure instead of interpreting missing alerts as a pass.

## Launch and capture the exact process

Launch without `--ephemeral`; persistence and restore lanes need the normal
session store.

```bash
HOME="$ATTENTION_SMOKE_HOME" \
  RUST_LOG=horizon=info,horizon_core=info \
  target/debug/horizon \
  --config "$ATTENTION_SMOKE_HOME/.horizon/config.yaml" \
  > "$ATTENTION_SMOKE_ARTIFACTS/horizon.log" 2>&1 &
export ATTENTION_SMOKE_PID=$!

ps -p "$ATTENTION_SMOKE_PID" -o pid=,command=
```

If the session chooser appears, create the config-based session on first launch
and resume that same session later. Do not locate windows only by app title.

Optional exact window sizing, after granting Accessibility permission:

```bash
resize_horizon() {
  osascript - "$ATTENTION_SMOKE_PID" "$1" "$2" <<'APPLESCRIPT'
on run argv
  set targetPid to (item 1 of argv) as integer
  set targetWidth to (item 2 of argv) as integer
  set targetHeight to (item 3 of argv) as integer
  tell application "System Events"
    set targetProcess to first application process whose unix id is targetPid
    set size of front window of targetProcess to {targetWidth, targetHeight}
  end tell
end run
APPLESCRIPT
}
```

## Lane 1 — Default, startup grace, and OSC severities

1. Leave both fixtures untouched until `FIXTURE_READY HORIZON=1` appears.
2. Confirm the transient startup prompt created no feed item or badge.
3. In Codex type `attention`, `done`, then `info`, pausing after each.
4. Repeat in Claude.
5. Dismiss each item before the next subtest.
6. Add the three One-shot notification presets one at a time. Do not type into
   them.
7. Add Startup Approval and inspect it before the ten-second launch grace ends.

Expected:

- The feed appears without an enable override.
- Codex and Claude create one item per command.
- `attention`, `done`, and `info` map to `ATTENTION`, `DONE`, and `INFO` with
  their distinct normal colors.
- Explicit items stay open until dismissed; heuristic reconciliation does not
  immediately resolve them.
- Each silent one-shot item appears promptly without later panel output.
- The startup approval prompt appears during launch grace; only the ordinary
  transient ready prompt is baselined as startup noise.
- No duplicates appear.

Capture `01-explicit-severities.png`.

## Lane 2 — Multiple queued notifications

1. Clear existing items.
2. Type `burst` once in Codex, then once in Claude.

Expected:

- Each burst creates all three items; none is overwritten or lost.
- Newest-first order is item 3, item 2, item 1.
- The Codex and Claude bursts remain distinct, and Go to targets their source.

Capture `02-queued-notifications.png`.

## Lane 3 — Prompt heuristics, fast transitions, and scrollback

Run separately in Codex and Claude:

1. Type `approval`; verify one high-severity prompt item.
2. Type `question`; approval resolves and a new question item opens.
3. Type `ready`; question resolves and a ready item opens.
4. Type `clear`; ready resolves, remains visually recent, then disappears after
   about 30 seconds.
5. Type `fast`; no stale ready item may remain after the immediate working state.
6. Type `scroll-ready`, wait for its item, then scroll the terminal upward until
   the live prompt is off-screen.
7. Leave it scrolled up for five seconds. Return to the live bottom and type
   `clear`.

Expected:

- At most one open heuristic item exists per panel.
- Reconciliation resolves only heuristic items, never explicit OSC items.
- Fast transitions reflect the final screen and leave no ghost.
- Scrollback does not resolve or duplicate a ready item; detection follows the
  terminal's live bottom, not the user's display offset.
- Clearing at the live bottom resolves once.

In Shell Control run:

```bash
printf 'Allow deployment? [y/N]\nWhat should I do next?\n❯ '
```

Expected: shell text creates no heuristic item or badge.

## Lane 4 — Idle ready auto-dismiss

1. Clear all items and type `ready`.
2. Record when the item appears.
3. Cause no terminal output and wait 50 seconds.

Expected:

- It auto-dismisses at approximately 45 seconds despite no terminal event.
- Feed, titlebar, and sidebar state clear together.
- It does not reappear while the unchanged prompt remains.
- A later new ready prompt can create a fresh item.

Record the observed duration.

## Lane 5 — Dismiss, card click, and attached Go to

Arrange a normal panel partly behind the feed and create alerts from both agents.

1. Focus Shell Control.
2. Click only the dismiss `×` on the Codex alert.
3. Click the body of the Claude alert.
4. Create another Codex alert, switch to Move Target, and click its Go to.

Expected:

- Dismiss does not also activate the card or Go to.
- Card and Go-to actions each fire once.
- No action focuses a panel underneath the feed.
- Go to selects Attached Agents, focuses the exact source, and brings it into
  view without an extra canvas jump.
- Identical summaries from different panels navigate to their own sources.

Capture `05-attached-navigation.png`.

## Lane 6 — More than ten open plus recent resolved

1. Type `ready`, `approval`, `question`, then `clear`, pausing between commands,
   to create recent resolved history.
2. Immediately type `many` to add twelve open items.
3. Scroll from the newest row to the bottom and account for all twelve open
   `open-N` items before any recent resolved row.
4. Dismiss one open item and confirm the other eleven remain reachable in their
   stable order.

Expected:

- The header reports the true open count, not just rendered rows.
- All open actionable rows remain shown and reachable by scrolling, including
  when more than ten are open.
- Recent resolved rows come after every open row and never displace one.
- Ordering is stable and newest first.
- Resolved rows age out at the normal interval.

Capture `06-capacity-and-resolved.png`, including the count.

## Lane 7 — Unicode and long badges

1. Clear items and type `unicode`.
2. Inspect source titlebar and feed row.
3. Type `long`.
4. Resize the source panel from wide to its smallest practical width.

Expected:

- No panic or UTF-8 boundary error.
- Unicode and emoji truncate only at valid text boundaries.
- Long badges clip or ellipsize within available titlebar width.
- Badges never overlap title, history meter, detach, maximize, or close.
- Feed text and actions remain usable.

Capture `07-unicode-wide.png`, `07-unicode-narrow.png`, and the log segment.

## Lane 8 — Workspace move and removal

1. Create an open explicit alert and an open heuristic alert in Codex.
2. Move that panel from Attached Agents to Move Target.
3. Confirm alerts and badge remain; use Go to on both.
4. Move or close remaining source panels, then remove the original workspace.
5. Recheck the moved panel's alerts.

Expected:

- Every attention item updates to the panel's destination workspace.
- Removing the original workspace does not delete moved-panel alerts.
- Sidebar counts and Go to follow Move Target.
- Removing a workspace still removes alerts for panels actually closed with it.

Capture `08-moved-panel-alerts.png`.

## Lane 9 — Detached Go to

1. Put a fixture panel with an open alert in a workspace and detach it.
2. Focus the root window and move its canvas away from the former position.
3. From the root feed, click the alert card and Go to.
4. Reattach and repeat.

Expected:

- Go to raises the detached native window and focuses the exact source panel.
- Root canvas does not pan or zoom toward invisible detached content.
- No focus oscillation occurs.
- Reattached navigation works normally.

Capture decisive motion evidence:

```bash
screencapture -V 10 "$ATTENTION_SMOKE_ARTIFACTS/09-detached-go-to.mov"
ps -p "$ATTENTION_SMOKE_PID" -o pid=,command=
```

Inspect root and child windows by exact PID and non-root membership, not title.

## Lane 10 — Settings toggle, minimap, and feed sizing

1. With open items present, open Settings.
2. Confirm feed/minimap neither cover nor intercept Settings.
3. Close Settings and confirm both return unchanged.
4. Open and close the command palette, Remote Hosts, Session Manager, a preset
   picker, and a populated toolbar-search results dropdown one at a time.
5. Toggle minimap off and on.
6. Toggle Attention Feed off.
7. While off, emit one OSC notification and run `clear`, `ready`, `clear`.
8. Wait five seconds, re-enable the feed, then emit one new notification.
9. Set feed width/height to UI minimums, then maximums, then 320 by 600.

Expected:

- Settings has exclusive interaction priority.
- Modal backdrops, pickers, and search results have exclusive interaction
  priority; feed/minimap hide and return with their state unchanged.
- Minimap toggling does not push the feed into Settings or off-screen.
- Disabling hides feed, sidebar attention UI, and titlebar badges.
- Completed events while disabled do not surface stale on re-enable.
- The first new post-enable event appears once.
- Settings bounds agree with rendered minimums; no silent clamping mismatch.
- Overlay toggles do not duplicate or reorder state.

Capture `10-settings-exclusive.png` and `10-overlays-restored.png`.

## Lane 11 — Window sizes and live resize

Test feed and minimap enabled at:

- 1600 by 1000
- 1280 by 860
- 1024 by 768
- 800 by 600

At each size, test feed size 320 by 600 and the configured maximum. Scroll first
to last row, open/close Settings, toggle minimap, then live-resize across the
target for at least ten seconds.

Expected:

- Feed geometry clamps to the actual available viewport.
- It never overlaps toolbar, minimap, Settings, or inaccessible screen space.
- Header, first row, last reachable open row, dismiss, and Go to remain usable.
- Feed/minimap keep their gap.
- Input exclusion follows actual clamped geometry throughout resize.
- No jitter, snap-back, alternating position, or stranded partial row.
- Root remains usable at 1024 by 768 and 800 by 600.

Capture screenshots and motion:

```bash
screencapture -x "$ATTENTION_SMOKE_ARTIFACTS/11-1600x1000.png"
screencapture -x "$ATTENTION_SMOKE_ARTIFACTS/11-1280x860.png"
screencapture -x "$ATTENTION_SMOKE_ARTIFACTS/11-1024x768.png"
screencapture -x "$ATTENTION_SMOKE_ARTIFACTS/11-800x600.png"
screencapture -V 12 "$ATTENTION_SMOKE_ARTIFACTS/11-live-resize.mov"
```

The motion clip, not a still screenshot, is decisive for resize stability.

## Lane 12 — Wheel, pinch, Space, and middle-button isolation

Use enough rows to scroll. Record root pan/zoom before the lane using the minimap
or persisted `runtime.yaml`.

With the pointer over a feed row, then empty space inside the feed:

1. Wheel and two-finger scroll.
2. Pinch and Command-wheel zoom.
3. Space plus primary-button drag.
4. Middle-button drag.
5. Primary click on dismiss, card, and Go to.

Expected over the feed:

- Wheel/two-finger input scrolls only the feed.
- Pinch and Command-wheel do not zoom root.
- Space/middle drags do not pan root, move a panel behind the feed, or start
  terminal selection.
- Presses do not focus an underlying panel.
- Dismiss, card, and Go to are mutually exclusive and fire once.

Control: repeat gestures over empty root canvas outside all overlays and
workspaces. Normal root pan/zoom must still work there.

```bash
screencapture -V 20 "$ATTENTION_SMOKE_ARTIFACTS/12-input-isolation.mov"
```

The clip must show before state, feed gestures with unchanged root/minimap, and
the empty-canvas control.

## Lane 13 — Persistence and restore failure

### Normal restart

1. Set a non-default feed size and leave the feature enabled.
2. Dismiss one item, resolve one heuristic item, and leave one explicit open.
3. Wait two seconds for save.
4. Find and copy the session's `runtime.yaml` into artifacts.
5. Quit with Command-Q, confirm the exact PID exits, and resume the same session.
6. Capture the new PID.

Expected:

- Feed dimensions and feature setting persist through their config path.
- Runtime-only dismissed, resolved, and open history does not duplicate or
  resurrect after a clean restart.
- Fresh post-restart events appear once.

### Restore failure

1. Quit Horizon.
2. Change one saved agent panel's `cwd` in `runtime.yaml` to the absolute
   nonexistent directory `$ATTENTION_SMOKE_ROOT/restore-cwd-does-not-exist`.
   Do not create that directory.
3. Resume the same session and inspect the restore failure without other output.
4. Click Go to, dismiss it, quit, and relaunch while the cwd remains invalid.

Expected:

- The failure placeholder creates exactly one high-severity restore alert.
- The alert remains open until dismissed; placeholder exit/output must not
  immediately resolve it.
- Go to targets the failed placeholder panel.
- A later restart creates one fresh alert for that attempt, not duplicated
  restored feed history.
- Horizon remains responsive and logs the invalid working-directory failure.

Capture `13-restore-failure.png`, failing runtime YAML, and the log excerpt.

## Final checks and required artifacts

Scan the log for panics and rendering failures:

```bash
ps -p "$ATTENTION_SMOKE_PID" -o pid=,command= \
  > "$ATTENTION_SMOKE_ARTIFACTS/final-process.txt"
rg -n "panic|panicked|UTF-8|thread .* panicked|wgpu.*error" \
  "$ATTENTION_SMOKE_ARTIFACTS/horizon.log" \
  > "$ATTENTION_SMOKE_ARTIFACTS/crash-scan.txt" || true
```

Required evidence:

- Exact commit, dirty status, macOS version, and architecture.
- Horizon log and empty crash scan.
- Screenshots for severities, queueing, navigation, count/capacity, Unicode,
  workspace move, Settings/minimap, restore failure, and all four sizes.
- Motion clips for detached Go to, live resize, and input isolation.
- Before/after pan and zoom evidence.
- Observed idle auto-dismiss duration.
- Normal and intentionally failing runtime state.
- Exact reason for any unexecuted lane.

## PR smoke request

Post after the PR exists:

```text
SMOKE-TEST REQUEST macOS — plan: docs/testing/2026-07-19-attention-feed-macos-smoke.md — scope: all lanes
```

Never include the completion marker in a request.

## Report template

```text
SMOKE-TEST REPORT (macOS <version>, <arm64|x86_64>)
Commit: <full SHA>
Artifacts: <link or attachment list>

- Build and isolated setup: pass | fail — <note>
- Default/startup grace/OSC severities: pass | fail — <note>
- Multiple queued notifications: pass | fail — <note>
- Prompt/fast/scrolled-terminal detection: pass | fail — <note>
- Idle 45-second auto-dismiss: pass | fail — observed <seconds>
- Dismiss/card/attached Go to: pass | fail — <note>
- More than ten open plus recent resolved: pass | fail — <note>
- Unicode and long badges: pass | fail — <note>
- Workspace move/removal: pass | fail — <note>
- Detached Go to: pass | fail — <note>
- Settings/minimap/toggle: pass | fail — <note>
- 1600x1000 resize: pass | fail — <note>
- 1280x860 resize: pass | fail — <note>
- 1024x768 resize: pass | fail — <note>
- 800x600 resize: pass | fail — <note>
- Wheel/pinch/Space/middle input isolation: pass | fail — <note>
- Persistence/restart/restore failure: pass | fail — <note>
- Crash and log scan: pass | fail — <note>

Summary: <fixes pushed, remaining failures, and anything the next agent must know>
SMOKE-TEST: DONE
```

The final line of a completed report must be exactly `SMOKE-TEST: DONE`. Add it
only after every requested lane has been executed and reported.
