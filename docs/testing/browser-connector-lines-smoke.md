# Smoke test plan — browser connector lines (temporary)

> Temporary validation artifact for the browser-connector-lines PR. Delete
> after the UI validation pass is complete.

Connector lines link an agent panel (Codex/Claude/Gemini/…) to each browser
panel it currently drives. The link source is the same in-memory owner
snapshot the browser chrome chip renders: the driver refreshes
`BrowserPanelState::owner` from the manifest's live owner heartbeat (10 s
TTL) on its 250 ms signal tick, so the line and the chip cannot disagree. A
line is drawn only while the owner is set and both panels are visible in the
same viewport.

## Preconditions

- `cargo build` (debug) of the candidate commit.
- An X11/Wayland display (real desktop or `Xvfb :NN -screen 0 1900x1300x24`).
- Chromium available on `PATH` for the browser panels.
- Isolated runtime state: `export DEMO=<tmpdir>` with `HOME=$DEMO/home` and
  a temp config (never the user's real `~/.horizon`).

## Config template (`$DEMO/config.yaml`)

```yaml
version: 10
window: { width: 1900, height: 1300 }
workspaces:
  - name: Demo
    position: [60, 50]
    terminals:
      - { name: Codex A, kind: codex, position: [0, 0], size: [640, 400] }
      - { name: Codex B, kind: codex, position: [700, 0], size: [640, 400] }
      - { name: Browser 1, kind: browser, browser_url: about:blank, position: [0, 460], size: [640, 460] }
      - { name: Browser 2, kind: browser, browser_url: about:blank, position: [700, 460], size: [640, 460] }
```

Launch: `DISPLAY=:NN HOME=$DEMO/home target/debug/horizon --config $DEMO/config.yaml --new-session`

## Owner injection helper

While the MCP adapter is not driving the browser, simulate it with a 3 s
heartbeat. Read the new session's `runtime.yaml` for the panel `local_id`s
(panels are the `  - local_id:` entries), then:

```python
import fcntl, json, os, time
# pairs: (browser_panel_local_id, "horizon:<agent_panel_local_id>")
pairs = [("…", "horizon:…")]
base = os.path.expanduser("~/.horizon/runtime/browsers")
while True:
    now = int(time.time() * 1000)
    for path in os.listdir(base):
        if not path.endswith(".json"):
            continue
        full = os.path.join(base, path)
        with open(full) as f:
            m = json.load(f)
        owner = next((a for b, a in pairs if b == m.get("panel_local_id")), None)
        if not owner:
            continue
        lock = open(full + ".lock", "w")
        fcntl.flock(lock, fcntl.LOCK_EX)
        m = json.load(open(full))
        m["owner"] = {"name": owner, "updated_at": now}
        tmp = full + ".tmp"
        json.dump(m, open(tmp, "w"))
        os.replace(tmp, full)
        fcntl.flock(lock, fcntl.LOCK_UN)
    time.sleep(3)
```

## Steps

1. **Baseline (no owner).** Launch, wait for both browser panels to reach
   `about:blank`. Screenshot: **no connector lines**, no browser chrome
   `agent:` chip. Both Codex panels show no line endpoints.
2. **Line appears.** Run the helper with `Browser 1 → Codex A`. Within ~2 s
   a 1.5 px cyan line with a dot on Codex A's bottom edge and an arrowhead on
   Browser 1's top edge is visible. Browser 1's chrome shows the `agent:`
   chip. **No line** for Browser 2. Screenshot.
3. **Handoff / re-anchor.** Stop the helper, restart it with
   `Browser 1 → Codex B`. Within ~1 s (driver 250 ms signal tick) the line
   re-anchors to Codex B and no longer touches Codex A. Screenshot.
4. **Line expiry.** Stop the helper. Within ~12 s the line disappears and the
   chip clears (stale heartbeat). Screenshot.
5. **Fan-out.** Restart the helper with `Codex A → [Browser 1, Browser 2]`.
   Two lines leave Codex A (one to each browser). Codex B shows no line.
   Screenshot.
6. **Geometry variations.** Reposition (or relaunch with a config where):
   - the browser is to the right of the agent → horizontal line;
   - the browser is diagonally offset → bézier curve leaving/arriving
     perpendicular to the facing edges;
   - a third panel covers the middle of a line → the line is hidden under it
     and visible on both sides (lines render under panels);
   - the agent and browser panels are dragged/resized live → the line
     follows continuously without flicker;
   - the panels are dragged to touch (gap < 48 px) → the line is omitted;
   - the agent panel is dragged to partially overlap the browser panel →
     the line is omitted (no wrapping curve), the chip still shows the owner.
   Screenshot each.
7. **Detached workspace.** Drag the workspace to a detached window (or the
   `Detach` button). Both panels in the detached window: line renders inside
   the detached viewport. Move one panel back to the root workspace: the line
   disappears in both viewports (cross-window links are out of scope).
8. **Themes.** Relaunch with `appearance: { theme: light }` in the config and
   repeat step 2. The line is visible on the light canvas (teal-cyan).
9. **Fit/zoom.** Press the workspace `Fit` button and zoom out/in with the
   pointer: lines scale and stay attached to panel edges.
10. **Performance sanity.** With fan-out (step 5) active, move the pointer
    across the canvas: the HUD fps holds near the idle value (no per-frame
    work for unrelated panels; the owner is read from the same in-memory
    field the chrome chip uses — no disk I/O).
11. **Cleanup.** Kill the demo instance, `rm -rf $DEMO`.

## Pass criteria

- Lines appear/disappear/re-anchor strictly on the live owner heartbeat.
- Lines connect the facing edges, leave/arrive perpendicular, and track
  drag/resize live.
- No lines for unowned browsers, idle agents, or cross-viewport pairs.
- No layout, focus, or input regressions on the canvas (spot-check panel
  drag, resize handle, and double-click on empty canvas).
