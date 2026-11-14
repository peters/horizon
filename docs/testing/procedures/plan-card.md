---
procedure: plan-card
feature: the plan card of the skill horizon-plan
platforms: [linux, macos]
cost: none
destructive: no
secrets: none
owner: peters
---

# Plan card test procedure

## 1. Purpose

This procedure proves that the plan card of the skill `horizon-plan` draws the
board, the callout and the log, in the wide form and in the narrow form. It
proves the fold of the narrow form, a press on an answer of the callout and a
press on `Show helper terminals`. The last three tasks prove the cards of a task
that came from a GitHub issue: the plan before the go, the branch that waits for
the person to choose the push, and the end with the link of the pull request.

## 2. Applicability

- Candidate: every build that holds the `board`, `callout` and `log` components
  and the `width` field of a card document.
- Platforms: Linux and macOS.
- This procedure does not test: the work of the plan, the helper agents and the
  machine list. It tests the card only.

## 3. Safety

This procedure rents no compute, deletes no data, sends no secret and changes no
access. It starts one stand-in agent that answers the replay lines. Read the
[device smoke guide](../../../scripts/device-smoke/README.md) before you open the
isolated desktop.

## 4. Equipment and preconditions

- A Horizon build of the candidate.
- One isolated desktop and a native VNC Device panel, as the agent guidelines
  describe.
- One stand-in agent in a terminal panel of the workspace. It answers a replay
  line with one `block_panel` call.
- One helper terminal that `open_terminal` can show, when task 7 runs.

## 5. Setup

1. Start Horizon on the isolated desktop.

   Result: the window shows one empty workspace.

2. Start a stand-in agent in a terminal panel. It reads one line, which is one
   whole `block_panel` request, calls the tool with it, and prints the
   `panel_id` that a `create` answer holds.

   Result: the panel waits for a line.

## 6. Tasks

Each request below is one line and one whole `block_panel` request. Give it to
the stand-in agent. An `update` names no `panel_id`: the stand-in agent adds the
`panel_id` of the last `create` answer to it. Every card document of the skill
`horizon-plan` is one of these lines.

### 6.1 A1 — the short path at its start

Give this line to the stand-in agent:

```text
{"operation":"create","title":"Plan","document":{"components":[{"type":"heading","text":"Fix the login redirect","subtitle":"Issue #1987 · this computer","badge":{"text":"Running","tone":"info"}},{"type":"progress","label":"0 of 1 done · 0 needs you","percent":0,"detail":"t1 now"},{"type":"board","columns":[{"label":"Change","tag":"1","value":"~20 m","current":true}],"rows":[{"label":"coordinator · this computer"}],"items":[{"column":0,"row":0,"title":"Fix the login redirect","state":"now","value":"~20 m","note":"check: cargo test -p auth redirect"}]},{"type":"caption","text":"AGENTS.md · 1 at once · this computer · rent: no"},{"type":"buttons","items":[{"id":"stop","label":"Stop","tone":"danger"}]}]}}
```

1. Read the card.

   Result: the card is narrow. The heading, the progress bar, one item of the
   board, the caption and the `Stop` button are in this order. The item shows
   the accent ring and the accent border of the state `now`. The second line of
   the item starts with `coordinator · this computer`.

### 6.2 A2 — the short path when it is done

Give this line to the stand-in agent:

```text
{"operation":"update","document":{"components":[{"type":"heading","text":"Fix the login redirect","subtitle":"Issue #1987 · this computer","badge":{"text":"Done","tone":"ok"}},{"type":"progress","label":"1 of 1 done · 0 needs you","percent":100,"detail":"done"},{"type":"board","columns":[{"label":"Change","tag":"1","value":"18 m"}],"rows":[{"label":"coordinator · this computer"}],"items":[{"column":0,"row":0,"title":"Fix the login redirect","state":"done","value":"18 m","note":"12 tests pass"}]},{"type":"log","entries":[{"at":"07:28","text":"t1 done: 12 tests pass"}]},{"type":"caption","text":"AGENTS.md · 1 at once · this computer · rent: no"},{"type":"buttons","items":[{"id":"close","label":"Close"}]}]}}
```

1. Read the card.

   Result: the same card changed. The state mark of the item is a filled circle
   with a dark check, and the evidence is in a soft ok colour. One log line
   shows `07:28` in the monospace face and the words behind it.

