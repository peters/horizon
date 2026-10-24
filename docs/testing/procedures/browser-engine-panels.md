---
procedure: browser-engine-panels
feature: One browser panel per engine for each agent
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Browser engine panels test procedure

## 1. Purpose

This procedure tests that an agent can hold one browser panel for each browser
engine without `allow_additional`. It also tests that a second panel of the same
engine still needs `allow_additional`.

## 2. Applicability

- Use the exact candidate for the pull request.
- The steps use Linux. The rule is the same on macOS.
- This procedure does not test remote targets. A remote target still needs
  `allow_additional` when the agent holds a panel.

## 3. Equipment and preconditions

- A Linux computer with Xvfb, openbox, x11vnc, Python 3, Chromium, Firefox and
  geckodriver. If x11vnc is not installed, unpack it with
  `apt-get download x11vnc libvncserver1 libvncclient1` and `dpkg-deb -x`.
  On macOS, run the same steps in an isolated desktop session that you expose
  through VNC.
- An agent panel in your own Horizon, which can call the `device_panel` tool.
  You watch the candidate live through a native VNC Device panel.
- The steps use their own Xvfb display, not `scripts/device-smoke/serve.py`:
  Snap Chromium cannot start in the filesystem namespace of that fixture.
- The candidate `horizon` binary.
- An empty folder for the home folder of the candidate, for example
  `/home/<user>/engine-panels-home`. Snap Chromium cannot read hidden folders or `/tmp`.

## 4. Setup

1. Write this agent probe to `<work>/probe.py`. It records the identity that
   Horizon gives to an agent panel, then waits:

   ```python
   import json, os, sys, time
   keys = ["HORIZON_BROWSER_ACTOR", "HORIZON_BROWSER_HOST_INSTANCE"]
   json.dump({k: os.environ.get(k) for k in keys}, open(sys.argv[1], "w"))
   while True:
       time.sleep(3600)
   ```

   Result: The file exists.

2. Write this configuration to `<work>/config.yaml`. Use your paths. The first
   start in `<work>` imports this file into `<work>/settings.sqlite` and keeps the
   file as `config.yaml.imported-<time>`:

   ```yaml
   version: 11
   browser:
     profile_root: <home folder>/profiles
     firefox_command: /usr/lib/firefox/firefox
     geckodriver_command: <path to geckodriver>
   workspaces:
     - name: Smoke
       terminals:
         - name: Agent
           kind: codex
           command: /usr/bin/python3
           args: ["<work>/probe.py", "<work>/identity.json"]
   ```

   Result: The file exists.

3. In the repository root, start a page server:
   `python3 -m http.server 18731 --bind 127.0.0.1 --directory docs/testing/fixtures`.

   Result: `http://127.0.0.1:18731/browser-file-upload.html` returns a page.

4. Start a private display and a loopback VNC server on it:
   `Xvfb :41 -screen 0 1600x1000x24`, `DISPLAY=:41 openbox` and
   `x11vnc -display :41 -localhost -rfbport 5941 -forever -shared -viewonly -nopw`.
   Use a display number and a port that are not in use.

   Result: `127.0.0.1:5941` accepts a VNC connection.

5. From your agent panel, call `device_panel` with
   `{"operation":"create","endpoint":"127.0.0.1:5941"}`, then
   `{"operation":"inspect","panel_id":"<returned id>"}`.

   Result: `connection` is `connected`, `image_received` and `image_displayed`
   are true, and `frame_sequence` increases while the display changes. Keep
   this Device panel open and watch each task in it.

6. Start the candidate:
   `env -u WAYLAND_DISPLAY -u HORIZON -u HORIZON_BROWSER_ACTOR HOME=<home folder> DISPLAY=:41 horizon --config <work>/config.yaml --ephemeral`.

   Result: `<work>/identity.json` contains `HORIZON_BROWSER_ACTOR` and
   `HORIZON_BROWSER_HOST_INSTANCE`.

7. From a stdio MCP client, start `horizon --browser-mcp` with `HOME=<home folder>`
   and the two variables from `<work>/identity.json`. Remove all `CLAUDE_CODE_*`
   variables from its environment.

   Result: `browser_list` returns an empty list.

8. Optional: take screenshots for the record with
   `DISPLAY=:41 import -window root <file>.png`.

   Result: The files show the candidate.

## 5. Tasks

### 5.1 E01 — Open a Chromium panel

1. Call `browser_create` with `backend: chromium` and `url: http://127.0.0.1:18731/browser-file-upload.html`.

   Result: A Chromium panel opens and shows the page.

### 5.2 E02 — Open a Firefox panel beside it

1. Call `browser_create` with `backend: firefox` and the page URL. Do not set `allow_additional`.

   Result: A Firefox panel opens and shows the page. The Chromium panel stays open.

2. Call `browser_list`.

   Result: The list shows both panels. Each panel shows the agent as owner.

### 5.3 E03 — Refuse a second panel of one engine

1. Call `browser_create` with `backend: firefox`. Do not set `allow_additional`.

   Result: The server refuses the request. The message names the Firefox panel
   and `(firefox)`, and tells you to set `allow_additional` only for a user request.

2. Call `browser_create` with no `backend`. Do not set `allow_additional`.

   Result: The server refuses the request and names one of the open panels.

3. Call `browser_list`.

   Result: The list shows only the two panels from E01 and E02.

### 5.4 E04 — Refuse a concurrent create of one engine

1. Close the Firefox panel with `browser_close`.

   Result: Only the Chromium panel remains in `browser_list`.

2. Send two `browser_create` calls with `backend: firefox` and the page URL
   before you read either reply. Do not set `allow_additional`.

   Result: One call opens a Firefox panel. The other call is refused: its
   message says that a create of this agent is still pending, or names the new
   Firefox panel.

3. Call `browser_list`.

   Result: The list shows the Chromium panel and one Firefox panel.

## 6. Pass criteria

- E01 to E04 give the specified results.
- No step opens a third panel, and no step opens a second panel of one engine.

## 7. Cleanup

1. Close the two panels with `browser_close`.

   Result: The panels leave `browser_list`.

2. Close the Device panel with `device_panel` `{"operation":"close","panel_id":"<id>"}`.

   Result: The Device panel leaves your workspace.

3. Stop the MCP client, the candidate, x11vnc, openbox, Xvfb and the page server.

   Result: The processes stop.

> **CAUTION:** DELETE ONLY THE HOME FOLDER AND `<work>` THAT YOU MADE FOR THIS
> PROCEDURE. Do not delete your own home folder or a folder that another test uses.

4. Delete the home folder and `<work>`.

   Result: No browser profile from this procedure remains.

## 8. Record of results

Put the results in the pull request. Keep private evidence out of the repository.
