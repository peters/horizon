# Smoke-Test Plan — Browser panels on macOS (temporary)

> Validation artifact for the browser-panel PR. Delete after the macOS
> validation pass is complete.

Validate browser panels (headless Chrome over CDP) on macOS from a clean
runtime. The Linux/X11 lane is covered separately; this plan covers the
macOS runtime lane: browser **discovery from `/Applications`** (no
`browser.command` in the config), headless Chrome lifecycle, rendering,
input (including keydown delivery and key repeat), navigation, handoff,
persistence, shutdown, and the error/Retry path.

Executable by an agent that has only this file, the PR branch checkout,
and the repo's AGENTS.md.

## Prerequisites

- macOS 14 or newer (Apple Silicon or Intel), logged-in GUI session.
- Xcode Command Line Tools (`xcode-select --install`) and a stable Rust
  toolchain (CI builds this workspace; `cargo build` must succeed).
- At least one Chrome-family browser installed. Check in this order:
  ```bash
  ls "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
     "/Applications/Chromium.app/Contents/MacOS/Chromium" \
     "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge" 2>/dev/null
  ```
  If none exist: `brew install --cask chromium` (slowest, do it first).
- UI automation: `brew install cliclick` (precise mouse/keyboard).
  `screencapture` and `osascript` are built in.

## Build

```bash
git checkout <PR-branch>
cargo build          # debug binary is fine and much faster
```

## Isolated runtime (never touch the tester's real Horizon/Chrome state)

```bash
export SMOKE_HOME="$(mktemp -d /tmp/horizon-browser-mac-smoke.XXXXXX)"
mkdir -p "$SMOKE_HOME/.horizon"
cat > "$SMOKE_HOME/.horizon/config.yaml" <<'YAML'
version: 9
window:
  width: 1600
  height: 1000
workspaces:
  - name: browser-smoke
    terminals:
      - name: test-page
        kind: browser
        command: https://example.com
      - name: shell
        kind: shell
YAML
```

Launch (always with the isolated `HOME`, which `HorizonHome` honors):

```bash
HOME="$SMOKE_HOME" ./target/debug/horizon > "$SMOKE_HOME/hz.log" 2>&1 &
echo $! > "$SMOKE_HOME/horizon.pid"
```

Drive the UI scoped to that PID (several Horizon instances may be running
on the machine — never target windows by app name alone):

```bash
# window id of the instance under test
WID=$(osascript -e "tell application \"System Events\" to get id of window 1 of (first process whose unix id is $(cat $SMOKE_HOME/horizon.pid))")
screencapture -x -o "$WID" shot.png        # window screenshot
cliclick -R:$WID                            # bring to front before driving
```

If `osascript` cannot resolve the window id (sandbox/accessibility),
fall back to full-screen `screencapture -x` and note it in the report;
mouse coordinates then come from the visible window position
(`osascript -e 'tell application "System Events" to get position of window 1 of process 1'`).

## Local page for the input lane

```bash
mkdir -p /tmp/hbmac-www
cat > /tmp/hbmac-www/keytest.html <<'HTML'
<!DOCTYPE html><html><body>
<input id="in" style="width:300px;font-size:20px">
<pre id="log"></pre>
<script>
window.events = [];
const log = document.getElementById('log');
function rec(t, e){ window.events.push(t + ":" + (e.key || "")); log.textContent = window.events.join(" | "); }
for (const t of ["keydown","keyup","keypress","input"]) {
  document.getElementById('in').addEventListener(t, e => rec(t, e), true);
}
</script></body></html>
HTML
(python3 -m http.server 18099 --bind 127.0.0.1 --directory /tmp/hbmac-www &) 2>/dev/null
```

## Lanes

1. **L1 launch + browser discovery.** Launch. Expect: the `test-page`
   panel shows a starting state, then renders `https://example.com`
   (white page, "Example Domain" heading) within ~15 s — with **no**
   `browser.command` in the config (discovery from `/Applications` is the
   point of this lane). A manifest appears at
   `$SMOKE_HOME/.horizon/runtime/browsers/<local_id>.json` with non-empty
   `browser_ws`, `target_id`, `url`.
2. **L2 chrome + title.** Screenshot the panel. Expect: back/forward/
   reload buttons, URL bar showing `https://example.com/`, titlebar chip
   `WEB`, panel title from the page title ("Example Domain" or similar).