### 6.3 A3 — the plan before the go

Give this line to the stand-in agent:

```text
{"operation":"update","document":{"width":"wide","components":[{"type":"heading","text":"Fix the login redirect","subtitle":"Issue #1987 · this computer","badge":{"text":"Plan","tone":"info"}},{"type":"progress","label":"0 of 3 done · 0 needs you","percent":0,"detail":"no todo runs"},{"type":"callout","caption":"Nothing starts before Go","text":"3 todos in 2 waves, on 2 computers. Nothing is rented.","buttons":[{"id":"go","label":"Go","tone":"primary"},{"id":"change","label":"Change the plan"},{"id":"stop","label":"Stop","tone":"danger"}]},{"type":"board","columns":[{"label":"Understand","tag":"1","value":"~12 m"},{"label":"Change","tag":"2","note":"in parallel","value":"~35 m"}],"rows":[{"label":"coordinator · this computer"},{"label":"helper-1 · build-1","subtitle":"16 vCPU"}],"items":[{"column":0,"row":0,"title":"Read the issue and the failing test","value":"~12 m","note":"check: cargo test -p horizon-ui login_redirect"},{"column":1,"row":1,"title":"Change the redirect target","value":"~20 m","note":"check: cargo test -p horizon-ui"},{"column":1,"row":0,"title":"Run the test and the full check","value":"~15 m","note":"check: cargo test --workspace"}]},{"type":"caption","text":"AGENTS.md · 2 at once · this computer, build-1 · rent: no"}]}}
```

1. Read the card.

   Result: the same card is now wide. Its width follows its board: the half of
   the canvas view, between 230 plus 260 for each wave and 230 plus 480 for each
   wave, plus 32 of body padding, and 2400 at most. The heading takes the larger
   title. The callout is one row with the caption above the question at the left
   and the three answers at the right. The board is a grid: the first column of
   230 points names the workers, each wave takes a column of 480 points at most,
   the head row is 44 points.
2. Compare the drawing with the picture of the design.

   Result: the numbers of the design hold: one chip height for every item, one
   right edge for the values of a column, a hair line between two cells, and the
   corner radius of 12 around the grid.

### 6.4 A4 — the plan runs and one todo needs the person

Give this line to the stand-in agent:

```text
{"operation":"update","document":{"width":"wide","components":[{"type":"heading","text":"Fix the login redirect","subtitle":"Issue #1987 · example-org/shop · branch fix-login-redirect","badge":{"text":"Running","tone":"info"}},{"type":"progress","label":"3 of 8 done · 1 needs you","percent":38,"detail":"27 min so far · about 30 left"},{"type":"callout","tone":"warn","caption":"Needs you · t5 · waits 3 min","text":"Which address must the redirect use?","buttons":[{"id":"answer.t5.account","label":"The account address","tone":"primary"},{"id":"answer.t5.last","label":"The last address"},{"id":"answer.t5.skip","label":"Skip this todo"}]},{"type":"board","columns":[{"label":"Understand","tag":"1","value":"4 m"},{"label":"Prepare","tag":"2","note":"in parallel","value":"~9 m"},{"label":"Change","tag":"3","note":"in parallel","value":"~10 m","current":true},{"label":"Verify","tag":"4","value":"~20 m"},{"label":"Report","tag":"5","value":"~5 m"}],"rows":[{"label":"coordinator · this computer"},{"label":"helper-1 · build-1","subtitle":"16 vCPU"},{"label":"helper-2 · build-2","subtitle":"32 vCPU"},{"label":"You · this computer","subtitle":"you answer on this card"}],"items":[{"column":0,"row":0,"title":"Read the issue and the failing test","state":"done","value":"4 m","note":"1 failing test found"},{"column":1,"row":1,"title":"Add the redirect case to the test","state":"done","value":"6 m","note":"12 tests pass"},{"column":1,"row":2,"title":"Check the route table","state":"done","value":"9 m","note":"9 rows, 1 wrong"},{"column":2,"row":1,"title":"Change the redirect target","state":"now","value":"8 m","note":"edits routes/login.rs"},{"column":2,"row":3,"title":"Choose the address of the redirect","state":"needs_you","value":"waits 3 m","note":"t4 goes on with the default"},{"column":2,"row":0,"title":"Update the route notes","state":"waiting","value":"~5 m","note":"check: docs build"},{"column":3,"row":1,"title":"Run the full check","state":"waiting","value":"~20 m","note":"check: cargo test --workspace"},{"column":4,"row":0,"title":"Write the report of the task","state":"waiting","value":"~5 m","note":"check: 3 lines and the evidence","tag":"added"}]},{"type":"log","entries":[{"at":"07:31","text":"t3 done on build-2, 9 rows"},{"at":"07:28","text":"t5 waits for you: the address is your choice"},{"at":"07:26","text":"t2 done on build-1, 12 tests pass"},{"at":"07:20","text":"t8 added: the report was missing"},{"at":"07:10","text":"You pressed Go"}]},{"type":"caption","text":"AGENTS.md · 2 at once · this computer, build-1, build-2 · rent: ask"},{"type":"buttons","items":[{"id":"change","label":"Change the plan"},{"id":"stop","label":"Stop","tone":"danger"},{"id":"terminal.helper-1","label":"Show helper terminals"}]}]}}
```

