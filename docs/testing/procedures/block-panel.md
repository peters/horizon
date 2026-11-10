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
Horizon.

## 2. Applicability

- Candidate: a frozen debug build of one exact commit, `<candidate-commit>`.
- Platforms: Linux. Watch the candidate live through a native VNC Device panel
  in your own Horizon workspace, as `AGENTS.md` requires.
- This procedure does not test clicks on a card. Buttons and list rows are inert
  in this version, and no operation delivers a click.

## 3. Equipment and preconditions

- A Linux computer with Xvfb, openbox, x11vnc, Python 3 and dbus. The
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
