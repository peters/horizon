---
procedure: set-up-wizard
feature: The first start page and Set up
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# First start page and Set up test procedure

## 1. Purpose

This procedure makes sure that a person who starts Horizon for the first time
sees a start page with four starts, that each tile of the page is a control, and
that Set up starts an agent panel with the goal of the press in its first
message. It also makes sure that Set up draws its own notice when no agent can
start, that the notice explains an integration switch that is off, that a second
press shows the panel of the first press and starts no second one, and that the
machine section of the sidebar and the Menu offer Set up. The last task uses a
stand-in agent to draw the four cards of the Set up mock-up, so the new parts of
a card are visible: a value at the right edge of a row, a badge on a row, a row
that cannot be chosen, a caption, a row of pills and a list of steps.

## 2. Applicability

- Candidate: a frozen build of one exact commit, `<candidate-commit>`.
- Platforms: Linux and macOS. On macOS, use Cmd instead of Ctrl in each step.
- Watch the candidate live through a native VNC Device panel in your own
  Horizon workspace, as `AGENTS.md` requires.
- This procedure does not test the agent side of the wizard. The skill
  `horizon-setup` and the cards that a real agent draws in step 6.7 are a later
  part.
- This procedure rents no machine and reaches no provider.

## 3. Safety

No step in this procedure deletes data, sends a secret, changes access or rents
compute. The stand-in agent of task 6.6 draws cards only, and it answers a press
with one line of text.

## 4. Equipment and preconditions

- A Linux computer with Xvfb, openbox, x11vnc, xdotool, ImageMagick, Python 3
  and dbus, as the
  [local device smoke fixture](../../../scripts/device-smoke/README.md) uses.
  On macOS, an isolated desktop that a native VNC Device panel can show.
- Your own Horizon, with an agent panel that can call `device_panel`.
- The candidate binary, `<candidate>`.
- The repository that built the candidate, as `<repo>`.
- An empty evidence folder `<state-fresh>` for the first start, an empty folder
  `<state-off>` for the run with the integrations off, an empty folder
  `<state-tiles>` for the tile presses, and an empty folder `<state-cards>` for
  the stand-in agent.
- An empty work folder `<work>`.

## 5. Setup

1. Write this panel list to `<work>/stand-in.json`. It gives the fixture one
   agent panel that runs the stand-in agent of this repository:

   ```json
   [
     {"name": "Stand-in agent", "kind": "claude", "command": "/bin/sh",
      "args": ["-c", "exec /usr/bin/python3 -u <repo>/scripts/device-smoke/file_agent.py \\\n  --mcp-command \"<candidate> --browser-mcp\" --commands <work>/commands.txt \\\n  --answers <work>/answers.json \"$@\"", "file-agent"],
      "cwd": "<work>", "position": [40, 620], "size": [560, 320]}
   ]
   ```

   The `/bin/sh -c` wrapper is necessary. Horizon starts an agent panel with
   `--session-id <id>` in front of the arguments of the panel and
   `--plugin-dir <path>` after them, and the program receives them as `"$@"`.
   The program takes its identity of an agent panel from the environment of
   that panel.

   Result: The file exists.

2. Write this command file to `<work>/commands.txt` with no line:

   ```sh
   : > <work>/commands.txt
   ```

   Result: The file is empty. Task 6.6 appends one line per card.

