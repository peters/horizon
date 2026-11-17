---
procedure: block-panel
feature: Agent cards in a Horizon workspace
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Agent card test procedure

## 1. Purpose

This procedure makes sure that an agent can draw a card in its own Horizon
workspace with the `block_panel` tool, change it, read it, hide it, show it and
close it. It also makes sure that another agent cannot change the card, that an
invalid document is refused, and that the card comes back after a restart of
Horizon. It then makes sure that a click of the person reaches the agent that
owns the card, with its selection and its time, that one wait covers every card
of an agent, and that Horizon wakes an idle agent for a click. The last task
checks the drawn look of the card in both themes, its fitted height, the
column that the cards of one agent stand in, the fold of a finished card, the
scrolling of a card that is shorter than its content, the foot that holds the
last buttons of a card that scrolls, the mark that brings the
foot of a card that the view cuts into the view, the frame that a card fits for
its own notices, the selected row of a card without its agent, and the press of a
button at a low canvas zoom. The next tasks check that an agent can open an
SSH terminal panel for the person: the checked route, each refusal, the keyboard
focus, the mark of the agent that opened it, and the mark after a restart. The
last checks of the terminals make sure that Horizon keeps one SSH key per rented
machine: the key operations and their refusals, the private files, a terminal
that connects with that key alone, the key of a workspace for every agent panel
of that workspace, and a restored terminal whose key was deleted.

The tasks B01 to B24 send their requests from a small MCP client that carries the
identity of an agent panel, so one person reads each answer of the host. The
tasks B25 to B28 and B30 use the scripted card agent instead: an agent panel that
draws its own cards, answers the clicks of the person and opens a terminal,
without an AI model. Section 6 repeats the four interactions that a real agent must pass,
with Claude Code and with a Codex panel.

## 2. Applicability

- Candidate: a frozen debug build of one exact commit, `<candidate-commit>`.
- Platforms: Linux. Watch the candidate live through a native VNC Device panel
  in your own Horizon workspace, as `AGENTS.md` requires.
- The click tasks send pointer presses to the fixture display with `xdotool`,
  scoped to the display and the process of the fixture. A pointer press on a card
  gives the keyboard focus to that card, because the card is a panel.
- The tasks B25 to B28 and B30 need the scripted card agent of the candidate's
  repository, `scripts/device-smoke/card_agent.py`, and Python 3. The script is
  deterministic: each click of the person makes one fixed change to its card, so
  a second run gives the same result.
- The task B29 needs a browser engine on the candidate's browser host, as a
  browser panel of the canvas picker does. The pages of B29 are synthetic pages
  of a loopback HTTP fixture that the task owns.

## 3. Equipment and preconditions

- A Linux computer with Xvfb, openbox, x11vnc, xdotool, ImageMagick, Python 3
  and dbus. The
  [local device smoke fixture](../../../scripts/device-smoke/README.md) uses
  these programs.
- Your own Horizon, with an agent panel that can call `device_panel` and
  `block_panel`.
- A stdio MCP client that can start `horizon --browser-mcp` and send JSON-RPC
  messages. A CLI that speaks MCP is sufficient.
- The candidate `horizon` binary, `<candidate>`.
- The scripted card agent, `<repo>/scripts/device-smoke/card_agent.py`, of the
  repository that built the candidate, with Python 3 and its standard library.
- An empty evidence folder `<state>` for the fixture state of the protocol tasks,
  an empty evidence folder `<state-agent>` for the scripted agent, an empty
  evidence folder `<state-tall>` for the tall card, an empty evidence folder
  `<state-light>` for the light theme, and an empty work folder `<work>` for the
  files of this procedure.

## 4. Setup

1. Write this probe to `<work>/probe.py`. It records the identity that Horizon
   gives to an agent panel, then waits:

   ```python
   import json, os, sys, time
   keys = ["HORIZON_BROWSER_ACTOR", "HORIZON_BROWSER_HOST_INSTANCE"]
   json.dump({k: os.environ.get(k) for k in keys}, open(sys.argv[1], "w"))
   while True:
       time.sleep(3600)
   ```

   Result: The file exists.

2. Write this panel list to `<work>/panels.json`. It gives the fixture two agent
   panels, A and B, that Horizon treats as agents of one workspace:

   ```json
   [
     {"name": "Agent A", "kind": "codex", "command": "/usr/bin/python3",
      "args": ["<work>/probe.py", "<work>/identity-a.json"], "cwd": "<work>",
      "position": [40, 620], "size": [540, 300]},
     {"name": "Agent B", "kind": "codex", "command": "/usr/bin/python3",
      "args": ["<work>/probe.py", "<work>/identity-b.json"], "cwd": "<work>",
      "position": [620, 620], "size": [540, 300]}
   ]
   ```

   Result: The file exists.