1. Read the card.

   Result: the callout takes the warn tint, its caption is `NEEDS YOU · T5 ·
   WAITS 3 MIN` in the monospace face, and the question stands above the board.
   The third wave is the current one: its tag square is filled with the accent,
   a 2 point accent line stands under its head, and its cells carry a light
   accent tint. The item of t5 shows the warn ring and the warn value. No state
   mark looks like a ring of a choice.
2. Read the log.

   Result: the five entries flow in columns of at least 330 points, row by row,
   the newest first. The time of each entry keeps one column of one width.

### 6.5 A5 — twelve todos in eight waves

Give this line to the stand-in agent:

```text
{"operation":"update","document":{"width":"wide","components":[{"type":"heading","text":"Fix the login redirect","subtitle":"Issue #1987 · example-org/shop · branch fix-login-redirect","badge":{"text":"Running","tone":"info"}},{"type":"progress","label":"11 of 12 done · 1 needs you","percent":92,"detail":"52 min so far · your answer is left"},{"type":"callout","tone":"warn","caption":"Needs you · t12 · waits 2 min","text":"May I push the branch fix-login-redirect and open a pull request?","buttons":[{"id":"answer.t12.push","label":"Push and open the PR","tone":"primary"},{"id":"answer.t12.wait","label":"Not now"}]},{"type":"board","columns":[{"label":"Understand","tag":"1","value":"4 m"},{"label":"Prepare","tag":"2","note":"in parallel","value":"9 m"},{"label":"Change","tag":"3","note":"in parallel","value":"13 m"},{"label":"Fix","tag":"4","value":"7 m"},{"label":"Verify","tag":"5","note":"in parallel","value":"24 m"},{"label":"Review","tag":"6","note":"in parallel","value":"9 m"},{"label":"Report","tag":"7","value":"5 m"},{"label":"Push","tag":"8","value":"waits 2 m","current":true}],"rows":[{"label":"coordinator · this computer"},{"label":"helper-1 · build-1","subtitle":"16 vCPU"},{"label":"helper-2 · build-2","subtitle":"32 vCPU"},{"label":"helper-3 · build-1","subtitle":"another agent program"},{"label":"You · this computer","subtitle":"you answer on this card"}],"items":[{"column":0,"row":0,"title":"Read the issue and the failing test","state":"done","value":"4 m","note":"1 failing test found"},{"column":1,"row":1,"title":"Add the redirect case to the test","state":"done","value":"6 m","note":"12 tests pass"},{"column":1,"row":2,"title":"Check the route table","state":"done","value":"3 m","note":"9 rows, 1 wrong"},{"column":2,"row":1,"title":"Change the redirect target","state":"done","value":"8 m","note":"12 tests pass"},{"column":2,"row":2,"title":"Update the route notes","state":"done","value":"5 m","note":"the notes name one address"},{"column":3,"row":3,"title":"Fix the two findings of the review","state":"done","value":"7 m","note":"412 tests pass"},{"column":4,"row":1,"title":"Run the full check","state":"done","value":"20 m","note":"412 tests pass"},{"column":4,"row":0,"title":"Write the report of the task","state":"done","value":"4 m","note":"4 lines in the PR","tag":"added"},{"column":5,"row":2,"title":"Read the review of the change","state":"done","value":"3 m","note":"2 findings"},{"column":5,"row":0,"title":"Check the two findings","state":"done","value":"6 m","note":"412 tests pass"},{"column":6,"row":0,"title":"Write the report of the task","state":"done","value":"5 m","note":"the report is in the PR"},{"column":7,"row":4,"title":"Name the branch of the push","state":"needs_you","value":"waits 2 m","note":"your push is your own choice"}]},{"type":"log","entries":[{"at":"09:12","text":"t12 waits: a push is the person's"},{"at":"09:08","text":"t11 done on build-1, 412 tests pass"},{"at":"09:01","text":"t10 done, the two findings are checked"},{"at":"08:55","text":"t9 done, 2 findings"},{"at":"08:40","text":"t8 added by the person in the agent panel"}]},{"type":"caption","text":"AGENTS.md · 2 at once · this computer, build-1, build-2 · rent: ask"},{"type":"buttons","items":[{"id":"change","label":"Change the plan"},{"id":"stop","label":"Stop","tone":"danger"},{"id":"terminal.helper-1","label":"Show helper terminals"}]}]}}
```

