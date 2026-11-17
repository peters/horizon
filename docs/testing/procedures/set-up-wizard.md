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
machine section of the sidebar and the Menu offer Set up. The last tasks of the
first part use a stand-in agent to draw the four cards of the Set up mock-up, so
the new parts of a card are visible: a value at the right edge of a row, a badge
on a row, a row that cannot be chosen, a caption, a row of pills and a list of
steps.

The second part tests the agent side of the wizard with a real agent: the flow of
each goal of the skill `horizon-setup`, the two settings that only the person
confirms, and the failure card. One task set of that part needs no money: a
stand-in provider program answers the calls of the Hetzner CLI and makes
nothing. The other tasks of the second part rent a machine, and they cost what a
small machine costs while it runs.

## 2. Applicability

- Candidate: a frozen build of one exact commit, `<candidate-commit>`.
- Platforms: Linux and macOS. On macOS, use Cmd instead of Ctrl in each step.
- Watch the candidate live through a native VNC Device panel in your own
  Horizon workspace, as `AGENTS.md` requires.
- The first part of this procedure tests no agent side of the wizard. The skill
  `horizon-setup` and the cards that a real agent draws in the second part,
  tasks 6.8 to 6.12, are its own tasks.
- Task 6.8 rents no machine and reaches no provider: its provider program is a
  stand-in of this repository. Tasks 6.9 to 6.11 rent a machine at the provider
  of the person, and they cost money. Task 6.12 rents nothing.

## 3. Safety

No step of the first part of this procedure deletes data, sends a secret, changes
access or rents compute. The stand-in agent of task 6.7 draws cards only, and it
answers a press with one line of text.

Task 6.8 rents nothing: its provider program is a stand-in that makes nothing.

> **CAUTION: TASK 6.9 RENTS A MACHINE AT THE PROVIDER OF THE PERSON.** The person
> pays that provider from the press on `Rent this machine` until the delete. Use
> the smallest machine, and run task 6.9 only after the person agrees to the
> cost.

> **CAUTION: THE DELETE OF TASK 6.9 DELETES THE MACHINE AND ITS DISK.** It ends
> the cost. A machine that the delete does not reach keeps costing money: read
> the list of the provider after the delete.

Task 6.10 puts the Horizon server on a computer of the person. On this computer
it costs nothing and it changes no access. On a rented machine it costs what that
machine costs. Task 6.11 writes the browser host entry of the settings, and task
6.12 writes the server entry of the settings: both ask the person on a card of
Horizon before they write anything.

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
  `<state-tiles>` for the tile presses, an empty folder `<state-cards>` for
  the stand-in agent, and an empty folder `<state-homes>` for task 6.14 with the
  folder `<homes-bin>` for its stand-in agent program.
- The files of the stand-in agent, in the state folder of the fixture. The
  sandbox of the fixture shows the whole file system read-only, and it binds only
  these folders read-write: the state folder `<state>`, its `data/home` (the
  private home of the run), its `data/tmp` (`/tmp`) and its `data/runtime`
  (`/run/user/<uid>`). A file that a panel program reads or writes must lie in
  one of them.
- An empty folder `<state-agent>` for the second part, with the work folder
  `<work-agent>` inside it: `<work-agent>` is `<state-agent>/data/work-agent`,
  because the fixture binds the state folder and not a folder beside it.
- For tasks 6.9 to 6.12: an agent preset that the settings name first, and an
  SSH login of this computer to its own name, for the server of task 6.10.
- For task 6.9: the empty folder `<state-rent>` with its work folder
  `<work-rent>`, which is `<state-rent>/data/work-rent`, and a signed-in provider
  CLI of the person.
- For task 6.10: the empty folder `<state-server>` with its work folder
  `<work-server>`, which is `<state-server>/data/work-server`.
- For task 6.11: the empty folder `<state-browser>`, and the browser machine of
  the procedure `browser-machine.md`.
- For task 6.12: the empty folder `<state-join>`, and a Horizon server that runs
  on another computer of the person.

## 5. Setup