3. Start the fixture on a private desktop:

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <candidate> --native-view \
     --state <state> --panels <work>/panels.json --saved-session
   ```

   Result: The fixture prints one JSON line with `display`, `vnc_address` and
   `pids`. `<work>/identity-a.json` and `<work>/identity-b.json` hold
   `HORIZON_BROWSER_ACTOR` and `HORIZON_BROWSER_HOST_INSTANCE`.

4. From your agent panel, call `device_panel` with
   `{"operation":"create","endpoint":"<vnc_address>"}`. Then call it with
   `{"operation":"inspect","panel_id":"<returned id>"}`.

   Result: `connection` is `connected`, `image_received` and `image_displayed`
   are true, and `frame_sequence` increases while the fixture desktop changes.
   Keep this Device panel open. Each task below is visible in it.

5. From the stdio MCP client, start `horizon --browser-mcp` with
   `HOME=<state>/data/home` and the two variables from
   `<work>/identity-a.json`. Remove every `CLAUDE_CODE_*` variable from its
   environment. This is client A.

   Result: `block_panel` with `{"operation":"list"}` answers `panels` with an
   empty list.

6. Start a second stdio MCP client the same way, with the variables from
   `<work>/identity-b.json`. This is client B.

   Result: The list of client B is empty too.

## 5. Tasks

### 5.1 B01 — Create a card

1. In client A, call `block_panel` with:

   ```json
   {"operation":"create","title":"cloud-1","document":{"components":[
     {"type":"heading","text":"cloud-1"},
     {"type":"badge","text":"running","tone":"ok"},
     {"type":"kv","rows":[{"label":"Address","value":"203.0.113.10"}]}]}}
   ```

   Result: The answer has `status: created`, `panel_id` `<card>` and
   `revision: 1`. The live view shows a new panel named `cloud-1` with the
   heading, the badge and the address. The card fits its content: the footer
   sits below the address, and no part of the body is cut off. The card stands
   in the column of Agent A, at the right of the agent panel: its top edge is
   level with the top edge of the agent panel, and it covers no terminal and no
   other card. When another panel holds the place at the right of the agent
   panel, the column stands at the right of that panel.

2. Examine the fixture desktop.

   Result: The terminal cursor stays in the panel that had it. The new card does
   not take the keyboard.

### 5.2 B02 — Update the card

1. In client A, call `block_panel` with
   `{"operation":"update","panel_id":"<card>","document":{"components":[{"type":"text","text":"Hour 4 of 24."}]}}`.

   Result: The answer has `status: updated` and `revision: 2`. The live view
   shows only the new text; the old components are gone.

### 5.3 B03 — List and inspect the card

1. In client A, call `block_panel` with `{"operation":"list"}`.

   Result: One card is listed with `panel_id` `<card>`, `title` `cloud-1`,
   `revision` 2 and `owned_by_caller` true.

2. In client A, call `block_panel` with
   `{"operation":"inspect","panel_id":"<card>"}`.

   Result: The answer has `status: inspected`, `revision` 2, the document of
   B02, `visible` true, `agent_present` true and `owned_by_caller` true.

### 5.4 B04 — Hide and show the card

1. In client A, call `block_panel` with
   `{"operation":"visibility","panel_id":"<card>","visible":false}`.

   Result: The answer has `status: visibility` and `visible` false. The card
   leaves the live view.

2. In client A, call `block_panel` with `{"operation":"inspect","panel_id":"<card>"}`.

   Result: `visible` is false and `revision` is still 2.

3. In client A, call `block_panel` with
   `{"operation":"visibility","panel_id":"<card>","visible":true}`.

   Result: The card returns to the live view with the document of B02.

### 5.5 B05 — Refuse an invalid document

1. In client A, call `block_panel` with:

   ```json
   {"operation":"create","document":{"components":[
     {"type":"buttons","items":[{"id":"Not An Id","label":"Stop"}]}]}}
   ```

   Result: The answer has `status: failed`, `code: invalid_document`, and the
   message names `components[0].items[0].id`. No new panel appears in the live
   view.

2. In client A, call `block_panel` with:

   ```json
   {"operation":"create","document":{"components":[
     {"type":"list","selection":"single","rows":[
       {"id":"size.cx22","title":"CX22","selected":true},
       {"id":"size.cx32","title":"CX32","selected":true}]}]}}
   ```

   Result: The answer has `code: invalid_document`, and the message names
   `components[0].rows[1].selected`. No new panel appears in the live view.

3. In client A, call `block_panel` with a list that has `selection: none` and a
   row with `selected: true`.

   Result: The answer has `code: invalid_document`, and the message names the
   `selected` field of that row.

4. In client A, call `block_panel` with `{"operation":"list"}`.

   Result: The list still holds one card.

### 5.6 B06 — Refuse another agent

1. In client B, call `block_panel` with
   `{"operation":"update","panel_id":"<card>","document":{"components":[{"type":"text","text":"B was here."}]}}`.

   Result: The answer has `status: failed` and `code: not_owner`. The live view
   does not change.

2. In client B, call `block_panel` with
   `{"operation":"visibility","panel_id":"<card>","visible":false}` and then
   with `{"operation":"close","panel_id":"<card>"}`.

   Result: Both answers have `code: not_owner`. The card stays in the live view.

3. In client B, call `block_panel` with `{"operation":"list"}` and then with
   `{"operation":"inspect","panel_id":"<card>"}`.

   Result: Both answers report the card with `owned_by_caller` false.
   `inspect` reports `agent_present` true.

### 5.7 B07 — See the card again after a restart

1. Close the fixture Horizon normally, as the
   [local device smoke fixture](../../../scripts/device-smoke/README.md)
   describes.

   Result: The fixture stops its own processes.

2. Start the fixture again on the same state, and resume the saved board:

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <candidate> --native-view \
     --state <state> --panels <work>/panels.json --reuse-state --resume-session
   ```

   Result: The fixture prints a new `vnc_address` and new `pids`. The private
   home of the earlier run is kept.

3. From your agent panel, call `device_panel` with
   `{"operation":"create","endpoint":"<new vnc_address>"}` and inspect it.

   Result: The live view shows the workspace with the card in it.

4. Read the new identities from `<work>/identity-a.json` and start client A
   again, as in setup step 5. Then call `block_panel` with
   `{"operation":"inspect","panel_id":"<card>"}`.

   Result: The answer reports `revision` 2 and the document of B02. The card
   came back with the panel.

### 5.8 B08 — Close the card

1. In client A, call `block_panel` with
   `{"operation":"close","panel_id":"<card>"}`.

   Result: The answer has `status: closed`. The card leaves the live view.

2. In client A, call `block_panel` with `{"operation":"list"}`.

   Result: The list is empty.

3. In client A, call `block_panel` with
   `{"operation":"inspect","panel_id":"<card>"}`.

   Result: The answer has `status: failed` and `code: panel_unavailable`.

### 5.9 B09 — Read a click with next_click

1. In client A, call `block_panel` with:

   ```json
   {"operation":"update","panel_id":"<card>","document":{"components":[
     {"type":"list","selection":"single","rows":[
       {"id":"size.cx22","title":"CX22 - 2 vCPU"},
       {"id":"size.cx32","title":"CX32 - 4 vCPU"}]},
     {"type":"buttons","items":[{"id":"continue","label":"Continue","tone":"primary"}]}]}}
   ```

   Result: The answer has `status: updated` and `revision` 3. The card shows two
   rows and one button.

2. In client A, call `block_panel` with
   `{"operation":"next_click","panel_id":"<card>","timeout_millis":120000}`, and
   keep the call open.

   Result: The call does not answer yet.

3. Find the screen position of the `Continue` button on `<display>`. Take the
   picture and read the position:

   ```sh
   DISPLAY=<display> import -window root <work>/card.png
   ```

   Then press the button on the fixture display only:

   ```sh
   DISPLAY=<display> xdotool mousemove <x> <y> click 1
   ```

   Result: The call of step 2 answers `status: clicked`. `click.component_id` is
   `continue`, `click.revision` is 3, `click.selected` is empty and
   `click.stale` is false.

4. In client A, call `block_panel` with `{"operation":"list"}`.

   Result: `pending_clicks` is 0 and `listening` is false: the click was
   delivered once.

### 5.10 B10 — A selected row travels with the click

1. Press the row `CX32 - 4 vCPU` on the fixture display, as in B09 step 3.

   Result: The row is marked as selected in the live view. The press is also a
   click, so it waits for the agent.

2. In client A, call `block_panel` with
   `{"operation":"next_click","panel_id":"<card>","timeout_millis":120000}`.

   Result: The answer is `clicked` with `component_id` `size.cx32` and
   `selected` `["size.cx32"]`.

3. Press `Continue`, then call `next_click` again.

   Result: The answer has `component_id` `continue` and `selected`
   `["size.cx32"]`: the button reports which option the person chose.

4. In client A, update the card with a document that has only the row
   `size.cx22`, and then update it with the document of B09 step 1. Press
   `Continue` and call `next_click`.

   Result: `selected` is empty, because the update dropped the selection of the
   row that is gone.

### 5.11 B11 — The wait ends with timed_out

1. In client A, call `block_panel` with
   `{"operation":"next_click","panel_id":"<card>","timeout_millis":2000}` and do
   not press the card.

   Result: The call answers about two seconds later with `status: timed_out` and
   `timed_out` true. No click is lost.

2. Call `next_click` again with the same wait, then press `Continue` after five
   seconds.

   Result: The click waits on the card. The next `next_click` answers it at once.

### 5.12 B12 — The wake line reaches an idle agent

1. In client A, call `block_panel` with `{"operation":"list"}` and check that
   `listening` is false and `pending_clicks` is 0. Do not call `next_click`.

2. Press `Continue` on the fixture display.

   Result: The terminal of agent panel A shows one new line within a second:

   ```
   A button was pressed on your Horizon card. Call the block_panel tool with operation next_click to read it.
   ```

   The card shows no other text, and the line holds no text from the card.

3. In client A, call `block_panel` with `{"operation":"list"}`.

   Result: `pending_clicks` is 1 and `listening` is false.

4. In client A, call `block_panel` with
   `{"operation":"next_click","panel_id":"<card>","timeout_millis":120000}`.

   Result: The answer is `clicked`: the line and the click agree.

### 5.13 B13 — No wake line while the agent panel has focus

