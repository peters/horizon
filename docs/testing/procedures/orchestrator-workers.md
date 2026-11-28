---
procedure: orchestrator-workers
feature: Workers of an orchestrator
platforms: [linux]
cost: none
destructive: no
secrets: none
owner: peters
---

# Workers of an orchestrator test procedure

## 1. Purpose

This procedure tests the workers of the orchestrator of a workspace. It proves
that a request for a worker of a program that the chip `Workers` does not hold
starts nothing and draws the row with the three controls, that a press starts one
worker panel with the mark of Horizon, that the worker reports and that the
orchestrator reads the report, and that the chip of the workers opens the menu of
the agent programs of the settings. It also proves the look of the rows of a
worker and of a request for a worker, in the dark theme and in the light theme.

## 2. Applicability

- Candidate: a Horizon build that contains this change.
- Platforms: Linux, with the local device fixture.
- The program of the orchestrator and the program of the worker are the stand-in
  `scripts/device-smoke/orchestrator_standin.py`. It answers in the line format
  of Claude Code and it calls the MCP server of this Horizon, so no real agent
  program and no paid account take part.
- This procedure does not test: a real agent program as a worker, a worker on a
  rented machine, and the stop of a worker by an orchestrator.

## 3. Safety

> **CAUTION:** USE ONLY THE ISOLATED DESKTOP. A click on the developer desktop changes a real board.

## 4. Equipment and preconditions

- A frozen candidate and its SHA-256.
- The local device fixture with `--native-view`.
- A live Device panel in the current workspace.
- An evidence folder for the run.
- A presets file for the fixture with a preset of an agent kind whose command is
  the stand-in of the worker. Write it as `workers-presets.json`. The first
  preset is the program that the orchestrator asks for, and each preset is a row
  of the chip `Workers`:

  ```json
  [{"name": "Gemini", "kind": "gemini",
    "command": "<repo>/scripts/device-smoke/orchestrator_standin.py",
    "args": ["--worker"]},
   {"name": "Pi", "kind": "pi",
    "command": "<repo>/scripts/device-smoke/orchestrator_standin.py",
    "args": ["--worker"]},
   {"name": "Codex", "kind": "codex",
    "command": "<repo>/scripts/device-smoke/orchestrator_standin.py",
    "args": ["--worker"]}]
  ```

  The first preset is the kind `gemini`, because a panel of that kind carries no
  session badge: the titlebar holds the mark `opened by worker of the orchestrator`
  beside it. A panel of the kind `claude`, `codex` or `pi` is the same panel, and a
  narrow titlebar drops its mark.

## 5. Setup

1. Copy the candidate to a task-owned folder. Write down its SHA-256.

   Result: The candidate does not change during the run.

2. Start the fixture with the switch of the dock on, the stand-in as the program
   of the orchestrator, and the presets file of section 4.

   ```sh
   HORIZON_ORCHESTRATOR_DOCK=1 \
   HORIZON_ORCHESTRATOR_PROGRAM=<repo>/scripts/device-smoke/orchestrator_standin.py \
     python3 scripts/device-smoke/serve.py \
       --horizon <candidate>/horizon --native-view --state <evidence>/workers \
       --presets workers-presets.json
   ```

   Result: The manifest prints an address for the native view, and the dock
   stands at the right edge of the Horizon window.

3. Open a Device panel on that address in the current workspace.

   Result: The live view shows the isolated desktop. The frame sequence advances.

4. Press the row of the workspace in the sidebar, so that the dock shows the
   conversation of that workspace.

   Result: The header of the dock names the workspace, and the composer stands at
   its bottom edge with the three chips `Orchestrator`, `Workers` and `Place`.
   The chip `Workers` reads `not chosen`.

## 6. Tasks

### 6.1 OW-1 — A request for a worker of a program outside the chip asks the person

1. Write `start a worker` in the field of the composer, and press the send arrow.

   Result: The conversation of the workspace holds the text `You said: start a
   worker` and, under it, the answer `I asked for a worker.`.