1. Write no panel of the stand-in agent by hand. The fixture writes it, because
   the program of an agent panel must be a script FILE: Horizon puts
   `--session-id <id>` in front of the arguments of an agent panel, so a
   `/bin/sh -c <program>` command receives it as an option of `/bin/sh` and ends
   with `Illegal option --`. A script file takes those arguments as `"$@"`.

   Task 6.7 starts the fixture with `--file-agent`. The fixture then writes four
   files into `<state-cards>/data/file-agent`: the program of the panel
   `file-agent.sh`, the two stand-in programs `file_agent.py` and
   `card_agent.py`, and the empty command file `commands.txt`. It writes the
   answers to `<state-cards>/data/file-agent/answers.json`. The JSON line that
   the fixture prints names the last two files as `file_agent.commands` and
   `file_agent.answers`.

   Result: The rule of the files is clear, and no file of the procedure lies
   outside the state folder.

2. Do not write the command file. The fixture writes it empty at every start,
   and task 6.7 appends one line per card. The answers of an earlier run stay in
   the answer file of that state folder.

   Result: `commands.txt` is empty before the first card.

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

   Result: The page says `What do you want to do?`, and the line under it is in
   the middle of the canvas: the block of the heading, the four tiles and the
   line of shortcuts stands in the middle of the height of the canvas, and the
   lower half of the canvas is not empty. Under the heading is the line that
   starts `Horizon is one canvas for your terminals`, and each of its two lines
   is centred under the heading. Four tiles are in one row:
   `Run agents side by side`, `Work on a bigger machine`, `Use Horizon on all
   your computers` and `A browser that stays signed in`. Each tile holds a small
   drawing, its title, one sentence and one line that starts with
   `Start with one agent`, `Rent a machine`, `Set up the server` or
   `Set up a browser`. Below the tiles is the line `You know Horizon already?`
   with the keys of this computer. The whole line shows: no letter of it is cut,
   and no scroll bar stands at the right edge of the last tile.

2. Move the pointer over a tile.

   Result: The outline of that tile changes to the accent colour and the pointer
   becomes a hand.

3. Push Tab four times, then Shift+Tab once.

   Result: Each tile shows a ring of the focus colour while it holds the
   keyboard. The ring moves with each press and back with Shift+Tab.

4. Push Space while the first tile holds the keyboard.

   Result: Horizon makes a workspace and opens a panel of the first agent program
   that this computer has, in the order of Settings, Presets. On a computer with
   `claude` and `codex`, it is the preset of the agent that comes first in the
   list.

   When the new workspace has no directory, the dialog that asks for one opens
   first. For an agent preset its title is `Where should the agent work?`, and
   for the shell preset it is `Select terminal directory`. The foot of the dialog
   is `enter select`, `tab complete`, `esc cancel` and
   `Type . to see hidden folders`. The list of the dialog holds no folder whose
   name starts with a dot. Type `.` at the end of the query: the list then holds
   the folders of the home that start with a dot, such as `.config` and `.codex`.
   Type the work folder of the test in full, such as `~/hz-slim`, and push Enter.

   On a computer with no agent program, Horizon opens a panel of the first
   preset of the settings, which is a terminal, and one line in the sidebar says
   `No agent program was found on this computer (<programs>). This is a
   terminal.` The line names the programs that Horizon looked for.

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

8. Make the window wide enough for four tiles, and then narrow it step by step
   to about 1 200 points of window width, where a title takes two rows.

   Result: The four tiles of a row have one height, which follows the tallest
   title and the tallest sentence of the row. The sentence of every tile keeps
   a gap of 14 points above the line of its press, so a sentence never ends on
   that line, also when its title takes two rows. The presses of the tiles of
   one row stand on one line.

9. Change the theme to the light theme in the settings.

   Result: The page, the drawings and the keys are in the light palette, and no
   text touches an edge.

10. Leave the page still for one minute without a pointer move.

   Result: The process uses no central processor time for the page. The
   `fps` counter of the titlebar says that no frame was drawn.