1. Press `Continue` on the card again within 30 seconds of the line of B12.

   Result: No new line appears in the terminal of agent panel A. The card keeps
   the click, and `list` reports `pending_clicks` 1.

2. Press in the terminal of agent panel A, so that it holds the keyboard focus.
   Wait 35 seconds without another press.

   Result: No wake line appears, because agent panel A holds the keyboard focus.
   `next_click` still answers the waiting click at once.

3. Press `Continue` again, then press on the empty canvas of the workspace, so
   that no agent panel holds the focus.

   Result: The terminal of agent panel A shows the wake line within a second.

### 5.14 B14 — A card whose agent is gone

1. Press a row of the card, then close agent panel A in the live view with its
   close button.

   Result: A yellow status frame appears at the top of the card body. Its strong
   line is `Its agent is not here` and its second line is `Start an agent in this
   workspace to use the buttons.`. The titlebar chip says `No agent` with a
   hollow dot, every badge is drawn neutral with the prefix `Last seen: `, and
   the buttons and rows are drawn disabled. The footer says `Card drawn by an
   agent`. The card takes the height of its document and the frame, so the
   button row and the footer are in view. The row that you pressed keeps the
   selected look in a dimmed form: the 2 px outline and the filled radio mark
   are dimmed, and the fill keeps a weak accent tint. The other rows keep their
   raised fill and their hollow mark.

2. Press `Continue` on the card.

   Result: Nothing is recorded. In client B, `block_panel` with
   `{"operation":"list"}` reports `pending_clicks` 0, and `inspect` reports
   `agent_present` false.

3. Start the fixture again on the same state with `--reuse-state
   --resume-session`, as in B07 step 2, and read the new identity of agent panel
   A from `<work>/identity-a.json`.

   Result: The card keeps its document and it is live again, because its owner
   is back. The card drops the room of the frame and takes the height of its
   document only, and the row keeps the selection that the person made before
   step 1.

### 5.15 B15 — A full queue of clicks

1. Press `Continue` on the fixture display 33 times:

   ```sh
   DISPLAY=<display> xdotool mousemove <x> <y> click --repeat 33 --delay 150 1
   ```

   Result: A blue status frame appears above the components. Its strong line is
   `Too many clicks are waiting` and its second line is `The agent has not read
   the clicks yet. Press again after it has.`. `list` reports `pending_clicks`
   32.

2. In client A, call `next_click` 32 times and read each answer.

   Result: The 32 oldest clicks come back in the order of the presses, each with
   a higher `event_id`, and the oldest is never lost. After the last answer the
   card no longer shows the frame and `pending_clicks` is 0.

### 5.16 B16 — The drawn look of a card

This task checks the sizes, the order and the colours of the designed card. Do
every step on the fixture display that the native VNC Device panel shows.

1. Create a card whose document holds every component:

   ```json
   {"operation": "create", "title": "build-1",
    "document": {"components": [
      {"type": "heading", "text": "build-1", "subtitle": "Hetzner · Helsinki",
       "badge": {"text": "Running", "tone": "ok"}},
      {"type": "badge", "text": "needs your choice", "tone": "info"},
      {"type": "kv", "rows": [
        {"label": "Address", "value": "203.0.113.10", "mono": true},
        {"label": "Up for", "value": "2 h 14 min"}]},
      {"type": "text", "text": "Billed from 14:02 UTC."},
      {"type": "progress", "label": "Step 1 of 3", "percent": 33, "detail": "Provider"},
      {"type": "divider"},
      {"type": "list", "selection": "single", "rows": [
        {"id": "size.cx22", "title": "CX22 - 2 vCPU", "subtitle": "0.42 EUR/h"},
        {"id": "gcloud", "title": "Google Cloud",
         "subtitle": "gcloud is not installed", "tone": "warn"}]},
      {"type": "buttons", "items": [
        {"id": "connect", "label": "Connect", "tone": "primary"},
        {"id": "stop", "label": "Stop"},
        {"id": "delete", "label": "Delete", "tone": "danger"}]}]}}
   ```

   Result: The card is 420 wide. The heading is 18 strong with a 12.5 second
   line, and the `Running` badge sits on the heading row at the right, in green
   with a dot. The label and value box is an inset surface with one 34-high pair
   per row, a thin line between the rows and the address in a monospace face.
   The progress bar shows a label on the left and `Provider` on the right. The
   `Google Cloud` second line is yellow. `Connect` is the solid accent button,
   `Stop` is a raised neutral button and `Delete` sits at the right edge in red.
   The footer shows `Card drawn by the agent <name>` on the left and
   `updated HH:MM` on the right, and the clock time is the local time of the
   create.

2. Select `Google Cloud` with one click.

   Result: The row takes a 2 px accent outline, an accent-tinted fill and a
   filled radio mark. The other row keeps its raised fill and hollow mark.

3. Read the titlebar chip while client A calls `next_click` with a wait of 60000
   milliseconds.

   Result: The chip says `<name> listens` with a filled green dot. When the wait
   ends with `timed_out` and no new wait is open, the chip says `<name>` with a
   hollow dot. Step 1 of B14 gives the third state, `No agent`.

4. Update the card with one text component, then read the footer.

   Result: The footer clock still shows `HH:MM` and not a relative time. Leave
   the card idle for one minute: the time does not change and the card does not
   redraw.

5. Set the application theme to light in Settings.

   Result: The same card is drawn on paper-white surfaces. The heading, the
   second line, the inset box, the rows, the buttons and the footer stay
   readable, and no colour is the dark-mode value. B28 checks the same card on a
   first start with `--theme light`.

6. Drag the right edge of the card until it is narrower than 420.

   Result: The heading keeps at least 60 per cent of its row and stays whole
   while it can. Where the badge would leave it less than that, the badge takes a
   row of its own under the second line and starts at the left edge of the
   heading. A heading that is still too long for its room, a long label and a
   long value wrap to the lines that they need and the pair grows with them, body
   text wraps, and nothing overlaps or leaves the card. A value of one line keeps
   the right edge of its row; a value of more lines starts at the left edge of
   its column. The buttons stay inside the card and the destructive button keeps
   the right edge.

7. Drag the bottom edge of the card up until the card is shorter than its
   content.

   Result: The rows scroll with the wheel above the button row, which stays
   above the footer under a thin line. The footer stays at the bottom of the
   card, and no row is drawn under the button row or under the footer.

8. Call `update` with a long document, then with a short one.

   Result: The card grows and shrinks to the height of its content. The footer
   clock changes, and the card does not resize again after it has settled. The
   smallest card is 140 points high and keeps the titlebar, one line of its body
   and its footer: a card that holds one heading and one button row ends right
   after that content, with no empty band above the footer.

9. Zoom the canvas out to 50 percent. In client A, call `block_panel` with
   `{"operation":"next_click","panel_id":"<card>","timeout_millis":120000}` and
   keep the call open. Then press `Delete` at the right edge of the card.

   Result: The resize handle of the card stays in the lower right corner, and
   its hit area stops above the footer strip, so no part of it covers the button
   row. The card does not resize, and the call answers `clicked` with
   `component_id` `delete`.

10. Press the handle in the lower right corner of the card and drag it, then zoom
    the canvas back to 100 percent and set the theme back to dark.

   Result: The handle takes the drag below the button row, and the card keeps
   the size that you dragged.

### 5.17 B17 — One wait covers every card of an agent

1. In client A, create a second card with title `cloud-2` and one `Continue`
   button.

   Result: The answer has `status: created` and `revision: 1`. The second card
   stands at the top of the column of Agent A, the first card stands under it,
   and no card covers a panel.