3. **L3 navigation.** Click the "Learn more" link in the page (it is the
   only link on example.com). Expect: iana.org loads, URL bar updates to
   `https://www.iana.org/`. Click the **back** button → example.com.
   Click **forward** → iana.org again. (Back/forward now go through
   `Page.getNavigationHistory`/`navigateToHistoryEntry`.)
4. **L4 keyboard (keydown delivery + repeat).** Navigate the URL bar to
   `http://127.0.0.1:18099/keytest.html` (click URL bar, select all, type,
   Enter). Click the input field in the page, then:
   - `cliclick t:"abc123"` → the input shows `abc123` and the event log
     contains `keydown:a`, `keypress:a`, `input:` for the first character
     (a plain text insertion without `keydown` is a **fail**).
   - `cliclick t:"A"` → capital `A` appended (shift path).
   - Type `xx`, then hold Backspace: `cliclick kd:53; sleep 1.5; cliclick ku:53`
     → both `x`s are deleted (held-key auto-repeat reaches the page).
5. **L5 pointer.** On iana.org: wheel-scroll down and back up (page moves
   smoothly, no teleporting). Drag-select a word (selection highlight).
   Press-drag a link **outside the panel body** and release there — then a
   normal click on a link must still work (release-outside must not strand
   Chrome's button state).
6. **L6 handoff.** While the panel shows example.com:
   ```bash
   M=$(ls "$SMOKE_HOME/.horizon/runtime/browsers/"*.json | head -1)
   python3 - "$M" <<'PY'
   import json, sys, time
   p = sys.argv[1]
   m = json.load(open(p))
   m["handoff"] = {"reason": "need human: captcha", "requested_at": int(time.time() * 1000), "done": False}
   json.dump(m, open(p, "w"), indent=2)
   PY
   ```
   Expect within ~1 s: a banner in the panel with the reason and a
   "hand back" action. Click it → the manifest's `handoff.done` becomes
   `true` and the banner clears.
7. **L7 persistence.** With the panel on iana.org, quit the app
   (close button or Cmd+Q). Relaunch with the same isolated HOME. Expect:
   the browser panel restores at `https://www.iana.org/` (its last-viewed
   URL, not about:blank) with its title, within ~15 s.
8. **L8 clean shutdown.** After the quit in L7:
   ```bash
   pgrep -fl "user-data-dir=$SMOKE_HOME" || echo "no orphan chrome"
   ```
   Expect `no orphan chrome` (the app joins driver teardown before exit).
9. **L9 error + Retry.** Relaunch, wait for example.com, then kill the
   panel's Chrome out from under it:
   ```bash
   pkill -9 -f "user-data-dir=$SMOKE_HOME/.horizon/browser-profiles"
   ```
   Expect: the panel body shows an **error message with a reason** (not a
   bare "Browser stopped") and a **visible** `Retry` button. Click Retry
   → the browser restarts and returns to the last URL; no
   "profile in use"/lock failure (restart waits for old teardown).
10. **L10 two panels.** Add a second Browser panel via the New panel
    picker (URL: `https://example.org`). Expect: both panels render
    independently (click a link in one; the other is unchanged) and
    `$SMOKE_HOME/.horizon/browser-profiles/` contains two distinct
    per-panel profile directories.

## Visual regression

Compare panel screenshots against the Linux lane: same chrome strip
(back/forward/reload + URL bar), same letterboxed page rendering, same
theme. Missing chrome or a stuck "Starting browser…" is a fail.

## Report format

```
SMOKE-TEST REPORT (macos/<arch>)
- L1 discovery+launch: pass | fail — note
- L2 chrome+title:     pass | fail — note
- L3 navigation:       pass | fail — note
- L4 keyboard:         pass | fail — note
- L5 pointer:          pass | fail — note
- L6 handoff:          pass | fail — note
- L7 persistence:      pass | fail — note
- L8 clean shutdown:   pass | fail — note
- L9 error+retry:      pass | fail — note
- L10 two panels:      pass | fail — note
Summary: <what was fixed, what remains, anything the next agent must know>
SMOKE-TEST: DONE
```

The final line must be exactly `SMOKE-TEST: DONE`.