11. Resize the window through these sizes, in this order. Wait for one second at
    each size. At each size, count the columns of tiles, look at the picture of a
    tile, look for a scroll bar at the right edge of the page, and measure the
    width of the block: the distance from the left edge of the first tile of a
    row to the right edge of the last tile of that row.

    | Window size | Tiles | Picture of a tile | Scroll bar | Block width |
    |---|---|---|---|---|
    | 1600 x 1000 | Four in one row | 92 points high | No | 1270 points |
    | 5120 x 1348 | Four in one row | 92 points high | No | 1480 points |
    | 2560 x 1300 | Four in one row | 92 points high | No | 1480 points |
    | 1280 x 800 | Four in one row | 92 points high | No | 950 points |
    | 1000 x 700 | Two in two rows | 30 points high | No | 700 points |
    | 800 x 600 | One in one column | None | Yes | 512 points |

    Result: The page answers each size as the table says. The size of a row of
    the section **Pick up where you left off** follows the block. After each
    resize, every picture of a tile stands inside the band of its own tile: no
    picture stands on the heading, on another tile or under the sidebar. On a
    canvas that is wider than 1 480 points, the distance from the left edge of
    the canvas to the block is the same as the distance from the block to the
    right edge.

12. At the window size 800 x 600, push Page Down two times, then Page Up two
    times. Then put the pointer on the middle of the page and roll the wheel of
    the mouse three notches down, three notches up, and two notches to the side
    while you hold Ctrl.

    Result: The page moves down and up for the keys, and it moves down and up for
    the wheel of the mouse and for a scroll of a touchpad. The two side notches
    and the Ctrl notches move no pixel of the window, and the board behind the
    page never moves sideways or changes its zoom. At the largest scroll, the
    last row of the page, which starts with `You know Horizon already?`, is whole
    inside the canvas. At the top of the page, the heading is whole and no part
    of it stands under the titlebar.

13. Make the window 1600 x 1000, where the page is whole inside the canvas.
    Put the pointer on the middle of the page and roll the wheel down three
    notches.

    Result: No pixel of the window changes: a page that the window holds has
    nothing to scroll, and the board behind it does not move either.

14. At the window size 800 x 600, push Page Up until the page stops. Then push
    Tab four times.

    Result: Each press moves the focus ring to the next tile. The fourth tile
    comes whole into the canvas, and the page moves to it.

15. Make the window 1000 x 700. Look at a row of the section **Pick up where you
    left off** whose folder path is longer than the row.

    Result: The detail line of the row starts with the program of the session and
    ends with its time and its state. The middle of the path is cut with an
    ellipsis, and the start and the end of the path stay. The press of the row
    keeps its width.

16. Read the first line of that row, the title of the session, in a session whose
    prompt is longer than about 70 characters. Then make the window 5120 x 1348
    and read it again.

    Result: At 1000 x 700 the title ends with an ellipsis at the end of the room
    that the row has, and no word of it stands outside the row. At 5120 x 1348
    the row shows the whole prompt: no ellipsis stands at its end, and the row is
    filled from the left pad to its press.

17. Make a workspace with one panel, and keep the keyboard in another workspace.
    Point at `+ Panel` on the line of the first workspace.

    Result: The line of a workspace holds `Default`, `Rows`, `Cols`, `Grid`,
    `Detach` and `+ Panel`, in this order. The hover text of `+ Panel` is
    `Add a panel to this workspace`.

18. Click `+ Panel` and read the list that opens. Select `Shell`.

    Result: The list holds the presets alone, its title is `Presets`, and it
    holds no action. The shell panel opens in the workspace whose line the press
    came from. That workspace takes the keyboard, and the workspace that held it
    before has no new panel.

19. Push Ctrl+Shift+K and read the rows of the presets. On a computer without the
    program of `Gemini CLI`, read the row of that preset.

    Result: The presets whose program this computer has stand first, in the order
    of Settings, Presets. The row of an agent preset without its program stands
    after them, its label is dimmed, and the words `not installed` stand at its
    right edge. The rows of the terminal, the browser and a card hold no such
    words.

20. Select the row of `Gemini CLI` and then `Escape`.

    Result: The press opens a panel of that preset, as before the mark: the mark
    tells the person that the program is not here, and it stops nothing.

21. Make the window 800 x 600. Push Ctrl+Shift+K. Then push Escape, click
    `+ Panel` on the line of a workspace, and push Escape again.

    Result: At a window of 800 x 600 the whole card of each list stands inside
    the window. Its title, `Command Palette` or `Presets`, and its search field
    show whole at the top of the card. The list is shorter than in a large
    window, and it scrolls under the search field.

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
   `Set up needs an agent`, and the line under it says that an agent in a panel
   does the work of Set up, and that Horizon found no agent program on this
   computer. The line takes every line that its words need: no part of it ends in
   an ellipsis. The chip of the title bar says `Horizon`, because Horizon wrote
   this card itself.