2. In client A, call `block_panel` with
   `{"operation":"next_click","timeout_millis":120000}`, without `panel_id`, and
   keep the call open.

   Result: The call does not answer yet. `list` reports `listening` true for
   both cards of client A.

3. Press the `Continue` button on the first card on the fixture display.

   Result: The call answers `status: clicked`, and `click.panel_id` is the id of
   the first card. `list` reports `listening` false and `pending_clicks` 0 for
   both cards.

4. In client A, call `next_click` without `panel_id` with a wait of 60000
   milliseconds, then call it again the same way.

   Result: The older call answers `timed_out` true first, and the newer call
   stays open.

5. Close both cards of client A with `close`. Then call `next_click` without
   `panel_id`.

   Result: The answer has `code: no_cards`, at once.

6. In client B, call `next_click` without `panel_id` while client A owns a card.

   Result: The answer has `code: no_cards`: a wait covers only the cards of the
   caller.

### 5.18 B18 — A row starts selected

1. In client A, create a card with:

   ```json
   {"operation":"create","title":"choice","document":{"components":[
     {"type":"list","selection":"single","rows":[
       {"id":"size.cx22","title":"CX22 - 2 vCPU"},
       {"id":"size.cx32","title":"CX32 - 4 vCPU","selected":true}]},
     {"type":"buttons","items":[{"id":"continue","label":"Continue","tone":"primary"}]}]}}
   ```

   Result: The card opens with `CX32 - 4 vCPU` drawn as the selected row: the
   accent outline, the tinted fill and the filled radio mark.

2. Press `Continue` without touching a row, then call `next_click` with the
   `panel_id` of the card.

   Result: The answer is `clicked` with `selected` `["size.cx32"]`: the default
   row travels with the click.

3. Press the row `CX22 - 2 vCPU`, then call `next_click` with the `panel_id` of
   the card.

   Result: The answer has `component_id` `size.cx22` and `selected`
   `["size.cx22"]`: the person's row wins over the default.

4. Update the card with a document whose list keeps only `size.cx32`, without
   `selected`, then update it with the document of step 1 and press `Continue`.

   Result: The first update drops the selection of the row that is gone, and the
   second update selects `size.cx32` again, so `selected` is `["size.cx32"]`.

### 5.19 B19 — Open an SSH terminal for the person

No SSH server is necessary for this task. Use a loopback address with a port
where no server listens: `ssh` fails to connect at once, and the panel stays.

1. Put the keyboard focus in the terminal of Agent A. Then, in client A, call
   `block_panel` with:

   ```json
   {"operation":"open_terminal","ssh":{"host":"127.0.0.1","port":2222},
    "remote_command":"tmux new -A -s build"}
   ```

   Result: The answer has `status: opened` and `panel_id` `<terminal>`. The live
   view shows a new SSH panel named `127.0.0.1` beside the panel of Agent A. The
   panel says that `ssh` could not connect, and the answer is not a refusal.

2. Examine the keyboard focus on the fixture desktop.

   Result: The cursor is still in the terminal of Agent A. The new panel did not
   take the keyboard.

3. Find the `ssh` child of the fixture Horizon process, and read its command
   line:

   ```sh
   ps -o pid=,args= -C ssh
   ```

   Result: The line whose parent is the fixture Horizon PID holds `-p 2222`,
   `-t`, `127.0.0.1` and `tmux new -A -s build` as its last argument. The line
   holds no other `-o` option than the two keepalive options and no `-i`, `-J`
   or `-L`.

4. In client A, call `block_panel` with
   `{"operation":"open_terminal","ssh":{"host":"127.0.0.1","user":"deploy"},"title":"build box"}`.

   Result: The answer has `status: opened`. The new panel is named `build box`.
   Its `ssh` line holds no `-t`, because the request has no remote command.

5. In client A, call `block_panel` with `{"operation":"list"}`.

   Result: The list holds only cards. The two terminals are not in it.

### 5.20 B20 — Refuse a route or a command that is not plain

1. In client A, call `block_panel` with each of these requests:

   ```json
   {"operation":"open_terminal","ssh":{"host":"-oProxyCommand=id"}}
   {"operation":"open_terminal","ssh":{"host":"lab 1"}}
   {"operation":"open_terminal","ssh":{"host":"127.0.0.1","user":"-L2222"}}
   {"operation":"open_terminal","ssh":{"host":"127.0.0.1","port":0}}
   {"operation":"open_terminal","ssh":{"host":"127.0.0.1","remote_command":"tmux new; id"}}
   {"operation":"open_terminal","ssh":{"host":"127.0.0.1"},"remote_command":"-oProxyCommand=id"}
   {"operation":"open_terminal","ssh":{"host":"127.0.0.1"},"remote_command":"echo $HOME"}
   ```

   Result: Each of the first four answers `status: failed` with
   `code: invalid_ssh_route`, and the message names `ssh.host`, `ssh.user` or
   `ssh.port`. Each of the last three answers `code: invalid_remote_command`,
   and the message names `remote_command`.

2. Examine the live view.

   Result: The workspace holds the two terminals of B19 and no other panel. A
   refused request opens no panel.

3. In client A, call `block_panel` with
   `{"operation":"open_terminal","ssh":{"host":"127.0.0.1","identity_file":"/home/peters/.ssh/id_ed25519"}}`.

   Result: The call fails before Horizon reads it: the `ssh` object has no such
   field.

### 5.21 B21 — The mark of the agent that opened the terminal

1. Examine the titlebar of the first terminal of B19 in the live view.

   Result: A chip at the right of the title says `opened by Codex`, because the
   fixture gives Agent A the kind `codex`.

2. Put the keyboard focus in the terminal, type `echo hi` and press Enter.

   Result: The text reaches the terminal. The chip stays.

3. Open a terminal of the person from the sidebar picker.

   Result: Its titlebar holds no such chip.

4. Close the terminal that holds the mark from its titlebar.

   Result: The panel leaves the workspace. No answer of `block_panel` names it
   again.

### 5.22 B22 — See the marked terminal again after a restart

1. Close the fixture Horizon normally, then start it again on the same state
   with `--reuse-state --resume-session`, as in B07.

2. From your agent panel, call `device_panel` with
   `{"operation":"create","endpoint":"<new vnc_address>"}` and inspect it.

   Result: The live view shows the workspace with the remaining terminal in it.

3. Examine the titlebar of that terminal.

   Result: The chip still says `opened by Codex`. The mark came back with the
   saved panel.

### 5.23 B23 — Keep a key for a rented machine

No SSH server is necessary for this task. The fixture keeps its private home at
`<state>/data/home`, so its Horizon home is `<state>/data/home/.horizon`.

1. In client A, call `block_panel` with
   `{"operation":"machine_key","name":"build-1"}`.

   Result: The answer has `status: machine_key`, `name: build-1` and
   `created: true`. `public_key` is one line that starts with `ssh-ed25519 ` and
   ends with `horizon-machine-build-1`. The answer holds no line that starts
   with `-----BEGIN`.

2. Call `block_panel` with `{"operation":"machine_key","name":"build-1"}` again.

   Result: The answer has `created: false` and the same `public_key`.

3. Call `block_panel` with `{"operation":"machine_keys"}`.

   Result: `keys` holds one entry for `build-1`, with `name`, `public_key` and
   `created_at_millis`.

4. Examine the files of the key:

   ```sh
   find <state>/data/home/.horizon/machine-keys -maxdepth 1 -printf '%M %p\n'
   ```

   Result: `machine-keys` is `drwx------`, because it holds the private half.
   `build-1` is `-rw-------`. `build-1.pub` holds the `public_key` of step 1.