1. Read the card without a scroll inside it.

   Result: eight columns and five rows fit in one card. Every item is inside its
   row, no cell shows more than four items, and the question stands above the
   board. The card is at most 1200 points tall.

### 6.6 A6 — the narrow form of the same plan

1. Make the window narrow, or resize the card of A5 to about 420 points.

   Result: the same document draws one list. Each wave has a caption line with
   its tag, its name and `NOW` in the accent for the current wave, and its value
   at the right edge. Every item takes the whole width and its second line starts
   with the label of its row, such as `helper-1 · build-1 · 12 tests pass`.

2. Read the leading waves.

   Result: the leading waves whose items all finished are folded into one line:
   a done mark, `N done` and `show` at the right edge. The items of those waves
   are not drawn.

3. Press the fold line.

   Result: the items of the folded waves appear with their captions. The word at
   the right edge now reads `hide`.

4. Press the fold line again.

   Result: the items fold away and the word reads `show`.

5. Send `next_click` from the stand-in agent.

   Result: the fold sent no click. No answer arrives.

### 6.7 A7 — a press on an answer of the callout

1. Press `The account address` on the card of A4.

   Result: `next_click` answers with the click of the id `answer.t5.account`.

### 6.8 A8 — show the helper terminals

1. Press `Show helper terminals` on the card of A4.

   Result: `next_click` answers with the click of the id `terminal.helper-1`, and
   the terminals of the helpers appear. No terminal opened before this press.

### 6.9 A9 — a stopped plan

Give this line to the stand-in agent:

```text
{"operation":"update","document":{"width":"wide","components":[{"type":"heading","text":"Fix the login redirect","subtitle":"Issue #1987 · this computer","badge":{"text":"Stopped","tone":"warn"}},{"type":"progress","label":"3 of 8 done · 1 needs you","percent":38,"detail":"stopped at t5"},{"type":"callout","tone":"warn","caption":"Needs you · t5 · stopped","text":"The helper of t4 on build-1 stopped with the link. Continue on this computer?","buttons":[{"id":"resume","label":"Continue on this computer","tone":"primary"},{"id":"answer.t5.account","label":"The account address"},{"id":"answer.t5.last","label":"The last address"}]},{"type":"board","columns":[{"label":"Understand","tag":"1","value":"4 m"},{"label":"Prepare","tag":"2","note":"in parallel","value":"9 m"},{"label":"Change","tag":"3","note":"in parallel","value":"~10 m"}],"rows":[{"label":"coordinator · this computer"},{"label":"helper-1 · build-1","subtitle":"16 vCPU"}],"items":[{"column":0,"row":0,"title":"Read the issue and the failing test","state":"done","value":"4 m","note":"1 failing test found"},{"column":1,"row":1,"title":"Add the redirect case to the test","state":"done","value":"6 m","note":"12 tests pass"},{"column":1,"row":0,"title":"Check the route table","state":"done","value":"3 m","note":"9 rows, 1 wrong"},{"column":2,"row":1,"title":"Change the redirect target","state":"failed","value":"3 m","note":"the link to build-1 stopped"}]},{"type":"log","entries":[{"at":"08:02","text":"t4 failed: the link to build-1 stopped"},{"at":"07:58","text":"left: t5 to t8, and no test chooses the address"},{"at":"07:52","text":"t3 done on this computer, 9 rows, 1 wrong"},{"at":"07:48","text":"t2 done on build-1, 12 tests pass"},{"at":"07:40","text":"You pressed Go"}]},{"type":"caption","text":"AGENTS.md · 2 at once · this computer, build-1 · rent: no"},{"type":"buttons","items":[{"id":"change","label":"Change the plan"},{"id":"stop","label":"Stop","tone":"danger"}]}]}}
```