3. Examine the inset box of the card.

   Result: One row for each agent program that Horizon looked for, with `found`
   or `not found` in a monospace face at the right edge. The last row is
   `Agent integrations` with `on`.

4. Examine the two buttons and the footer.

   Result: The buttons are `Look again` and `Open Settings, Presets`, and they
   stand whole above the footer. The footer says
   `Notice of Horizon, not of an agent`. The card holds no link.

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
  --state <state-cards> --file-agent --saved-session
```

Open the live view as in setup step 4. Each step appends one line to
`<state-cards>/data/file-agent/commands.txt`. The answer of each line is in
`<state-cards>/data/file-agent/answers.json`, and the panel of `File agent`
shows its short form.

1. Append this line. It draws the card of the provider choice: a row with a
   badge, a default row, and three rows that cannot be chosen.

   ```
   {"tool": "block_panel", "arguments": {"operation": "create", "title": "Rent a machine", "document": {"components": [{"type": "heading", "text": "Where do you rent?", "subtitle": "The machine is made in your own account. You pay the provider, not Horizon.", "badge": {"text": "Step 2 of 4"}}, {"type": "list", "selection": "single", "rows": [{"id": "provider.hetzner", "title": "Hetzner", "subtitle": "Signed in with hcloud", "badge": {"text": "Ready", "tone": "ok"}, "selected": true}, {"id": "provider.azure", "title": "Azure", "subtitle": "Signed in with az", "badge": {"text": "Ready", "tone": "ok"}}, {"id": "provider.gcloud", "title": "Google Cloud", "subtitle": "Not signed in. Run gcloud auth login", "disabled": true}, {"id": "provider.aws", "title": "AWS", "subtitle": "No profile found. Run aws configure sso", "disabled": true}, {"id": "provider.runpod", "title": "RunPod", "subtitle": "GPU machines. No RUNPOD_API_KEY is stored", "disabled": true}]}, {"type": "buttons", "items": [{"id": "back", "label": "Back"}, {"id": "look_again", "label": "Look again"}]}]}}, "timeout": 30}
   ```

   Result: A card appears beside the panel of `File agent`. It says
   `Where do you rent?`, with the badge `Step 2 of 4` at the right of the title
   row. The line under the title is the whole sentence
   `The machine is made in your own account. You pay the provider, not Horizon.`:
   it wraps inside the card and no word of it is replaced by an ellipsis. The row `Hetzner` is drawn as the default choice before any press, with
   the badge `Ready` in the ok tone at its right edge. The rows `Google Cloud`,
   `AWS` and `RunPod` have a dashed outline and a softer title, and their second
   lines say what is missing.

2. Press the row `Google Cloud`, then the row `AWS`.

   Result: Nothing changes on the card: the rows are not a choice. The panel of
   `File agent` gains no line, and
   `<state-cards>/data/file-agent/answers.json` gains no click.

3. Press the row `Azure`.

   Result: `Azure` takes the selected look, and the panel of `File agent`
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
   panel of `File agent` gains one line for the click with
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

   Result: Nothing changes on the card. The panel of `File agent` gains no
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

   Result: The panel of `File agent` gains one line for each close and
   stays in the workspace.

## 6.8 F08 — The rent flow with a real agent and a stand-in provider

This task set needs no money. The provider program is a stand-in: it answers the
calls of the Hetzner CLI in fixed text, it rents no machine and it reaches no
network. The agent is real, and it follows the skill `horizon-setup`.

1. Make the private `PATH` of the stand-in, with the copy of this repository:

   ```sh
   mkdir -p <work-agent>/bin
   cp <repo>/scripts/device-smoke/hcloud-standin.sh <work-agent>/bin/hcloud
   chmod 755 <work-agent>/bin/hcloud
   ```

   Result: `<work-agent>/bin/hcloud` is an executable file, and
   `<work-agent>/bin/hcloud version` prints `hcloud 1.69.0`.

2. Start the fixture with that folder in front of the `PATH` of the run.
   Step 1 made the state folder, so the run keeps it:

   ```sh
   PATH=<work-agent>/bin:$PATH python3 scripts/device-smoke/serve.py \
     --horizon <candidate> --native-view --state <state-agent> --reuse-state
   ```

   Open the live view as in setup step 4. Result: The fixture prints its JSON
   line, and Horizon runs with `<work-agent>/bin` in front of its `PATH`, so the
   agent that Set up starts finds the stand-in as `hcloud`. The fixture binds the
   state folder at its own path, so `<work-agent>/bin` is the same path inside
   the run.

3. Press `Set up` in the machine section of the sidebar.

   Result: An agent panel opens with the first message
   `Use the horizon-setup skill. The person pressed Set up. Goal: choose.` A card
   appears beside it, and the card says `What do you want to do?` with the badge
   `Step 1`. The card holds the rows `Work on a bigger machine`,
   `Use Horizon on all your computers` and `A browser that stays signed in`, and
   the line under them offers the join. The card holds no button.

4. Press the row `Work on a bigger machine`.

   Result: The same card changes to `Where do you rent?` with the badge
   `Step 2 of 4`. The row `Hetzner` carries the badge `Ready`, and the rows
   `Google Cloud`, `AWS` and `RunPod` have a dashed outline and name the command
   that signs in. The button `Look again` stands beside `Back`.

5. Press `Look again`, then press the row `Hetzner`.

   Result: The card keeps the rows of the read, and `Hetzner` takes the selected
   look. The card then changes to `How big, and where?` with the badge
   `Step 3 of 4`, the caption `SIZE`, three sizes, the caption `PLACE` and the
   pills of the places. Each size row holds `price not read` at its right edge,
   because the stand-in answers no price. The button says `Rent this machine`.

6. Press `Rent this machine`.

   Result: The card changes to `Renting the machine` with the badge `Working`, a
   bar and five steps, and the agent's panel says what it does. The step list
   moves: the machine key, the create, the wait for SSH. The card holds the
   button `Stop and delete the machine`.

7. Wait up to four minutes.

   Result: The wait for SSH fails, because no machine answers at `127.0.0.1`
   with the key that Horizon made. The card changes to
   `The step did not finish`, with the badge `Needs you`, the row `What failed`
   in one plain sentence, the row `Undone` and the row `Left`, and the buttons
   `Try again` and `Stop`. No terminal text stands on the card.

8. Press `Stop`.

   Result: The agent deletes the resources of the flow with the stand-in: the
   server, its firewall and its key. The card says that nothing of the flow is
   left, and `<work-agent>/bin/hcloud-standin.record` is gone.

9. Read `<work-agent>/bin/hcloud server list --quiet`.

   Result: The header row stands alone, so the stand-in holds no machine.

10. In the Machine section of the sidebar, read the line under `Machines`.

    Result: No machine of this task stands there, and the line `Server: not set`
    is unchanged.

## 6.9 F09 — The rent flow with a real provider, with money

> **CAUTION: THIS TASK RENTS A MACHINE AND COSTS MONEY.** Ask the person first.
> Use the smallest machine and the nearest place.

1. Repeat steps 1 to 5 of task 6.8 at the provider of the person, on
   `<state-rent>` and without the stand-in `PATH`.

   Result: The row of that provider carries the badge `Ready`, and each size row
   carries the list price that the provider CLI answered in this run, such as
   `0.0079 EUR / hour`. No row says `price not read` when the CLI answers a
   price.

2. Press `Rent this machine`.

   Result: The card changes to `Renting the machine`, and the steps finish one
   after another with their times. The machine appears in the Machines section of
   the sidebar with its provider line.

3. Press `Connect` on the card of the machine.

   Result: A terminal panel opens beside the card, with the tmux session of the
   machine. The prompt of the machine answers.

4. Press `Put the Horizon server here`.

   Result: The card changes to `Where does the server run?` of task 6.10, with
   this machine selected.

5. Press `Delete` on the card of the machine, then the confirm button.

   > **CAUTION: THIS STEP DELETES THE MACHINE AND ITS DISK.**

   Result: The card says `Deleted`, the machine leaves the Machines section, and
   the provider CLI answers an empty list for the tag `horizon-managed=true`.

## 6.10 F10 — The server flow with a real agent

This task costs nothing on this computer. On a rented machine it costs what that
machine costs.

1. Repeat step 3 of task 6.8 on `<state-server`. Press the row
   `Use Horizon on all your computers`.

   Result: The card says `Where does the server run?` with the badge
   `Step 2 of 4`, the row `This computer`, one row for each machine of the list,
   and the row `A new rented machine`. The button names the selected place, such
   as `Set up on this computer`.

2. Press `Set up on this computer`.

   Result: The card says `Setting up the server`, with the badge `Working` and
   five steps. The steps that copy the binary and install the service finish,
   with their times.

3. Read the service of the server on this computer:

   ```sh
   systemctl --user status horizon-server        # Linux
   launchctl print "gui/$(id -u)/net.horizon.server" | head -20   # macOS
   ```

   Result: The service is `active (running)` or `running`, and the server home
   `~/.horizon-server` holds `server.sqlite`, `secret.key` and `server.sock`.

4. Examine the card after the last step.

   Result: A card of Horizon appears beside the agent panel, and it says
   `Link this computer to the Horizon server on <target>?` with the two rows of
   the entry and the buttons `Link` and `Not now`.

5. Press `Not now`.

   Result: The card of Horizon goes away, and the card of the flow says that this
   computer does not follow the server yet. The file
   `~/.horizon/settings.before-server.<time>.json` does not exist.

6. Press `Add another computer`, then `Done`.

   Result: The card names Settings, Server for the QR code of the invitation and
   the list of the computers. The Set up card closes.

7. Stop the service of this computer, and remove the server home that this task
   made, when the person agrees:

   ```sh
   systemctl --user disable --now horizon-server     # Linux
   launchctl bootout "gui/$(id -u)/net.horizon.server"   # macOS
   ```

   > **CAUTION: THE SERVER HOME HOLDS EVERY MACHINE KEY AND EVERY PROVIDER VALUE
   > OF THIS COMPUTER.** Remove `~/.horizon-server` only when this test made it
   > and no computer follows it.

## 6.11 F11 — The browser flow with a real agent

1. Start the fixture on `<state-browser>` with the browser machine of this
   repository, as that task procedure says.

2. Press `Set up`, then the row `A browser that stays signed in`.

   Result: The card says `A browser that stays signed in` with the badge
   `Step 2 of 4`, the caption `BROWSER` and the two engine rows. `Chromium`
   carries the badge `Ready` and the row is selected. `Firefox` carries the
   badge `Ready` and its subtitle names Horizon and Firefox on that machine.

3. Press the row `Firefox`.

   Result: The row of `Firefox` is selected and the row of `Chromium` is not,
   and the card keeps its other lines.

4. Press the row `Chromium` again.

   Result: The row of `Chromium` is selected, so step 5 sets up Chromium.

5. Press the button that names the place, such as
   `Put the browser on helsinki-1`.

   Result: The card says `Preparing the browser` with the badge `Working` and
   five steps. The steps that copy the files and install the container finish,
   with their times.

6. Examine the card after the last step.

   Result: A card of Horizon says
   `Use the browser on <target> for new browser panels?` with the engine, the
   profile and the SSH target, and the buttons `Use it` and `Not now`.

7. Press `Use it`, then read **Settings**, **General**, **Browser host**.

   Result: The SSH target and the profile hold the values of the card. The card
   of the flow says that new browser panels use that browser now.
   `~/.horizon/settings.before-server.<time>.json` is unchanged.

8. Ask the agent to open a browser panel, and read the panel.

   Result: The panel draws its page with the browser of the machine. Horizon was
   not started again. A browser panel that was open before the press of `Use it`
   still draws with the browser of this computer.

9. Ask the agent for `browser_host_request` with `machine` set to a machine of
   the list, the profile `work` and the engine `firefox`, and press `Use it`.

   Result: The card says `Use the browser on the machine <name> for new browser
   panels?` with the rows `Browser`, `Profile`, `Machine` and `SSH from`. The
   section **Browser host** of Settings shows the machine and no SSH target.

## 6.12 F12 — The join flow with a real agent

This task rents nothing.

1. On the computer that runs the server, read the SSH name that `ssh` resolves
   there, such as `server-vm`.

2. Start the fixture on `<state-join>`. Press `Set up` in the machine section,
   then write in the agent panel: `join my server`.

   Result: The card says `Join a server that you already have` with the badge
   `Step 2 of 4`, the machines of the list as rows, the hosts of the SSH
   configuration as rows, and the line that offers to write the SSH name in the
   panel.

3. Write the SSH name in the agent panel.

   Result: A card of Horizon says
   `Link this computer to the Horizon server on <name>?` with the two rows
   `Server target` and `Command` and the buttons `Link` and `Not now`.

4. Press `Not now`.

   Result: The card of Horizon goes away. The agent's card says that nothing was
   written, and `Settings`, `Server` shows no server.

5. Write the SSH name in the agent panel again, and press `Link`.

   Result: The card of Horizon goes away, and the card of the flow says
   `The server is ready`. The sidebar line changes to `Server: in step` without a
   second start of Horizon, and the machine list of this computer shows the
   machines of the server.

6. Press `Close`.

   Result: The Set up card closes. The machine list of the server stays.

7. Take the server entry out of **Settings**, **Server**, so this task changes
   nothing that the person keeps.

## 6.13 F13 — The whole workshop in one run

This task uses the stand-in agent of task 6.7 and rents nothing: its provider
program is the stand-in of this repository, and a part that would rent draws its
price card and stops.

1. Start the fixture on `<state-workshop>` with the stand-in provider of task 6.8
   on `PATH`. Press `Set up`, and write in the agent panel:
   `set up my workshop`.

   Result: The card says `Set up my workshop` with the badge `Step 1`, one row
   for each of the three parts, a badge on each row that reads `Ready`, `Not set
   up` or `Needs you`, the caption `Nothing is rented and no setting changes
   before your press.`, and the buttons `Set up the parts that are not ready`,
   `The server only`, `The browser only`, `The machines only` and `Stop`.

2. Read the three rows of the parts.

   Result: The row of the server names the entry that `server_status` answers,
   the row of the browser names the entry that `browser_host_status` answers, and
   the row of the machines names the records of `machines` and the repositories
   that hold the section `## Horizon machine`.

