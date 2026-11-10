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
owns the card, with its selection and its time, and that Horizon wakes an idle
agent for a click.

## 2. Applicability

- Candidate: a frozen debug build of one exact commit, `<candidate-commit>`.
- Platforms: Linux. Watch the candidate live through a native VNC Device panel
  in your own Horizon workspace, as `AGENTS.md` requires.
- The click tasks send pointer presses to the fixture display with `xdotool`,
  scoped to the display and the process of the fixture. A pointer press on a card
  gives the keyboard focus to that card, because the card is a panel.

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
- An empty evidence folder `<state>` for the fixture state and an empty work
  folder `<work>` for the files of this procedure.

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
   heading, the badge and the address.

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

2. In client A, call `block_panel` with `{"operation":"list"}`.

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

1. Close agent panel A in the live view, with its close button.

   Result: The card shows one line, `Its agent is not here.`, and its button and
   rows are drawn disabled.

2. Press `Continue` on the card.

   Result: Nothing is recorded. In client B, `block_panel` with
   `{"operation":"list"}` reports `pending_clicks` 0, and `inspect` reports
   `agent_present` false.

3. Start the fixture again on the same state with `--reuse-state
   --resume-session`, as in B07 step 2, and read the new identity of agent panel
   A from `<work>/identity-a.json`.

   Result: The card keeps its document and it is live again, because its owner
   is back.

### 5.15 B15 — A full queue of clicks

1. Press `Continue` on the fixture display 33 times:

   ```sh
   DISPLAY=<display> xdotool mousemove <x> <y> click --repeat 33 --delay 150 1
   ```

   Result: The card shows `Too many clicks are waiting.` and `list` reports
   `pending_clicks` 32.

2. In client A, call `next_click` 32 times and read each answer.

   Result: The 32 oldest clicks come back in the order of the presses, each with
   a higher `event_id`, and the oldest is never lost. After the last answer the
   card no longer shows the line and `pending_clicks` is 0.

## 6. Pass criteria

- B01: The card appears in the workspace of the calling agent, at revision 1,
  and takes no keyboard focus.
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
- B14: Without its agent the card says so, takes no click, and comes back with
  its owner.
- B15: A full queue drops the newest click and keeps the older ones in order.

## 7. Cleanup

1. Stop client A and client B.

   Result: Both MCP processes stop.

2. Close the Device panel with
   `device_panel` `{"operation":"close","panel_id":"<id>"}`.

   Result: The panel leaves your workspace.

3. Close the fixture Horizon normally.

   Result: The fixture stops its processes and the private display.

> **CAUTION:** DELETE ONLY `<state>` AND `<work>` THAT YOU MADE FOR THIS
> PROCEDURE. Do not delete your own home folder or a folder that another test
> uses.

4. Delete `<state>` and `<work>`.

   Result: No private state of this procedure remains.

## 8. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