3. Start the fixture on the first empty state:

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <candidate> --native-view \
     --state <state-fresh>
   ```

   Result: The fixture prints one JSON line with `display`, `vnc_address` and
   `pids`.

4. From your agent panel, call `device_panel` with
   `{"operation":"create","endpoint":"<vnc_address>"}`. Then call it with
   `{"operation":"inspect","panel_id":"<returned id>"}`.

   Result: `connection` is `connected`, `image_received` and `image_displayed`
   are true, and `frame_sequence` moves while the desktop changes.

## 6. Tasks

### 6.1 F01 — The first start page

1. Examine the live view of the first start.

   Result: The page says `What do you want to do?`. Under it is the line that
   starts `Horizon is one canvas for your terminals`. Four tiles are in one row:
   `Run agents side by side`, `Work on a bigger machine`, `Use Horizon on all
   your computers` and `A browser that stays signed in`. Each tile holds a small
   drawing, its title, one sentence and one line that starts with
   `Start with one agent`, `Rent a machine`, `Set up the server` or
   `Set up a browser`. Below the tiles is the line `You know Horizon already?`
   with the keys of this computer.

2. Move the pointer over a tile.

   Result: The outline of that tile changes to the accent colour and the pointer
   becomes a hand.

3. Push Tab four times, then Shift+Tab once.

   Result: Each tile shows a ring of the focus colour while it holds the
   keyboard. The ring moves with each press and back with Shift+Tab.

4. Push Space while the first tile holds the keyboard.

   Result: Horizon makes a workspace and opens a panel of the first preset of
   the settings. When the workspace has no directory, the dialog that asks for
   one opens first.

5. Close that workspace's panels and the workspace. Then push Enter while the
   second tile holds the keyboard.

   Result: Set up starts, and a notice card appears, in this sandbox without an
   agent program. Task 6.4 gives the words of the notice.

6. Push Ctrl+Shift+H and then Escape. Push Ctrl+Shift+K and then Escape.

   Result: The keys of the row under the tiles are the real bindings of this
   computer: Remote Hosts opens and closes, and the command palette opens and
   closes.

7. Make the window narrow until the tiles are in two rows, then in one column.

   Result: At a canvas width under about 900 points the four tiles stand in two
   rows. Under about 520 points they stand in one column. No tile cuts its
   title or its sentence.

8. Change the theme to the light theme in the settings.

   Result: The page, the drawings and the keys are in the light palette, and no
   text touches an edge.

9. Leave the page still for one minute without a pointer move.

   Result: The process uses no central processor time for the page. The
   `fps` counter of the titlebar says that no frame was drawn.

### 6.2 F02 — The three entry points of Set up

1. Examine the machine section of the sidebar before the first machine.

   Result: The section says `Machines` and holds the control `Set up` at its
   right edge. Under it are the line `Server: not set` and the line `No machines
   yet. Set up rents one, or adds the Horizon server.`

2. Press `Set up` in the machine section.

   Result: Set up starts with no goal, so its first message names the goal
   `choose`. Without an agent program the notice of task 6.4 appears.

3. Open the Menu of the titlebar.

   Result: The Menu holds the row `Set up` above `Settings`, in a group with its
   own caption, and the row says `Rent a machine, add the Horizon server, or a
   browser that stays signed in`.

4. Press that row.

   Result: Set up starts, as in step 2.

5. Press the third tile and the fourth tile of the start page, each on a new
   empty board.

   Result: The first message of the press names the goal `server` and `browser`.
   The agent panel takes the keyboard, so typing reaches it at once.

### 6.3 F03 — The rule for the agent of Set up

The first message is one sentence: `Use the horizon-setup skill. The person
pressed Set up. Goal: <goal>.` The goal is `rent`, `server`, `browser`, `join`
or `choose`.

1. On a computer with two agent programs installed, open Settings, Presets and
   note the order of the agent presets.

2. Press `Set up` in the machine section.

   Result: The agent panel is the first preset of that order whose program this
   computer has. The program stands on the `PATH`.

3. Move another installed agent program above that preset in the list.

4. Press `Set up` again on a new empty board.

   Result: The agent panel is the preset that you moved up.

5. Add a preset of Gemini or of KiloCode above the others.

   Result: Set up does not start it. The first message of that program would be
   a word that the program reads as a task, so Set up never offers it.

### 6.4 F04 — The notice without an agent program

1. Start Horizon on an empty state on a computer without an agent program, or
   hide the agent programs from the `PATH` of the fixture.

2. Press `Set up` in the machine section.

   Result: A card panel named `Set up needs an agent` opens. Its title is
   `Set up needs an agent` and the line under it says that an agent in a panel
   does the work of Set up, and that Horizon found no agent program on this
   computer.

3. Examine the inset box of the card.

   Result: One row for each agent program that Horizon looked for, with `found`
   or `not found` in a monospace face at the right edge. The last row is
   `Agent integrations` with `on`.

4. Examine the two buttons and the footer.

   Result: The buttons are `Look again` and `Open Settings, Presets`. The footer
   says `Notice of Horizon, not of an agent`. The card holds no link.

5. Press `Open Settings, Presets`.

   Result: The Settings window opens on the Presets tab.

6. Press `Look again` on the card.

   Result: Horizon looks for the agent programs again. The card keeps one
   instance: no second notice opens, and the rows say the new answer.

7. Install an agent program, or make it visible in the `PATH` of the fixture.

8. Press `Look again` on the card.

   Result: The notice card closes and the agent panel opens with the first
   message of task 6.3.

### 6.5 F05 — The notice with the integrations off

1. Start the fixture with the switch off. The fixture gives its environment to
   the Horizon of the isolated desktop, so the switch is set where the fixture
   starts:

   ```sh
   HORIZON_AGENT_INTEGRATIONS=off python3 scripts/device-smoke/serve.py \
     --horizon <candidate> --native-view --state <state-off>
   ```

   Result: The fixture prints one JSON line with `vnc_address`.

2. Open the live view as in setup step 4, then press `Set up` in the machine
   section.

   Result: The notice card opens. The line under its title is `Agent integrations
   are off, so the agent has no Horizon skills. Start Horizon without
   HORIZON_AGENT_INTEGRATIONS=off.` The row `Agent integrations` says `off`, and
   the card lists no agent program, because Horizon looked for none.

3. Examine the line under the inset box.

   Result: The line says that the person starts Horizon without that switch and
   presses Set up again, and that Horizon installs nothing and stores no login
   of the agent.

4. Press `Look again`.

   Result: The same notice stays. Set up starts no agent while the switch is
   off, even when an agent program is on the `PATH`.

### 6.6 F06 — A second press shows the panel of Set up

1. Start a candidate on an empty state on a computer with one agent program.

2. Press `Set up` in the machine section.

   Result: The agent panel of Set up opens in the active workspace, or in a new
   workspace when the board is empty. It takes the keyboard.

3. Move the panel away from the visible part of the board, or pan the board
   until the panel is not in the view.

4. Press `Set up` in the machine section again.

   Result: The board moves to the panel of step 2. No second agent panel opens.

5. Close the agent panel of Set up, then press `Set up` again.

   Result: A new agent panel opens, because the panel of set up is gone.

### 6.7 F07 — The new parts of a card, with the stand-in agent

Start the fixture on `<state-cards>` with the panel list of setup step 1:

```sh
python3 scripts/device-smoke/serve.py --horizon <candidate> --native-view \
  --state <state-cards> --panels <work>/stand-in.json --saved-session