3. Press `The server only`.

   Result: The card changes to the first card of Goal `server`, and the words of
   the card are the words of that goal. No second card of Set up is opened.

4. Press `Stop here` on the card between two parts.

   Result: The card says `Your workshop is ready` with the badge `Done`, the
   rows `Server`, `Browser`, `Machines` and `Cost`, one `text` line with the
   three things that a person does next, and the button `Close`. The row `Cost`
   says `nothing is rented` when the run rented nothing.

5. Press `Close`.

   Result: The card closes, and the panel of the agent says one line about the
   run.

6. Give this line to the stand-in agent of task 6.7, so the entry of the goal is
   visible without a real agent on the first card:

   ```text
   {"operation":"create","title":"Set up","document":{"components":[{"type":"heading","text":"What do you want to do?","subtitle":"Choose one. Nothing is rented and nothing is changed before you confirm.","badge":{"text":"Step 1"}},{"type":"list","selection":"single","rows":[{"id":"choose.workshop","title":"Set up my whole workshop","subtitle":"The server, the browser and the machines of your repositories, in one run.","badge":{"text":"3 parts","tone":"info"}},{"id":"choose.rent","title":"Work on a bigger machine","subtitle":"Rent a cloud machine in your own provider account."},{"id":"choose.server","title":"Use Horizon on all your computers","subtitle":"Set up the Horizon server. Machines, keys and settings follow you."},{"id":"choose.browser","title":"A browser that stays signed in","subtitle":"Chromium on a machine that stays on, for you and your agents."}]},{"type":"text","text":"Do you have a server already? Join it from this computer, and write \"join\" in this panel. You can also write to the agent in its panel at any step."}]}}
   ```

   Result: The first card of Set up lists `Set up my whole workshop` as its first
   row. The first start page still holds four tiles, and it holds no tile for
   this goal.