5. In client A, call `block_panel` with `{"operation":"machine_key","name":"../id_ed25519"}`,
   with `{"operation":"machine_key","name":"Build"}`, and with a name of 41
   characters.

   Result: Each call answers `status: failed` with `code: invalid_key_name`, and
   the message names the rule. No file appears outside the key directory.

6. Call `block_panel` with
   `{"operation":"delete_machine_key","name":"build-1"}`, then with
   `{"operation":"machine_keys"}`, then with
   `{"operation":"delete_machine_key","name":"build-1"}` again.

   Result: The first answer has `status: deleted_machine_key` and `name: build-1`.
   The list is empty and the key files are gone. The last answer is
   `code: key_unavailable`.

### 5.24 B24 — Connect with a machine key

1. In client A, call `block_panel` with
   `{"operation":"machine_key","name":"build-2"}`.

   Result: The answer has `created: true` and one `public_key`.

2. Call `block_panel` with:

   ```json
   {"operation":"open_terminal","key":"build-2","ssh":{"host":"127.0.0.1","port":2222},
    "remote_command":"tmux new -A -s build"}
   ```

   Result: The answer has `status: opened` and a `panel_id`. The live view shows
   a new SSH panel on port 2222 beside the panel of Agent A.

3. Read the command line of the `ssh` child of the fixture Horizon process:

   ```sh
   ps -o pid=,args= -C ssh
   ```

   Result: The line holds `-i <state>/data/home/.horizon/machine-keys/build-2`,
   `-o IdentitiesOnly=yes`, `-o IdentityAgent=none`,
   `-o UserKnownHostsFile=<state>/data/home/.horizon/machine-keys/build-2.known_hosts`,
   `-o StrictHostKeyChecking=accept-new`, `-t`, `127.0.0.1`, and
   `tmux new -A -s build` last. It holds no other `-i`, no `-J` and no `-L`. No
   server answers on the port, so `build-2.known_hosts` is not there yet: the
   file appears at the first contact with a host.

4. In client A, call `block_panel` with
   `{"operation":"open_terminal","key":"build-9","ssh":{"host":"127.0.0.1","port":2223}}`.

   Result: The answer is `code: key_unavailable`. No panel is opened.

5. In client B, call `block_panel` with the request of step 2.

   Result: The answer has `status: opened`. Every agent panel of the workspace
   uses the keys of that workspace.

6. Call `block_panel` with
   `{"operation":"delete_machine_key","name":"build-2"}`, then close the fixture
   Horizon normally and start it again with `--reuse-state --resume-session`, as
   in B22.

   Result: The two terminals come back with the mark of their agent. The `ssh`
   line of each still holds the `-i` path of `build-2`, and the terminal shows
   the warning of `ssh` that the identity file is not accessible. The panel
   starts, because Horizon does not stop a restore for a missing key.

### 5.25 B25 — The scripted agent draws and answers its own cards

The tasks from here use the scripted card agent of this repository instead of a
client with the identity of a probe panel. The agent panel draws the cards of its
scenario, reads the clicks of the person and answers them. Press the buttons on
the fixture desktop as in B09 step 3, or through the Interact toggle of the
Device panel.

1. Write this panel list to `<work>/agent-panels.json`. It gives the fixture
   three scripted agent panels, each with one scenario:

   ```json
   [
     {"name": "Scripted machine", "kind": "claude", "command": "/bin/sh",
      "args": ["-c", "CARD_AGENT_SSH_HOST=smoke.invalid exec /usr/bin/python3 -u <repo>/scripts/device-smoke/card_agent.py \\\n  --mcp-command \"<candidate> --browser-mcp\" --scenario machine \"$@\"", "card-agent"],
      "cwd": "<work>", "position": [1250, 60], "size": [330, 400]},
     {"name": "Scripted browser", "kind": "claude", "command": "/bin/sh",
      "args": ["-c", "exec /usr/bin/python3 -u <repo>/scripts/device-smoke/card_agent.py \\\n  --mcp-command \"<candidate> --browser-mcp\" --scenario browser \"$@\"", "card-agent"],
      "cwd": "<work>", "position": [40, 620], "size": [330, 400]},
     {"name": "Intruder", "kind": "claude", "command": "/bin/sh",
      "args": ["-c", "exec /usr/bin/python3 -u <repo>/scripts/device-smoke/card_agent.py \\\n  --mcp-command \"<candidate> --browser-mcp\" --scenario intruder \"$@\"", "card-agent"],
      "cwd": "<work>", "position": [1250, 620], "size": [330, 300]}
   ]
   ```

   The `/bin/sh -c` wrapper is necessary. Horizon starts an agent panel with
   `--session-id <id>` in front of the arguments of the panel and `--plugin-dir
   <path>` after them, and the script receives them as `"$@"`. Only the wrapper of
   `Scripted machine` names an SSH host. The name `smoke.invalid` is a reserved
   name that no resolver answers, so the Connect task of B26 needs no SSH server
   and reaches no machine. The other two panels have no host to open.

   Result: The file exists.

