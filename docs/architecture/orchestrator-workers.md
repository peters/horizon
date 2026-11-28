# The workers of an orchestrator

An orchestrator is the program that Horizon runs for the conversation of one
workspace. This page tells how an orchestrator starts other agent programs as
its workers, how it speaks to them, how they report to it, and what stays out
of its reach.

Each agent program signs in with its own account. The orchestrator can run on
one subscription and its workers on others, so the person uses each
subscription for the work that it does best.

## What a worker is

- A worker is an agent panel in the workspace of the orchestrator. It is a
  panel as any other: the person sees it on the canvas, types into it and
  closes it.
- Horizon starts a worker from a preset of the settings of the person. The
  command, the arguments and the kind of the program come from the preset. The
  request of the orchestrator names the preset and gives the task. It names no
  command and no argument.
- The folder of a worker is the folder of the workspace. The request names no
  folder.
- The titlebar of a worker carries the mark `worker of the orchestrator`, which
  is text of Horizon.
- An orchestrator has at most 4 workers that run at the same time.

## Which programs an orchestrator can start

- The chip `Workers` under the field of the conversation holds the programs
  that the person lets this orchestrator start. The person changes it at any
  time. A new conversation holds none.
- A request for a program that the chip holds starts the worker with no press.
- A request for a program that the chip does not hold starts nothing. The
  conversation shows one row with the name of the program, the first line of
  the task and the controls `Start`, `Always for this work item` and
  `Not now`. `Start` starts this one worker. `Always for this work item` adds
  the program to the chip and starts the worker. The orchestrator gets one
  line that tells it what the person pressed.

## The operations

The orchestrator reaches these operations of the tool `block_panel` only while
the dock runs its program, and only for its own workspace.

| Operation | What it does |
|---|---|
| `worker_start` | Starts one worker from a preset with one task. |
| `workers` | Reads the workers of this orchestrator: the id, the program, the state and the last report of each. |
| `worker_message` | Sends one message to one worker of this orchestrator. |

A worker reaches one operation:

| Operation | What it does |
|---|---|
| `worker_report` | Sends one report to the orchestrator that started this worker. |

- Horizon types the task into the worker as its first prompt line, and adds one
  constant sentence that tells the worker to report with `worker_report` when
  the task is done or when it cannot continue.
- Horizon types a message of the orchestrator into the terminal of the worker
  as one prompt line, as the first prompt line is typed.
- A report goes to the program of the orchestrator as one turn, through its
  bridge. A report that arrives while a turn runs waits for the end of that
  turn.
- The conversation of the orchestrator gets one entry for each start, each
  message and each report, so the person reads what the programs said to each
  other.

## Bounds

| What | Bound |
|---|---|
| Workers of one orchestrator that run | 4 |
| A task, a message, a report | 4,000 characters |
| Messages to one worker | 1 in 10 seconds |
| Reports of one worker | 1 in 10 seconds |

A request outside a bound is refused whole and changes nothing.

## Trust boundary

- The task, each message and each report are text of a program. Horizon draws
  them as text and passes them as text. No part of them becomes a command, a
  path or an option of a program.
- A report of a worker is read by the orchestrator, and a message of the
  orchestrator is read by a worker. One program can thus influence another.
  This is the purpose of the feature, and it is limited to the programs of one
  workspace that the person let the orchestrator start. The bounds above, the
  marks on the panels and the entries of the conversation make each step
  visible.
- An orchestrator reads no screen of a panel. It reads only what a worker
  reports. A panel that is not a worker of this orchestrator is out of its
  reach: it does not message it and does not read it.
- A worker reports only to the orchestrator that started it. A panel that the
  person opened is no worker, and its report is refused.
- The front desk starts no worker.
- Not guaranteed: that a worker does what the task says, and that a report is
  true. The person reads the panels.

## Not built yet

- A worker on a rented machine.
- A stop of a worker by the orchestrator. The person closes the panel.