### 6.14 F14 — The home folders that a first start writes

This task starts no agent and rents nothing. It examines the folders that
Horizon writes for the agent programs of the computer.

1. Make `<homes-bin>` and one stand-in agent program in it:

   ```sh
   mkdir -p <homes-bin> && printf '#!/bin/sh\n' > <homes-bin>/codex && chmod 755 <homes-bin>/codex
   ```

   Result: `<homes-bin>` holds the program `codex` and no other agent program.

2. Examine the `PATH` that step 3 gives the candidate:

   ```sh
   PATH=<homes-bin>:/usr/local/bin:/usr/bin:/bin sh -c 'command -v claude codex grok pi'
   ```

   Result: The output names `<homes-bin>/codex` only. No other agent program is
   in this `PATH`.

3. Start the fixture on the empty state folder `<state-homes>` and look at the
   window of the candidate:

   ```sh
   PATH=<homes-bin>:/usr/local/bin:/usr/bin:/bin \
     python3 scripts/device-smoke/serve.py --horizon <candidate> --native-view \
     --state <state-homes>
   ```

   Result: The fixture prints one JSON line, and the first start page opens.

4. List the private home of the run:

   ```sh
   ls -A <state-homes>/data/home
   ```

   Result: The list holds `.codex` and `.local` only. It holds no `.grok`, `.pi`,
   `.gemini`, `.kilocode`, `.agents` or `.claude`, because this computer has no
   program of those folders.