2. Start the fixture on a new private state with a wide desktop:

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <candidate> --native-view \
     --state <state-agent> --screen 2400x1400 \
     --panels <work>/agent-panels.json --saved-session
   ```

   Result: The fixture prints one JSON line with `display`, `vnc_address` and
   `pids`. The workspace holds the two terminals of the fixture, the three agent
   panels and two cards.

3. From your agent panel, call `device_panel` with
   `{"operation":"create","endpoint":"<vnc_address>"}` and inspect it.

   Result: The live view shows the machine card at the right edge of `Scripted
   machine` with level top edges, and the browser card at the right edge of
   `Scripted browser`. No card covers another panel.

4. Read the terminal of `Scripted machine`.

   Result: It holds these lines, in this order:

   ```
   card-agent: machine: created card <card> rev 1 (Machine · build-1)
   card-agent: documents use the rich model of the card document
   card-agent: listening continuous on every card with a 60 s wait
   ```

   No line holds a spinner glyph or one of the working words that Horizon reads
   as a working agent.

5. Read the machine card in the live view.

   Result: The heading is `build-1` with the second line `Hetzner · Helsinki` and
   the `Running` badge at the right of the heading row. The inset box holds four
   pairs, and the address and the session are in a monospace face. The button row
   is `Connect`, `Stop` and `Delete`, with `Delete` at the right edge in red. The
   footer says `Card drawn by the agent Claude` on the left and `updated HH:MM` on
   the right.

6. Press `Stop` on the machine card.

   Result: The badge says `Stopped` in the warn tone, `Start` takes the place of
   `Stop`, and a text says that the machine is stopped. The terminal of `Scripted
   machine` gains one line:

   ```
   card-agent: machine: click stop rev <revision> stale false selected []
   ```

   The card keeps its place, and the clock of the footer changes.

7. Press `Start`.

   Result: The card returns to the running form of step 5, and the terminal gains
   the line `card-agent: machine: click start rev <revision> stale false
   selected []`.

8. Press `Delete` on the machine card.

   Result: The card leaves the live view. The terminal gains its `click delete`
   line, and then `card-agent: machine: closed card <card>`.

9. Press the row `Chromium in Docker` on the browser card, then press `Set it
   up`.

   Result: The row takes the selected look, and the terminal of `Scripted
   browser` gains `card-agent: browser: row chromium-docker selected; the button
   below decides`. The card says `You picked Chromium in Docker. I am setting it
   up.`, the progress bar says `Step 3 of 4`, and the terminal gains `card-agent:
   browser: setup with Chromium in Docker`.

10. Drag the right edge of the browser card until it is narrower than 360.

    Result: The heading `home-mac` stays whole, and the badge `Needs your choice`
    takes a row of its own under the second line, starting at the left edge of the
    heading.

11. Press `Cancel` on the browser card.

    Result: The card closes. The terminal of `Scripted browser` gains `card-agent:
    browser: click cancel rev <revision> ...` and then `card-agent: every card is
    closed; exit`. The agent panel stays in the workspace with that output.

### 5.26 B26 — The Connect button of the scripted agent opens a terminal

1. Press `Connect` on the machine card of B25.

   Result: The card keeps its other components, and a text says `A terminal
   opened beside this panel at HH:MM:SS.`. The terminal of `Scripted machine`
   gains the line `card-agent: machine: connect: opened terminal <terminal> on
   smoke.invalid at HH:MM:SS (build-1 session)`.

2. Examine the live view.

   Result: A new SSH panel named `build-1 session` stands beside the `Scripted
   machine` panel: at its right edge with level top edges, or below it with level
   left edges when the place beside is taken. Its titlebar holds the chip `opened
   by Claude`. The panel says that `ssh` could not resolve the host name.

3. Examine the keyboard focus in the live view.

   Result: The focus stays on the card that you pressed. The new terminal did not
   take the keyboard from it.

### 5.27 B27 — Another agent cannot change the cards of the scripted agent

1. Read the terminal of `Intruder`. It ran its check when the fixture started, so
   its output stays in the scrollback.

   Result: It ends with `card-agent: intruder check done` and holds these lines:

   ```
   card-agent: intruder: list answered <n> card(s), <m> of them of another agent
   card-agent: intruder: card <card> (<title>) is owned by another agent
   card-agent: intruder: card <card> inspect answered inspected
   card-agent: intruder: card <card> update answered not_owner
   card-agent: intruder: card <card> close answered not_owner
   card-agent: intruder: the wait on every card answered no_cards
   card-agent: intruder check done
   ```

   When the card of the other agent was not there at its first look, the
   intruder prints `card-agent: intruder: list answered 0 card(s) and none is
   foreign; look again in 5 s (<n> of 12)` first, and looks again.

   The cards of the scripted agents were still in the live view after the check,
   with the documents that their own agents drew.

### 5.28 B28 — The light theme draws the same cards on light surfaces

The theme of the fixture is part of the configuration, and the first start of a
state path imports it. A later run in the same private home keeps the theme of
its settings store, so this task needs a new state path.

1. Start the fixture on a new state, with the light theme and the same panel
   list:

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <candidate> --native-view \
     --state <state-light> --screen 2400x1400 --theme light \
     --panels <work>/agent-panels.json --saved-session
   ```

   Result: The fixture prints one JSON line with `display`, `vnc_address` and
   `pids`.

2. From your agent panel, call `device_panel` with
   `{"operation":"create","endpoint":"<vnc_address>"}` and inspect it.

   Result: The workspace and the cards are drawn on paper-white surfaces. The
   heading, the second line, the badges, the inset box, the rows, the buttons and
   the footer stay readable, and no colour is a dark-mode value.

3. Press `Connect` on the machine card.

   Result: The card says `No SSH host is set in CARD_AGENT_SSH_HOST, so no
   terminal was opened.`, and no terminal panel appears. This panel list gives
   the machine panel a host in the run of B25 and no host in this run.

4. Close the fixture Horizon normally. Then start the fixture again on the same
   state with the dark theme:

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <candidate> --native-view \
     --state <state-light> --screen 2400x1400 --theme dark \
     --panels <work>/agent-panels.json --reuse-state --resume-session
   ```

   Result: The candidate starts on the board that the earlier run saved. The
   cards and the workspace stay light, because the settings store of
   `<state-light>` holds the theme that its first start imported.

### 5.29 B29 — A link on a card opens its page in a browser panel

CAUTION: This task starts a network server on the loopback interface of this
computer. Use a synthetic page on a loopback port that no other task uses, and
stop the server in step 8.

1. Make a synthetic page in `<work>/link/page.html` with the text
   `Synthetic session page`, and start a loopback server for it:

   ```sh
   cd <work>/link && python3 -m http.server 18099 --bind 127.0.0.1
   ```

2. In client A, call `block_panel` with a link to that page:

   ```json
   {"operation":"update","panel_id":"<card>","document":{"components":[
     {"type":"heading","text":"cloud-1"},
     {"type":"link","label":"Open session","url":"http://127.0.0.1:18099/page.html"}]}}
   ```

   Result: The card draws `Open session` in the accent colour with a line under
   it, and `127.0.0.1:18099` under it in low-emphasis text. No browser panel
   appears, and the server writes no request to its log: Horizon fetches
   nothing before a press.

3. Move the pointer over the label.

   Result: The pointer becomes a pointing hand, and the hover text shows the
   whole URL and `Opens in a browser panel` on a second line.

4. Press `Open session` once.

   Result: Exactly one browser panel opens in the workspace of the card, beside
   the card, and it loads the page `Synthetic session page`. The card does not
   report a click, and a `next_click` of client A stays open or answers
   `timed_out`; `inspect` shows the same `pending_clicks` as before the press.

5. Close the agent panel of client A in the live view, and press the link again.

   Result: The card says that its agent is not here, its buttons and rows are
   drawn disabled, and the link stays live: a second browser panel opens the same
   page.

6. In client A, call `block_panel` with a document whose link has credentials:

   ```json
   {"operation":"update","panel_id":"<card>","document":{"components":[
     {"type":"link","label":"Open session","url":"https://user:pass@example.test/page"}]}}
   ```

   Result: The answer is a failure with code `invalid_document`, and the message
   names `components[0].url`. The message does not repeat the address. The card
   keeps the document of the last accepted revision.

7. Call `block_panel` with nine `link` components.

   Result: The answer is `invalid_document`, and the message names
   `components[8]` and the limit of 8 links.

8. Set `browser.command` in the settings to a program that does not exist, and
   press `Open session` again.

   Result: No browser panel stays on the board. Two lines say why, and both use
   the words of the engine: the sidebar notice reads `Browser panel: <reason>`
   for ten seconds, and the card draws `Could not open: <reason>` in the danger
   colour under the host of the address. The log holds both lines. A browser
   panel of this computer that ends with an error after it showed a page says the
   same reason in the same notice, and it goes the same way. A panel of a browser
   host does not: it stays, and it says why in its own body (see the
   [remote browser tunnel](remote-browser-tunnel.md) procedure).

9. Call `block_panel` again with the document of step 2.

   Result: The line under the link is gone, because a new document replaced the
   one that the person pressed.

10. Set `browser.command` back, press `Open session`, and stop the loopback
    server. Close the browser panels that the task opened.

   Result: The line under the link is gone after the press that opened the page,
   and the server exits.

### 5.30 B30 — The mark brings the foot of a card that the view cuts

The document of the scenario `tall` needs more room than the ceiling of a card
gives it. The ceiling is the height of the canvas of the window, not less than
560 points and not more than 900 points. The card rests at the ceiling, and the
view of a window of 900 points cuts the foot of a card that starts at 620
points.

1. Write this panel list to `<work>/tall-panels.json`:

   ```json
   [
     {"name": "Scripted tall card", "kind": "claude", "command": "/bin/sh",
      "args": ["-c", "exec /usr/bin/python3 -u <repo>/scripts/device-smoke/card_agent.py \\\n  --mcp-command \"<candidate> --browser-mcp\" --scenario tall \"$@\"", "card-agent"],
      "cwd": "<work>", "position": [80, 620], "size": [330, 400]}
   ]
   ```

2. Start the fixture on a new private state and on a screen of 1600 by 900:

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <candidate> --native-view \
     --state <state-tall> --screen 1600x900 \
     --panels <work>/tall-panels.json --saved-session
   ```

   Create its Device panel as in B25 step 3.

   Result: The live view shows the card `Overflow check` beside the panel
   `Scripted tall card`. The card stands at the ceiling of its height, the body
   holds more than it shows, and the mark `More below` stands at the lower edge
   of the view above the footer. The last button row of the card is under the
   lower edge of the view.