1. Read the card.

   Result: the item of t4 has a filled circle in the danger colour with a dark
   cross, and its chip carries a danger border. The callout asks the question of
   the stopped plan and holds its answers.

### 6.10 A10 — the plan of an issue before the go

Give this line to the stand-in agent:

```text
{"operation":"update","document":{"width":"wide","components":[{"type":"heading","text":"Fix the login redirect","subtitle":"Issue #1987 · example-org/shop · branch fix-login-redirect","badge":{"text":"Plan","tone":"info"}},{"type":"progress","label":"0 of 3 done · 0 needs you","percent":0,"detail":"no todo runs"},{"type":"callout","caption":"Nothing starts before Go","text":"3 todos in 2 waves, on this computer and build-1. Nothing is rented. The pull request waits for your press.","buttons":[{"id":"go","label":"Go","tone":"primary"},{"id":"change","label":"Change the plan"},{"id":"stop","label":"Stop","tone":"danger"}]},{"type":"board","columns":[{"label":"Understand","tag":"1","value":"~12 m"},{"label":"Change","tag":"2","note":"in parallel","value":"~35 m"}],"rows":[{"label":"coordinator · this computer"},{"label":"helper-1 · build-1","subtitle":"16 vCPU"}],"items":[{"column":0,"row":0,"title":"Read the issue and the failing test","value":"~12 m","note":"check: cargo test -p horizon-ui login_redirect"},{"column":1,"row":1,"title":"Change the redirect target","value":"~20 m","note":"check: cargo test -p horizon-ui"},{"column":1,"row":0,"title":"Run the check and open the pull request","value":"~15 m","note":"check: cargo test --workspace"}]},{"type":"caption","text":"AGENTS.md · 2 at once · this computer, build-1 · rent: no"}]}}
```

1. Read the card.

   Result: the subtitle of the heading names the issue and the branch, the
   callout says what runs and what it costs, and the card holds no button row
   beside the three answers of the callout. Press `Go`.

   Result: `next_click` answers with the click of the id `go`.

### 6.11 A11 — the branch is ready and the person chooses the push

Give this line to the stand-in agent:

```text
{"operation":"update","document":{"width":"wide","components":[{"type":"heading","text":"Fix the login redirect","subtitle":"Issue #1987 · example-org/shop · branch fix-login-redirect","badge":{"text":"Running","tone":"info"}},{"type":"progress","label":"11 of 12 done · 1 needs you","percent":92,"detail":"the branch is ready"},{"type":"callout","tone":"warn","caption":"Needs you · the branch is ready","text":"The branch fix-login-redirect holds 4 commits and the checks pass. Nothing is on GitHub yet. Push the branch writes the branch to example-org/shop. Push and open a pull request writes the branch and a pull request there. Keep it local writes nothing.","buttons":[{"id":"answer.t12.push","label":"Push the branch","tone":"primary"},{"id":"answer.t12.pull_request","label":"Push and open a pull request"},{"id":"answer.t12.local","label":"Keep it local"}]},{"type":"board","columns":[{"label":"Understand","tag":"1","value":"4 m"},{"label":"Change","tag":"2","note":"in parallel","value":"13 m"},{"label":"Verify","tag":"3","note":"in parallel","value":"24 m"},{"label":"Push","tag":"4","value":"waits","current":true}],"rows":[{"label":"coordinator · this computer"},{"label":"helper-1 · build-1","subtitle":"16 vCPU"}],"items":[{"column":0,"row":0,"title":"Read the issue and the failing test","state":"done","value":"4 m","note":"1 failing test found"},{"column":1,"row":1,"title":"Change the redirect target","state":"done","value":"13 m","note":"12 tests pass"},{"column":2,"row":0,"title":"Run the full check","state":"done","value":"24 m","note":"412 tests pass"},{"column":3,"row":0,"title":"Name the branch of the push","state":"needs_you","value":"waits 2 m","note":"your push is your own choice"}]},{"type":"log","entries":[{"at":"09:12","text":"t12 waits: a push is the person's"},{"at":"09:08","text":"t11 done, 412 tests pass"},{"at":"09:01","text":"t10 done, the two findings are checked"},{"at":"08:55","text":"t9 done, 2 findings"},{"at":"08:40","text":"t8 added by the person in the agent panel"}]},{"type":"caption","text":"AGENTS.md · 2 at once · this computer, build-1 · rent: no"},{"type":"buttons","items":[{"id":"change","label":"Change the plan"},{"id":"stop","label":"Stop","tone":"danger"}]}]}}
```