5. Make a stand-in `grok`, stop the fixture, and start it again on the same state
   folder:

   ```sh
   printf '#!/bin/sh\n' > <homes-bin>/grok && chmod 755 <homes-bin>/grok
   ```

   Result: `<state-homes>/data/home/.grok` and its `skills/horizon-device` exist
   after the second start. A program that a person installs later receives its
   skills at the next start.

6. Remove `<homes-bin>/grok`, stop the fixture, and start it again on the same
   state folder.

   Result: `.grok/config.toml` and `.grok/skills/horizon-device/SKILL.md` are
   still there. The folder of the program keeps its files when the search does
   not find the program.

## 7. Pass criteria

- The first start page shows its heading, the four tiles and the row of keys,
  and each tile starts what its last line says.
- Each of the three entry points starts Set up, and the first message of the
  press names the skill `horizon-setup` and the goal.
- The notice without an agent program names the programs that Horizon looked
  for and the state of the integrations, and its two buttons work.
- The integrations off stop Set up and name the exact switch.
- A row of the pickup shows the whole stored title at the widest block of the
  page, and cuts it at the end of its own room at a narrow one.
- A second press shows the panel of Set up and starts no second panel.
- The four cards of task 6.7 draw the value, the badge, the row that cannot be
  chosen, the caption, the pills and the steps, and a press on a row that cannot
  be chosen or on a step reaches no agent.
