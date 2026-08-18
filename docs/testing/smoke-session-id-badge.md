# Smoke test — session-id badge in agent panel titlebars (temporary)

Temporary validation plan for `feat/session-id-badge`. Delete after the pass is complete.

## What changed

Agent panels (Pi, Codex, Claude, OpenCode, Gemini, KiloCode, Grok) whose session is known
(captured binding or explicit `resume: session`) now show a small monospace pill with the
first 8 characters of the session id in the titlebar, left of the working-dots slot.
`Panel::session_id()` is the single source of truth; the private `panel_session_id` helper
in `app/session.rs` was removed in favor of it.

## Prerequisites

- Linux X11 or macOS, `target/debug/horizon` built from this branch
  (`cargo build` in the branch worktree).
- `pi` (or any supported agent CLI) on PATH.
- Screenshot tooling: `import -window <wid>` (ImageMagick) or `scrot`.
- For automated image inspection on a vision-less model, a one-shot vision CLI works:
  `claude -p "<question about <png path>>"`.

## Setup (isolated instance — never smoke on the operator's live session)

```bash
SMOKE_HOME=$(mktemp -d)
mkdir -p "$SMOKE_HOME/.horizon"
cat > "$SMOKE_HOME/.horizon/config.yaml" <<EOF
version: 9
window:
  width: 1100.0
  height: 700.0
features:
  attention_feed: false
  organize_workspaces_on_session_load: false
  speech:
    enabled: false
workspaces:
  - name: Smoke
    position:
      - 40.0
      - 40.0
    terminals:
      - name: Pi Panel
        kind: pi
        position:
          - 0.0
          - 0.0
        size:
          - 560.0
          - 380.0
      - name: Shell Panel
        kind: shell
        position:
          - 600.0
          - 0.0
        size:
          - 440.0
          - 380.0
EOF
cd "$SMOKE_HOME"
HOME="$SMOKE_HOME" DISPLAY=<your-display> nohup <repo>/target/debug/horizon > "$SMOKE_HOME/horizon.log" 2>&1 &
```

Wait ~20 s for the window and the pi TUI to come up. Find the test window by PID
(`xdotool search --pid <pid>`; if it returns nothing, use
`xwininfo -root -children | grep Horizon` and pick the ~1128x766 window — the big one is
the operator's instance). Scope every check to that window id/PID.

Seed a pi session file so the dynamic binding capture has a candidate (pi does not write a
session file until its first turn):

```bash
SESS_DIR="$SMOKE_HOME/.pi/agent/sessions/--$(echo "$SMOKE_HOME" | tr / -)_"
# fallback: the exact cwd-scoped dir under $SMOKE_HOME/.pi/agent/sessions/ (ls to confirm)
cat > "$SESS_DIR/01a0beef-1234-7abc-8def-cafe00000001.jsonl" <<EOF
{"type":"session","id":"01a0beef-1234-7abc-8def-cafe00000001","cwd":"$SMOKE_HOME"}
{"type":"user_message","role":"user","text":"Badge smoke test"}
EOF
```

Wait ~5 s (catalog refresh interval is 2 s).

## Lanes

### 1. Baseline (no regression)

- [ ] App launches, both panels render; `horizon.log` has no errors/warnings about rendering.
- [ ] Shell panel titlebar has NO session badge (no binding, no resume session).
- [ ] Titlebars, focus ring, history meter, and close control look unchanged for the
      shell panel (compare against a pre-change screenshot if available).

### 2. Primary flow — badge appears with the bound session id

- [ ] After the binding capture, the Pi panel titlebar shows a monospace pill reading
      exactly `01a0beef` (first 8 chars of the seeded session id).
- [ ] The same 8-char id is what the "Rebind & Restart" context menu uses for other
      sessions of the same kind (open the menu: right-click the Pi panel titlebar) —
      i.e. the formats match, so a panel can be matched against the list.
- [ ] The badge is inside the titlebar, left of the history meter (and left of the
      working-dots slot); the title text stops left of the badge.

### 3. Rename interaction

- [ ] Double-click the Pi panel title → inline rename editor appears and does NOT overlap
      the badge (editor right edge is left of the badge).
- [ ] Type a new name, Enter → committed; Cancel path (Escape) also works on a second edit.
- [ ] After rename, the badge is still visible and unchanged.

### 4. Resize

- [ ] Drag the Pi panel's bottom-right resize handle to ~420 px wide, then back to 560 px.
- [ ] At every width: badge stays fully inside the titlebar, never overlaps the history
      meter / close control; title truncates with ellipsis before the badge.

### 5. Working state stability

- [ ] Trigger the agent's working indicator (send a prompt to pi, e.g. type `hi` + Enter
      in the panel; the working dots appear while the model responds).
- [ ] While working: the badge does NOT shift position (unit test
      `session_badge_rect_sits_left_of_working_slot_and_stays_stable` pins this; confirm
      visually), working dots render between the badge and the history meter.

### 6. Persistence / restart

- [ ] Quit the isolated instance (Ctrl+Shift+Q or kill the PID); confirm a runtime state
      file under `$SMOKE_HOME/.horizon/sessions/*/runtime.yaml` exists containing
      `session_binding` with the seeded session id.
- [ ] Relaunch the same isolated instance; the Pi panel restores with `resume: session`
      and the badge shows the same `01a0beef` immediately (from the explicit resume
      fallback, before any catalog capture).

### 7. Visual regression sweep

- [ ] One full-window screenshot at the end; review: no overlapping titlebar elements,
      badge styling matches the history-meter pill (subtle fill, 1 px stroke, monospace
      10.5 pt dim text), focused vs unfocused panel states both fine.

## Report format

```
SMOKE-TEST REPORT (linux/x11, debug build <sha>)
- lane 1 baseline: pass | fail — note
- lane 2 badge: pass | fail — note
- lane 3 rename: pass | fail — note
- lane 4 resize: pass | fail — note
- lane 5 working: pass | fail — note
- lane 6 persistence: pass | fail — note
- lane 7 visual: pass | fail — note
Summary: <what was fixed, what remains>
SMOKE-TEST: DONE
```

## Cleanup

```bash
kill <horizon-pid>   # also its pi/bash children
rm -rf "$SMOKE_HOME"
rm docs/testing/smoke-session-id-badge.md   # temporary plan, delete after the pass
```
