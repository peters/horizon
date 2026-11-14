---
procedure: plan-card
feature: The plan card of the horizon-plan skill
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Plan card test procedure

## 1. Purpose

This procedure proves that the card documents of the `horizon-plan` skill are
valid cards, and that the person reads the plan, the state of each todo, its
place, its evidence, the question of a todo that needs the person, and the
history of the plan. It draws the card at five moments: the short path at its
start and at its end, the plan before the go, a plan that runs with an added
todo, and a plan of twelve todos with a question. It then proves that a press on
an answer button and a press on a button of the card reach the agent that owns
the card. No AI model, no helper agent and no machine is necessary.

## 2. Applicability

- Candidate: a frozen debug build of one exact commit, `<candidate-commit>`.
- Platforms: Linux. Watch the candidate live through a native VNC Device panel in
  your own Horizon workspace, as `AGENTS.md` requires.
- This procedure does not test the work of a todo, a helper agent, a worktree or
  the plan file. It tests the cards of the skill only.

## 3. Safety

> **CAUTION: USE A PRIVATE HORIZON HOME AND A PRIVATE EVIDENCE FOLDER FOR THIS
> RUN.** Do not write the cards of this procedure into the workspace of your own
> work, and do not delete a folder that you did not make.

## 4. Equipment and preconditions

- A Linux computer with Xvfb, openbox, x11vnc, ImageMagick and dbus, as the
  [local device smoke fixture](../../../scripts/device-smoke/README.md) uses
  them.
- The candidate `horizon` binary, `<candidate>`.
- A stdio MCP client that can start `horizon --browser-mcp` and send JSON-RPC
  messages.
- An empty evidence folder `<state>` for the private Horizon home of this run,
  and an empty work folder `<work>` for the files of this procedure.

## 5. Setup

1. Write this panel list to `<work>/panels.json`. It gives the fixture one agent
   panel, which is the owner of the cards:

   ```json
   [
     {"name": "Agent A", "kind": "codex", "command": "/usr/bin/python3",
      "args": ["<work>/probe.py", "<work>/identity-a.json"], "cwd": "<work>",
      "position": [40, 620], "size": [540, 300]}
   ]
   ```

2. Write this probe to `<work>/probe.py`. It records the identity that Horizon
   gives to the agent panel, then waits:

   ```python
   import json, os, sys, time
   keys = ["HORIZON_BROWSER_ACTOR", "HORIZON_BROWSER_HOST_INSTANCE"]
   json.dump({k: os.environ.get(k) for k in keys}, open(sys.argv[1], "w"))
   while True:
       time.sleep(3600)
   ```

   Result: The two files exist.

3. Start the fixture on a private desktop:

   ```sh
   python3 scripts/device-smoke/serve.py --horizon <candidate> --native-view \
     --state <state> --panels <work>/panels.json
   ```

   Result: The fixture prints one JSON line with `display`, `vnc_address` and
   `pids`. `<work>/identity-a.json` holds `HORIZON_BROWSER_ACTOR` and
   `HORIZON_BROWSER_HOST_INSTANCE`.

4. From your agent panel, call `device_panel` with
   `{"operation":"create","endpoint":"<vnc_address>"}`. Then call it with
   `{"operation":"inspect","panel_id":"<returned id>"}`.

   Result: `connection` is `connected`, `image_received` and `image_displayed`
   are true, and `frame_sequence` advances.

5. Start client A in `<work>` with the two variables of
   `<work>/identity-a.json`, so its requests arrive as the agent panel of the
   fixture.

   Result: The client lists one tool, `block_panel`.

## 6. Tasks

Each task appends one line to the standard input of client A. Every line is one
JSON-RPC request of the `block_panel` tool. The five cards hold 3, 8, 12 and 1
todos, so the moments P3 and P5 are one card changed in place.

### 6.1 P1 — The plan before the go