- A real agent draws the cards of each goal in the words of task 6.8 and tasks
  6.10 to 6.12, and one card holds the flow.
- The rent flow stops at the failure card when SSH does not answer, and the card
  names what failed, what was undone and what is left.
- A price on a card is the provider's value of this run, and `price not read`
  when the CLI answers none.
- The two cards of Horizon ask the person for the server entry and for the
  browser host, they write nothing before the press, and `Not now` writes
  nothing.
- A confirmed server entry starts the link of the run, and the sidebar reads
  `Server: in step` without a second start.
- Goal `workshop` draws its three parts with their state from the read of this
  computer, runs the parts in the order of the server, the browser and the
  machines, and ends with the card of what the workshop holds, its price for each
  hour and the three things that a person does next.
- The row `choose.workshop` of the first card of Set up is the entry of that
  goal, and the first start page holds four tiles.
- The dialog of a folder names the agent of an agent preset, and it lists a
  folder that starts with a dot only when the query names one.
- The line of a workspace adds a panel to that workspace, also when another
  workspace holds the keyboard.
- A preset without its program says `not installed`, stands last, and still
  starts.
- A first start writes the folder of an agent program that the search finds, and
  no folder of a program that the computer does not have. A folder that is there
  keeps its skills.

## 8. Cleanup

1. Close the cards and the panels that you opened.

   Result: The board holds no card of this procedure.

2. Close the window of the fixture, then close the Device panel of this
   procedure.

   Result: The fixture process ends and the live view closes.

   Task 6.14 starts the fixture two more times on `<state-homes>`. Close each
   window in the same way.

3. Remove the state folders of this procedure.

   Result: The folders are gone.

4. Remove `<state-agent>`, `<state-rent>`, `<state-server>`, `<state-browser>`
   and `<state-join>` with their work folders, and remove `<state-homes>` and
   `<homes-bin>`.

   Result: The folders are gone, and no `hcloud-standin.record` is left.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