```

Open the live view as in setup step 4. Each step appends one line to
`<work>/commands.txt`. The answer of each line is in `<work>/answers.json`, and
the panel of `Stand-in agent` shows its short form.

1. Append this line. It draws the card of the provider choice: a row with a
   badge, a default row, and three rows that cannot be chosen.

   ```
   {"tool": "block_panel", "arguments": {"operation": "create", "title": "Rent a machine", "document": {"components": [{"type": "heading", "text": "Where do you rent?", "subtitle": "The machine is made in your own account. You pay the provider, not Horizon.", "badge": {"text": "Step 2 of 4"}}, {"type": "list", "selection": "single", "rows": [{"id": "provider.hetzner", "title": "Hetzner", "subtitle": "Signed in with hcloud", "badge": {"text": "Ready", "tone": "ok"}, "selected": true}, {"id": "provider.azure", "title": "Azure", "subtitle": "Signed in with az", "badge": {"text": "Ready", "tone": "ok"}}, {"id": "provider.gcloud", "title": "Google Cloud", "subtitle": "Not signed in. Run gcloud auth login", "disabled": true}, {"id": "provider.aws", "title": "AWS", "subtitle": "No profile found. Run aws configure sso", "disabled": true}, {"id": "provider.runpod", "title": "RunPod", "subtitle": "GPU machines. No RUNPOD_API_KEY is stored", "disabled": true}]}, {"type": "buttons", "items": [{"id": "back", "label": "Back"}, {"id": "look_again", "label": "Look again"}]}]}}, "timeout": 30}
   ```

   Result: A card appears beside the panel of `Stand-in agent`. It says
   `Where do you rent?`, with the badge `Step 2 of 4` at the right of the title
   row. The row `Hetzner` is drawn as the default choice before any press, with
   the badge `Ready` in the ok tone at its right edge. The rows `Google Cloud`,
   `AWS` and `RunPod` have a dashed outline and a softer title, and their second
   lines say what is missing.

2. Press the row `Google Cloud`, then the row `AWS`.

   Result: Nothing changes on the card: the rows are not a choice. The panel of
   `Stand-in agent` gains no line, and `<work>/answers.json` gains no click.

3. Press the row `Azure`.

   Result: `Azure` takes the selected look, and the panel of `Stand-in agent`
   gains one line for the click with `selected ["provider.azure"]`.

4. Append this line. It draws the card of the size and the place: a value at the
   right edge of each row, a caption above each group, and a row of pills.

   ```
   {"tool": "block_panel", "arguments": {"operation": "create", "title": "Choose a size", "document": {"components": [{"type": "heading", "text": "How big, and where?", "subtitle": "Hetzner. The prices are the provider's list prices.", "badge": {"text": "Step 3 of 4"}}, {"type": "caption", "text": "Size"}, {"type": "list", "selection": "single", "rows": [{"id": "size.small", "title": "Small", "subtitle": "2 vCPU, 4 GB. Agents and light builds.", "value": "0.006 EUR/h", "selected": true}, {"id": "size.medium", "title": "Medium", "subtitle": "4 vCPU, 8 GB. Builds and tests.", "value": "0.012 EUR/h"}, {"id": "size.large", "title": "Large", "subtitle": "8 vCPU, 16 GB. Several agents at once.", "value": "0.024 EUR/h"}]}, {"type": "caption", "text": "Place"}, {"type": "list", "selection": "single", "layout": "pills", "rows": [{"id": "place.helsinki", "title": "Helsinki", "selected": true}, {"id": "place.falkenstein", "title": "Falkenstein"}, {"id": "place.ashburn", "title": "Ashburn"}, {"id": "place.singapore", "title": "Singapore"}]}, {"type": "buttons", "items": [{"id": "rent", "label": "Rent this machine", "tone": "primary"}, {"id": "back2", "label": "Back"}]}]}}, "timeout": 30}
   ```

   Result: The card says `How big, and where?`. The small dim caption `SIZE`
   stands above the three sizes, and `PLACE` above the row of pills. The prices
   `0.006 EUR/h`, `0.012 EUR/h` and `0.024 EUR/h` stand at the right edge of
   their rows in the monospace face. `Small` is the default choice and
   `Helsinki` is the default pill.

5. Press `Falkenstein`.

   Result: `Falkenstein` takes the selected look and `Helsinki` loses it. The
   panel of `Stand-in agent` gains one line for the click with
   `selected ["size.small", "place.falkenstein"]`.

6. Append this line. It draws the card of the work: the steps with their states
   and their times.

   ```
   {"tool": "block_panel", "arguments": {"operation": "create", "title": "Renting the machine", "document": {"components": [{"type": "heading", "text": "Renting the machine", "subtitle": "Small, Helsinki, Hetzner. This takes about two minutes.", "badge": {"text": "Working", "tone": "warn"}}, {"type": "progress", "label": "Three of five steps", "percent": 60}, {"type": "steps", "steps": [{"label": "Make a machine key for Horizon", "state": "done", "at": "2 s"}, {"label": "Create the machine at the provider", "state": "done", "at": "11 s"}, {"label": "Wait for SSH with the machine key", "state": "done", "at": "38 s"}, {"label": "Check tmux on the machine", "state": "now", "at": "now"}, {"label": "Add the machine to Machines", "state": "waiting"}]}, {"type": "text", "text": "You can close this card. The work goes on, and the machine comes to Machines in the sidebar."}, {"type": "buttons", "items": [{"id": "stop_and_delete", "label": "Stop and delete the machine", "tone": "danger"}]}]}}, "timeout": 30}
   ```

   Result: The bar fills to 60 percent. Under it are five steps. A step that
   finished has a green disc with a check and its time at the right edge
   (`2 s`, `11 s`, `38 s`). The step that runs has an accent ring with a halo,
   the time `now`, and the foreground colour. The step that waits has an empty
   ring and no time.

7. Press the step `Check tmux on the machine`.

   Result: Nothing changes on the card. The panel of `Stand-in agent` gains no
   line.

8. Append this line. It draws the card of the machine that is ready.

   ```
   {"tool": "block_panel", "arguments": {"operation": "create", "title": "helsinki-1", "document": {"components": [{"type": "heading", "text": "helsinki-1 is ready", "subtitle": "It is in Machines in the sidebar. A click there opens a terminal.", "badge": {"text": "Running", "tone": "ok"}}, {"type": "kv", "rows": [{"label": "Provider", "value": "Hetzner, Helsinki"}, {"label": "Size", "value": "2 vCPU, 4 GB"}, {"label": "Address", "value": "203.0.113.10", "mono": true}, {"label": "Rented since", "value": "09:40", "mono": true}, {"label": "Cost", "value": "0.006 EUR/h", "mono": true}]}, {"type": "buttons", "items": [{"id": "connect", "label": "Connect", "tone": "primary"}, {"id": "put_server", "label": "Put the Horizon server here"}]}, {"type": "buttons", "items": [{"id": "add_browser", "label": "Add a browser"}, {"id": "delete", "label": "Delete", "tone": "danger"}]}]}}, "timeout": 30}
   ```

   Result: The card says `helsinki-1 is ready`, with the badge `Running` in the
   ok tone at the right of the title row. The inset box holds five pairs, and the
   address, the time and the cost are in the monospace face. The buttons
   `Connect` and `Put the Horizon server here` stand above the buttons
   `Add a browser` and `Delete`, and `Delete` is at the right edge in red.

9. Close each of the four cards.

   Result: The panel of `Stand-in agent` gains one line for each close and
   stays in the workspace.

## 7. Pass criteria

- The first start page shows its heading, the four tiles and the row of keys,
  and each tile starts what its last line says.
- Each of the three entry points starts Set up, and the first message of the
  press names the skill `horizon-setup` and the goal.
- The notice without an agent program names the programs that Horizon looked
  for and the state of the integrations, and its two buttons work.
- The integrations off stop Set up and name the exact switch.
- A second press shows the panel of Set up and starts no second panel.
- The four cards of task 6.7 draw the value, the badge, the row that cannot be
  chosen, the caption, the pills and the steps, and a press on a row that cannot
  be chosen or on a step reaches no agent.

## 8. Cleanup

1. Close the cards and the panels that you opened.

   Result: The board holds no card of this procedure.

2. Close the window of the fixture, then close the Device panel of this
   procedure.

   Result: The fixture process ends and the live view closes.

3. Remove `<work>` and the four state folders.

   Result: The folders are gone.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