1. Read the card.

   Result: the callout says in words what each of its three answers writes and
   where. No other button of the card writes anything.
2. Press `Push the branch`.

   Result: `next_click` answers with the click of the id `answer.t12.push`. The
   card of the plan writes nothing to GitHub by itself.

### 6.12 A12 — the pull request is open

Give this line to the stand-in agent:

```text
{"operation":"update","document":{"width":"wide","components":[{"type":"heading","text":"Fix the login redirect","subtitle":"Issue #1987 · example-org/shop · branch fix-login-redirect","badge":{"text":"Done","tone":"ok"}},{"type":"progress","label":"12 of 12 done · 0 needs you","percent":100,"detail":"done in 52 min"},{"type":"board","columns":[{"label":"Understand","tag":"1","value":"4 m"},{"label":"Change","tag":"2","note":"in parallel","value":"13 m"},{"label":"Verify","tag":"3","note":"in parallel","value":"24 m"},{"label":"Push","tag":"4","value":"1 m"}],"rows":[{"label":"coordinator · this computer"},{"label":"helper-1 · build-1","subtitle":"16 vCPU"}],"items":[{"column":0,"row":0,"title":"Read the issue and the failing test","state":"done","value":"4 m","note":"1 failing test found"},{"column":1,"row":1,"title":"Change the redirect target","state":"done","value":"13 m","note":"12 tests pass"},{"column":2,"row":0,"title":"Run the full check","state":"done","value":"24 m","note":"412 tests pass"},{"column":3,"row":0,"title":"Push the branch and open the pull request","state":"done","value":"1 m","note":"412 tests pass"}]},{"type":"link","label":"Pull request 1998 of example-org/shop","url":"https://github.com/example-org/shop/pull/1998"},{"type":"log","entries":[{"at":"09:16","text":"t12 done: the pull request is open"},{"at":"09:14","text":"You pressed Push and open a pull request"},{"at":"09:12","text":"t12 waits: a push is the person's"},{"at":"09:08","text":"t11 done, 412 tests pass"},{"at":"09:01","text":"t10 done, the two findings are checked"}]},{"type":"caption","text":"AGENTS.md · 2 at once · this computer, build-1 · rent: no"},{"type":"buttons","items":[{"id":"terminal.helper-1","label":"Show helper terminals"},{"id":"close","label":"Close"}]}]}}
```

1. Read the card.

   Result: the card holds one `link` component with the label of the pull
   request, the host of the address under it, and no callout. A press on the
   link opens the page in a browser panel of the card's workspace and sends no
   click to the agent.

## 7. Pass criteria

- Tasks A1 to A12 pass.
- The wide form and the narrow form draw the same document, and the numbers of
  the design hold: 44 points of the head row, 230 points of the worker column,
  one chip height, one right edge of the values, one hair line between cells.
- No part of the card scrolls in the wide form at 2400 points.
- The question of the callout stands above the board in every task.
- The fold and the answers reach the agent only through `next_click`, and the
  fold sends no click.

## 8. Cleanup

1. Close the card with the `close` operation of `block_panel`.

   Result: the card leaves the workspace.

2. Close the native VNC Device panel of the test desktop.

   Result: the viewer releases the connection. The application of the fixture
   stops with its own window.

## 9. Record of results

Put the results in the pull request, with the pictures of the wide form and the
narrow form. Keep private evidence out of the repository.