1. Append this line. It draws the card of a plan of three todos, before the
   person gave the go.

   ```
   {"tool": "block_panel", "arguments": {"operation": "create", "title": "Fix the login redirect", "document": {"components": [{"type": "heading", "text": "Fix the login redirect", "subtitle": "Issue #1987 · this computer", "badge": {"text": "Plan", "tone": "info"}}, {"type": "progress", "label": "0 of 3 done · 0 needs you", "percent": 0, "detail": "no todo runs"}, {"type": "caption", "text": "Todos"}, {"type": "list", "selection": "none", "rows": [{"id": "t1", "title": "Read the issue and the failing test", "subtitle": "coordinator · this computer", "value": "~12 m", "badge": {"text": "Waiting"}}, {"id": "t2", "title": "Change the redirect target", "subtitle": "helper-1 · this computer", "value": "~20 m", "badge": {"text": "Waiting"}}, {"id": "t3", "title": "Run the test and the full check", "subtitle": "coordinator · this computer", "value": "~15 m", "badge": {"text": "Waiting"}}]}, {"type": "text", "text": "Three todos, two at once, on this computer, as AGENTS.md says. The plan file is in .horizon-plan/ of the worktree, and git ignores it. Nothing outside the worktree changes."}, {"type": "buttons", "items": [{"id": "go", "label": "Go", "tone": "primary"}, {"id": "change", "label": "Change the plan"}, {"id": "stop", "label": "Stop", "tone": "danger"}]}]}}, "timeout": 30}
   ```

   Result: A card appears beside the panel of `Agent A`. The heading is `Fix the
   login redirect`, its subtitle holds the issue and the place, and its badge
   reads `Plan`. The bar of the summary line reads `0 of 3 done · 0 needs you`,
   and its detail reads `no todo runs`. Under the caption `TODOS` are three rows,
   each in the form `todo · who · where · time`, for example
   `Change the redirect target`, `helper-1 · this computer`, `~20 m`, with the
   badge `Waiting`. The text names the policy of the repository. Below the text
   are the buttons `Go`, `Change the plan` and `Stop`, and `Stop` is drawn at the
   right edge in the danger tone.

2. Read the answer of the request.

   Result: The answer is `created` with a `panel_id` and `revision` 1. Record the
   `panel_id`.

### 6.2 P2 — A plan that runs, with evidence, an added todo and the history

1. Append this line. It draws the card of a plan of eight todos: three done, one
   that runs, one that needs the person, one that the coordinator added, and the
   last five events of the history.

   ```
   {"tool": "block_panel", "arguments": {"operation": "create", "title": "Fix the login redirect", "document": {"components": [{"type": "heading", "text": "Fix the login redirect", "subtitle": "Issue #1987 · this computer", "badge": {"text": "Running", "tone": "info"}}, {"type": "progress", "label": "3 of 8 done · 1 needs you", "percent": 38, "detail": "t4 now"}, {"type": "caption", "text": "Todos"}, {"type": "list", "selection": "none", "rows": [{"id": "t1", "title": "Read the issue and the failing test", "subtitle": "coordinator · this computer · 12 tests pass", "value": "4 m", "badge": {"text": "Done", "tone": "ok"}}, {"id": "t2", "title": "Add the redirect case to the test", "subtitle": "coordinator · this computer · 12 tests pass", "value": "6 m", "badge": {"text": "Done", "tone": "ok"}}, {"id": "t3", "title": "Check the route table", "subtitle": "helper-1 · build-1 · 9 rows", "value": "9 m", "badge": {"text": "Done", "tone": "ok"}}, {"id": "t4", "title": "Change the redirect target", "subtitle": "helper-2 · this computer", "value": "8 m", "badge": {"text": "Now", "tone": "info"}}, {"id": "t5", "title": "Choose the address of the redirect", "subtitle": "you · this computer", "value": "waits", "badge": {"text": "Needs you", "tone": "warn"}}, {"id": "t6", "title": "Update the route notes", "subtitle": "coordinator · this computer", "value": "~5 m", "badge": {"text": "Waiting"}}, {"id": "t7", "title": "Run the full check", "subtitle": "coordinator · build-1", "value": "~20 m", "badge": {"text": "Waiting"}}, {"id": "t8", "title": "Write the report of the task", "subtitle": "added · coordinator · this computer", "value": "~5 m", "badge": {"text": "Waiting"}}]}, {"type": "caption", "text": "Needs you"}, {"type": "text", "text": "t5: Which address must the redirect use?"}, {"type": "buttons", "items": [{"id": "answer.t5.account", "label": "The account address", "tone": "primary"}, {"id": "answer.t5.last", "label": "The last address"}, {"id": "answer.t5.skip", "label": "Skip this todo"}]}, {"type": "caption", "text": "History"}, {"type": "list", "selection": "none", "rows": [{"id": "h1", "title": "t3 done on build-1, 9 rows", "subtitle": "t3", "value": "07:31"}, {"id": "h2", "title": "t2 done, 12 tests pass", "subtitle": "t2", "value": "07:26"}, {"id": "h3", "title": "t5 waits for the person", "subtitle": "t5", "value": "07:22"}, {"id": "h4", "title": "t8 added: the report was missing", "subtitle": "t8", "value": "07:20"}, {"id": "h5", "title": "The person pressed Go", "subtitle": "plan", "value": "07:10"}]}, {"type": "buttons", "items": [{"id": "stop", "label": "Stop", "tone": "danger"}]}]}}, "timeout": 30}
   ```

   Result: A second card appears. Its badge reads `Running`. The summary line
   reads `3 of 8 done · 1 needs you`, its bar fills to 38 percent, and its detail
   reads `t4 now`. Eight rows are drawn:

   - Three rows carry the badge `Done` in the ok tone, a measured time, and one
     line of evidence after the place, such as
     `coordinator · this computer · 12 tests pass`.
   - One row carries the badge `Now` in the info tone.
   - One row carries the badge `Needs you` in the warn tone and the value
     `waits`.
   - Two rows carry the badge `Waiting` and an estimate with `~` in front.
   - The row `t8` starts its subtitle with `added`, so the person reads that the
     coordinator added it after the go.

   Under the caption `NEEDS YOU` is the question `t5: Which address must the
   redirect use?` and three buttons: `The account address`, `The last address`
   and `Skip this todo`. Under the caption `HISTORY` are five rows, newest
   first, each with the event, its todo and a clock time. The last row of the
   card holds `Stop`.