2. Read the row of the request under the answer.

   Result: The row holds the caption `The orchestrator wants to start a worker`,
   the name `Gemini` at 12.5 semibold, the first line of the task `Say
   hello and report.` at 11.5 in the soft colour, and the three controls `Start`,
   `Always for this work item` and `Not now` on one line, 28 points high and 8
   points apart. `Start` carries the accent colour.

3. Press `Start`.

   Result: A new panel `Gemini` stands on the canvas beside the panels of
   the workspace, it does not take the keyboard, and its titlebar carries the mark
   `worker of the orchestrator`. The row of the request reads `Started` and draws
   no control. The conversation holds the row of the worker, with the word
   `worker` at its right end.

4. Wait until the panel of the worker printed `The worker reports.` and `The
   worker read: Say hello and report. When the task is done, or when you cannot
   continue, call the block_panel tool with operation worker_report and one text
   that says what you did.`

   Result: The conversation holds the caption `Gemini reports` and, under
   it, the text `hello from the worker` with the bar of an agent text. The answer
   of the orchestrator reads `I read the report: hello from the worker`.

5. Press the row of the worker in the conversation.

   Result: The canvas brings the panel of the worker into view. The press sends
   nothing to a program.

### 6.2 OW-2 — The chip of the workers holds the programs of the person

1. Press the chip `Workers` of the composer.

   Result: A menu opens under the chip with one row for each agent preset of the
   settings whose program this computer has. The rows `Gemini`, `Pi` and `Codex`
   draw no check mark.

2. Press the row `Gemini`.

   Result: The row carries the check mark `✓`, the menu stays open, and the value
   line of the chip reads `Gemini`.

3. Press the same row again.

   Result: The check mark leaves the row, and the value line of the chip reads
   `not chosen`.

4. Press the row `Gemini` one more time, then press the escape key.

   Result: The menu closes, and the chip keeps the program.

5. Write `start a worker` in the field, and press the send arrow.

   Result: The worker starts with no row of a request, because the chip holds the
   program. The conversation holds the row of the worker and the report after it.

### 6.3 OW-3 — The refusals of the workers

1. Write `start a worker` in the field and send it, and press `Not now` on the row
   of the request, with a chip that holds no program.

   Result: The row reads `Not started`, no panel opens, and the answer of the
   orchestrator is one line that says that the person did not start the worker of
   that request.

2. Write `start a worker` again and send it.

   Result: The conversation keeps one row of a request, which reads `Not started`,
   and the answer of the orchestrator names the refusal `not_allowed`. No second
   row appears and no panel opens.

3. Start four workers of the chip, and send `start a worker` a fifth time.

   Result: Four panels of a worker stand in the workspace, and the answer names
   `too_many_workers`. No fifth panel appears.

4. Close one panel of a worker with a press on its close control.

   Result: The row of that worker in the conversation reads dim with `closed` at
   its right end, and a press on it does nothing. The read of the workers of the
   orchestrator names the three panels that stay.

### 6.4 OW-4 — The same rows in the light theme

1. Set the theme to light with the palette, or start the fixture with
   `--theme light`, and repeat OW-1.

   Result: Every row of OW-1 keeps its place and its size, and its text keeps a
   contrast of at least 4.5:1 against its fill. The accent box of the request, the
   accent control `Start`, the bar of the report and the check mark of the menu are
   readable.

## 7. Pass criteria

- OW-1, OW-2, OW-3 and OW-4 pass at every step.
- No frame takes the keyboard from the panel of the person.
- The conversation names each start, each message and each report of a worker.

## 8. Cleanup

1. Close the panel of each worker, and press `Stop` if the conversation of a
   workspace runs a turn.

   Result: The workspace holds the panels that the run started with.

2. Close the Device panel of the run.

   Result: The live view ends. The fixture keeps no process of this run.

## 9. Record of results

If the run must be kept, write a report in `docs/testing/reports/` with
[the report template](../reports/TEMPLATE.md). If not, put the results in the
pull request. Keep private evidence out of the repository.