3. Press the mark `More below` again and again.

   Result: Each press that finds more of the body under the lower edge of the
   view takes one page of the scroll, and the mark stays. The press that finds
   the end of the body moves the card up: the last button row (`OK`, `Close`),
   the footer and the lower edge of the card stand in the view, 40 points above
   its lower edge, and the heading row may leave the top of it. The zoom of the
   canvas does not change. No mark stays.

4. Press the place where the mark stood.

   Result: The press reaches the canvas, which now shows the room under the foot
   of the card. No mark appears and no part of the card changes.

### 5.31 B31 — The cards of an agent stand in one column

1. In client A, create three cards with the titles `job-1`, `job-2` and `job-3`,
   in this order. Give `job-2` a `kv` component of six rows, and give the other
   two cards one `heading` and one `text` component.

   Result: The three cards stand in one column at the right of the panel of
   Agent A. `job-3` is at the top, level with the top edge of the agent panel,
   `job-2` is under it, and `job-1` is under `job-2`. The gap between two cards
   is the gap between two tiles. No card covers a panel. A line goes from each
   card to the agent panel.

2. In client A, close `job-2`.

   Result: `job-1` moves up and stands directly under `job-3`. The column has no
   hole.

3. In a workspace with no preset selected, drag the title bar of the panel of
   Agent A by about 200 points.

   Result: The agent panel moves, and its column moves with it: `job-3` is level
   with the top edge of the agent panel again, at its right.

4. Drag the title bar of `job-1` to an empty place under the agent panel.

   Result: `job-1` stays where you put it. The preset of the workspace, if one
   is selected, stays selected.

5. In client A, create a card with the title `job-4`.

   Result: `job-4` stands at the top of the column and `job-3` stands under it.
   `job-1` does not move.

6. Do the restart of B07 steps 1 to 3.

   Result: Each card stands where it stood before the restart. `job-1` is not
   in the column.

### 5.32 B32 — A card has the height of its content

1. In client A, create a card whose document holds one `heading` and one `text`
   of two lines. Then create a card whose document holds one `heading`, a `kv`
   component of eight rows and one `buttons` component.

   Result: The first card is lower than the second card. Each card ends directly
   under its content, and no card shows the mark `More below`. This is the result
   in a workspace with a preset selected too: a card of an agent is no tile of
   the preset.

2. In client A, update the upper card of the column with a document that holds
   four more rows.

   Result: The card is higher by the height of four rows. Each card under it
   moves down by the same height.

3. In client A, create a card whose document holds one `heading`, a `kv`
   component of 24 rows and one `buttons` component with the button `Delete`.

   Result: The card is as high as the canvas of the window, not less than 560
   points and not more than 900 points: 560 points in a window of 800 x 600, and
   900 points in a window that is 1000 points high or more. Its body shows the
   mark `More below`. The button `Delete` stands at the foot of the card, under
   the mark and above the footer.

4. Change the height of the window, and look at the card again.

   Result: The card keeps its height. A change of the window size changes no
   card. The next document of the card takes the ceiling of the window of that
   moment.

5. Put the pointer on the body of that card and turn the wheel down.

   Result: The rows scroll above the button `Delete`. The button keeps its
   place and takes a press.

6. Drag the corner grip of that card until the card is 300 points high. Then
   update the card with a document of ten rows.

   Result: The card keeps the size that you dragged.

### 5.33 B33 — A finished card folds to one line

1. In client A, create a card with this document:

   ```json
   {"finished": true, "components": [
     {"type": "heading", "text": "Server ready", "badge": {"text": "Done", "tone": "ok"}},
     {"type": "text", "text": "The server answers on its port."},
     {"type": "buttons", "items": [{"id": "again", "label": "Run again"}]}]}
   ```

   Result: The card is one line at the width of its column. The line shows
   `Server ready` and `Done`. It shows no text of the body and no button. The
   card under it stands directly under the line.

2. Press the line.

   Result: The card opens. It shows the text and the button `Run again`.

3. Press `Run again`, and call `next_click` in client A.

   Result: The answer is `clicked` with `component_id` `again`. An open card
   works as every card.

4. In client A, update the card with the same document.

   Result: The card stays open. An update folds no card that the person opened
   after it was finished.

5. Press the arrow in the title bar of the card, left of its close control.

   Result: The card folds to its one line. A second press on the arrow opens it.

6. In client A, create a card without the field `finished`, and update it two
   times without the field.

   Result: The card stays open after each update. Horizon folds no card that is
   not finished.

7. Press the arrow in the title bar of that card. Then update the card in
   client A, without the field `finished`, with a new badge text.

   Result: The card stays folded, and its line shows the new badge text.

8. In client A, update that card with `"finished": true`.

   Result: The card is folded.

9. Do the restart of B07 steps 1 to 3.

   Result: Each card has the fold that it had before the restart.

10. In client A, create a card with `"finished": "yes"`.

    Result: The answer is `invalid_document`, and no card is created.

### 5.34 B34 — The last buttons of a card that scrolls stay at its foot

1. Make the window of the fixture 800 points wide and 600 points high. In
   client A, create a card whose document holds these components, in this order:
   one `heading`; two `kv` components of 15 rows each; one `buttons` component
   with `Connect` (tone `primary`) and `Start Codex here`; one `buttons`
   component with `Stop`, `Restart`, `Rebuild`, `Read the log`, `Rename`,
   `Add a key` and `Delete` (tone `danger`); one `text` component with
   `The machine costs money while it runs.`. Move the view until it shows the
   whole card.

   Result: The card is 560 points high. All nine buttons stand at the foot of
   the card, under a thin line and above the footer, and you turned no wheel.
   The buttons take three rows. `Connect` is the accent button, and `Delete` is
   red at the right edge of its row. The mark `More below` stands above the thin
   line.

2. Put the pointer on the rows of the card and turn the wheel down to the end.

   Result: The rows move up, and the nine buttons do not move. At the end, the
   text `The machine costs money while it runs.` stands above the thin line, and
   the mark is gone.

3. In client A, call `next_click` for the card and keep the call open. Press
   `Start Codex here`.

   Result: The call answers `clicked` with the `component_id` of that button.

4. In client A, update the card with the same document, but with two rows in
   each `kv` component.

   Result: The card ends directly under its content. It shows no thin line and
   no mark. The buttons stand in the body, in the order of the document, and the
   text stands under them.

5. In client A, update the card with the document of step 1. Then drag the
   bottom edge of the card up until the card is about 240 points high.

   Result: After the update, the buttons stand at the foot again. In the low
   card, the buttons scroll with the rows: a foot takes not more than half of
   the height of a card.

## 6. With a real agent

These steps check the same behaviour with a real AI agent. Do them in your own
Horizon, in a workspace of its own, with an agent panel that can call
`block_panel`. They need no fixture state.