2. Append this line. It waits for a press on any card of the agent.

   ```
   {"tool": "block_panel", "arguments": {"operation": "next_click", "timeout_millis": 300000}, "timeout": 300}
   ```

   Result: The request waits.

3. Press `The account address` on the second card.

   Result: The request answers `clicked`. `click.component_id` is
   `answer.t5.account`, `click.panel_id` is the `panel_id` of the second card,
   `click.revision` is the revision that the person saw, `click.selected` is
   empty, and `click.stale` is false.

### 6.3 P3 — Twelve todos, a question and five history lines

1. Append this line. It replaces the document of the second card with the card of
   a plan of twelve todos: eleven done with their evidence, one that needs the
   person, and the last five events.

   ```
   {"tool": "block_panel", "arguments": {"operation": "update", "panel_id": "<panel_id of P2>", "document": {"components": [{"type": "heading", "text": "Fix the login redirect", "subtitle": "Issue #1987 · this computer", "badge": {"text": "Running", "tone": "info"}}, {"type": "progress", "label": "11 of 12 done · 1 needs you", "percent": 92, "detail": "stopped at t12"}, {"type": "caption", "text": "Todos"}, {"type": "list", "selection": "none", "rows": [{"id": "t1", "title": "Read the issue and the failing test", "subtitle": "coordinator · this computer · 12 tests pass", "value": "4 m", "badge": {"text": "Done", "tone": "ok"}}, {"id": "t2", "title": "Add the redirect case to the test", "subtitle": "coordinator · this computer · 12 tests pass", "value": "6 m", "badge": {"text": "Done", "tone": "ok"}}, {"id": "t3", "title": "Check the route table", "subtitle": "helper-1 · build-1 · 9 rows", "value": "9 m", "badge": {"text": "Done", "tone": "ok"}}, {"id": "t4", "title": "Change the redirect target", "subtitle": "helper-2 · this computer · 12 tests pass", "value": "8 m", "badge": {"text": "Done", "tone": "ok"}}, {"id": "t5", "title": "Choose the address of the redirect", "subtitle": "you · this computer · the person chose the account address", "value": "2 m", "badge": {"text": "Done", "tone": "ok"}}, {"id": "t6", "title": "Update the route notes", "subtitle": "coordinator · this computer · the notes name one address", "value": "5 m", "badge": {"text": "Done", "tone": "ok"}}, {"id": "t7", "title": "Run the full check", "subtitle": "coordinator · build-1 · 412 tests pass", "value": "20 m", "badge": {"text": "Done", "tone": "ok"}}, {"id": "t8", "title": "Write the report of the task", "subtitle": "added · coordinator · this computer · 4 lines in the PR", "value": "5 m", "badge": {"text": "Done", "tone": "ok"}}, {"id": "t9", "title": "Read the review of the change", "subtitle": "helper-1 · build-1 · 2 findings", "value": "3 m", "badge": {"text": "Done", "tone": "ok"}}, {"id": "t10", "title": "Fix the two findings of the review", "subtitle": "coordinator · this computer · 412 tests pass", "value": "6 m", "badge": {"text": "Done", "tone": "ok"}}, {"id": "t11", "title": "Run the full check again", "subtitle": "coordinator · build-1 · 412 tests pass", "value": "4 m", "badge": {"text": "Done", "tone": "ok"}}, {"id": "t12", "title": "Name the branch of the push", "subtitle": "you · this computer", "value": "waits", "badge": {"text": "Needs you", "tone": "warn"}}]}, {"type": "caption", "text": "Needs you"}, {"type": "text", "text": "t12: May I push the branch fix-login-redirect and open a pull request?"}, {"type": "buttons", "items": [{"id": "answer.t12.push", "label": "Push and open the PR", "tone": "primary"}, {"id": "answer.t12.wait", "label": "Not now"}]}, {"type": "caption", "text": "History"}, {"type": "list", "selection": "none", "rows": [{"id": "h1", "title": "t12 waits: a push is the person's", "subtitle": "t12", "value": "09:12"}, {"id": "h2", "title": "t11 done on build-1, 412 tests pass", "subtitle": "t11", "value": "09:08"}, {"id": "h3", "title": "t10 done, the two findings are fixed", "subtitle": "t10", "value": "09:01"}, {"id": "h4", "title": "t9 done, 2 findings", "subtitle": "t9", "value": "08:55"}, {"id": "h5", "title": "t8 added by the person in the agent panel", "subtitle": "t8", "value": "08:40"}]}, {"type": "buttons", "items": [{"id": "stop", "label": "Stop", "tone": "danger"}]}]}}, "timeout": 30}
   ```

   Result: The second card changes in place, so its `panel_id` stays the one of
   step 2 of P2, and the answer is `updated` with a revision above 1. The summary
   line reads `11 of 12 done · 1 needs you`. Twelve rows are drawn and every done
   row ends its subtitle with its evidence. The question names the push and the
   pull request. Five history rows are drawn under the caption `HISTORY`. The
   card holds ten components and two lists of twelve and five rows, which are
   inside the bounds of 64 components and 32 rows in one list.

2. Append the `next_click` line of step 2 of P2 again, and press `Push and open
   the PR` on the second card.

   Result: The request answers `clicked` with `click.component_id`
   `answer.t12.push` and the `panel_id` of the second card.

### 6.4 P4 — The short path

1. Append this line. It draws the card of a task of one todo, with no plan of
   three todos and no go.

   ```
   {"tool": "block_panel", "arguments": {"operation": "create", "title": "Fix the login redirect", "document": {"components": [{"type": "heading", "text": "Fix the login redirect", "subtitle": "Issue #1987 · this computer", "badge": {"text": "Running", "tone": "info"}}, {"type": "progress", "label": "1 todo · starting", "percent": 10, "detail": "this computer"}, {"type": "buttons", "items": [{"id": "stop", "label": "Stop", "tone": "danger"}]}]}}, "timeout": 30}
   ```

   Result: A third card appears with the badge `Running`, one summary line that
   reads `1 todo · starting`, the place `this computer` in the detail of the
   summary line and in the subtitle of the heading, and one button `Stop` in the
   danger tone. The card holds no `Go` button and no list of todos.

2. Read the answer of the request.

   Result: The answer is `created` with a `panel_id` and `revision` 1. Record the
   `panel_id`.

3. Append this line. It replaces the document of the third card with the card of
   the same task when the todo is done.

   ```
   {"tool": "block_panel", "arguments": {"operation": "update", "panel_id": "<panel_id of P4>", "document": {"components": [{"type": "heading", "text": "Fix the login redirect", "subtitle": "Issue #1987 · this computer", "badge": {"text": "Done", "tone": "ok"}}, {"type": "text", "text": "The redirect of the sign-in page names the account address. Check: cargo test -p auth redirect, 12 tests pass. Evidence: check, the same command, .horizon-plan/progress-t1.md."}, {"type": "buttons", "items": [{"id": "close", "label": "Close"}]}]}}, "timeout": 30}
   ```

   Result: The third card changes in place. Its badge reads `Done` in the ok
   tone, one text holds the result and the evidence with the command and its
   result line, and the only button is `Close`.

## 7. Pass criteria

- Five cards are drawn at five moments, and each one reads as its task says.
- The summary line reads `0 of 3 done · 0 needs you`, then `3 of 8 done · 1 needs
  you`, then `11 of 12 done · 1 needs you`, then `1 todo · starting`.
- Every row holds a todo, an owner, a place and a time, and the badge of the
  state. The place is in the subtitle of every row of every card.
- Every done row ends its subtitle with one line of evidence, and every done todo
  of the plan file holds one `evidence` entry of the kind that the line shows.
- One row starts its subtitle with `added`, and the history holds five rows
  newest first.
- A press on an answer button answers `clicked` with the id of that button and no
  selection.
- `update` keeps the `panel_id` of the card and increases its revision.
- The short path draws no `Go` button, and its end card holds the result and the
  evidence.

## 8. Cleanup

1. Stop client A.

   Result: The MCP process stops.

2. Close each Device panel that this procedure opened with `device_panel`
   `{"operation":"close","panel_id":"<id>"}`.

   Result: The viewers leave your workspace. This closes the viewer only, not the
   fixture.

3. Close the fixture Horizon.

   Result: The fixture stops its processes and the private display.

> **CAUTION: DELETE ONLY `<state>` AND `<work>` THAT YOU MADE FOR THIS
> PROCEDURE.** Do not delete your own home folder or a folder that another test
> uses.

4. Delete `<state>` and `<work>`.

   Result: No private state of this procedure remains.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