1. Ask the Claude Code panel to draw a card with one heading and two buttons,
   `Stop` and `Delete`.

   Result: A card appears in the workspace of that panel, with the heading and
   both buttons. Its footer names the agent, and its titlebar chip says that the
   agent listens while it waits for a click.

2. Press `Stop`.

   Result: The agent reads the click and changes the card, as the request of the
   person says. The card keeps its place, and the clock of the footer changes.

3. Press `Delete`.

   Result: The agent closes the card, and the card leaves the workspace.

4. Ask the panel to draw a card again, and leave the agent idle, with no waiting
   `next_click`. Press a button of that card while the agent panel does not hold
   the keyboard focus.

   Result: One line arrives in the terminal of the agent panel:

   ```
   A button was pressed on your Horizon card. Call the block_panel tool with operation next_click to read it.
   ```

5. Do steps 1 to 4 with a Codex panel.

   Result: The same result, and the footer and the chip name the Codex agent.

## 7. Pass criteria

- B01: The card appears in the workspace of the calling agent, at revision 1,
  and takes no keyboard focus. It stands in the column at the right of the agent
  panel with level top edges.
- B02 and B03: Each new document raises the revision, and list and inspect
  report the card.
- B04: A hidden card keeps its document and revision, and a shown card returns.
- B05: An invalid document is refused with the offending path, and no card is
  created.
- B06: Another agent reads the card and cannot change, hide or close it.
- B07: The card returns after a restart with the same document and revision.
- B08: A closed card leaves the list and is unavailable.
- B09: A press on a button reaches the owning agent once, with the revision the
  person saw.
- B10: A click reports the selected row ids, and a removed row clears the
  selection.
- B11: A wait ends with `timed_out` at its bound, and a later click still waits.
- B12: A click without a waiting `next_click` types one fixed line into the
  owner's terminal.
- B13: No wake line goes out while the owner's agent panel holds the keyboard
  focus, and the line follows the focus away.
- B14: Without its agent the card says so, takes no click, shows its selected
  row in a dimmed form, fits its height to the frame, and comes back with its
  owner.
- B15: A full queue drops the newest click and keeps the older ones in order.
- B16: Every component, the chip and the footer are drawn as the design
  describes, in both themes, and a narrow card truncates or wraps without
  overlap. The card is as tall as its content needs, a card that is shorter than
  its content scrolls above a fixed footer, and no part of the body is drawn
  under the footer. At 50 percent zoom every button of the card still takes its
  press, because the resize handle of a card stops above its footer strip.
- B17: One wait without `panel_id` covers every card of its agent, answers the
  oldest click of those cards, and a caller without a card gets `no_cards`.
- B18: A row that the document marks as selected draws as selected, travels with
  the next click until the person selects another row, and comes back after an
  update that keeps its row.
- B19: A checked route opens an SSH panel beside the caller, without keyboard
  focus, and its `ssh` line holds `-t` and the remote command only when the
  request holds a command. The terminal is not a card.
- B20: Each route and command that is not plain is refused with its typed code
  and a message that names the field, and no panel is opened.
- B21: The titlebar of an opened terminal says `opened by` and the agent name,
  the person can type in the panel, a panel that the person opened holds no such
  chip, and the person can close the panel.
- B22: The mark returns with the terminal after a restart.
- B23: `machine_key` makes one key pair and answers the public line only, a
  second call returns the same public key with `created: false`, `machine_keys`
  lists the key, the directory is `0700` and the private key `0600`, a name that
  breaks the rule is `invalid_key_name`, and `delete_machine_key` removes the
  key pair and answers `key_unavailable` for a key that is gone.
- B24: A terminal with `key` runs `ssh` with the `-i` path of that key, the four
  options of the key, `-t` for a remote command and the command last; the key of
  the workspace serves every agent panel of the workspace; an unknown key is
  `key_unavailable` and opens no panel; and a restored panel whose key was
  deleted still starts and shows the warning of `ssh`.
- B25: The scripted agent draws the card of its scenario beside its own panel,
  answers a click with one fixed change, and closes the card at the button that
  closes. Its terminal names each step, and no line of it looks like a working
  agent. A narrow card keeps its heading and moves its badge under the second
  line.
- B26: The Connect button opens one SSH terminal beside the agent panel with the
  chip of that agent, and the card says when it opened.
- B27: Another agent reads the cards of the scripted agent and is refused every
  change, and the document of the card does not change.
- B28: A first start with `--theme light` draws the workspace and the cards on
  light surfaces, a panel without an SSH host refuses the Connect button on the
  card, and a later run in the same private home keeps the light theme even with
  `--theme dark`.
- B29: A link draws its label in the accent colour and the host of its URL under
  it, and the hover text shows the whole URL and `Opens in a browser panel`.
  Nothing is fetched before a press. One press opens exactly one browser panel
  beside the card, in the workspace of the card, at the page of the link, and it
  records no click. The link stays live while the agent of the card is gone. A
  press whose browser engine cannot start says its reason twice: in the sidebar
  notice and in the line `Could not open: <reason>` under the link, which the
  next press that opens the page or the next document removes. A URL with
  credentials and a ninth link are refused as `invalid_document`, and the message
  names the place without repeating the address.
- B30: A press on the mark of a card that the view cuts takes one page of its
  body, and the next press brings the foot of the card into the view at the zoom
  of the moment, takes the top of a card that is taller than the view out of it,
  and leaves no mark.

- B31: The cards of one agent panel stand in one column at the right of that
  panel, the newest card at the top, with the gap of tiles and no hole. A panel
  that holds the place at the right of the agent panel puts the column at its own
  right. The column moves with its agent panel. A card that the person drags
  stays where the person put it, also after a restart.
- B32: A card has the height of its content, from 140 points to the ceiling of
  its window, in a workspace with a preset too. The ceiling is the height of the
  canvas, not less than 560 points and not more than 900 points. A card above
  the ceiling scrolls in its body, shows `More below`, and keeps the buttons at
  its end in its foot. A card that grows moves the
  cards under it. A card that the person resized keeps its size.
- B33: A finished card is one line with its heading and its badge. A press on
  the line opens it, and the arrow in its title bar folds and opens every card.
  An update folds only a card that becomes finished, and never a card that the
  person opened after it was finished. The fold returns after a restart. A value
  of `finished` that is not `true` or `false` is `invalid_document`.
- B34: The `buttons` components that end the document of a card that scrolls
  stand at the foot of the card, above the footer, and only `text` components
  may follow them in the document. The rows scroll above them, each button takes
  its press, and the mark `More below` stands above them. A card that shows its
  whole content draws its buttons in its body, and a card whose foot would take
  more than half of its height has no foot.

## 8. Cleanup

1. Stop client A and client B.

   Result: Both MCP processes stop.

2. Close each Device panel that this procedure opened with `device_panel`
   `{"operation":"close","panel_id":"<id>"}`.

   Result: The panels leave your workspace. This closes the viewer only, not the
   fixture.

3. Close the fixture Horizon of the protocol tasks, then the fixtures of
   `<state-agent>`, `<state-tall>` and `<state-light>`.

   Result: Each fixture stops its processes and the private display.

> **CAUTION:** DELETE ONLY `<state>`, `<state-agent>`, `<state-tall>`,
> `<state-light>` AND `<work>` THAT YOU MADE FOR THIS PROCEDURE. Do not delete your own home folder
> or a folder that another test uses. A reused private home holds the sessions
> and records of the earlier run.

4. Delete `<state>`, `<state-agent>`, `<state-tall>`, `<state-light>` and
   `<work>`.

   Result: No private state of this procedure remains.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
