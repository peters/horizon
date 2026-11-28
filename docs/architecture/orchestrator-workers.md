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
- An orchestrator has at most 4 workers that run at the same time. A fifth
  request is `too_many_workers` and starts nothing.
- The person closes the panel of a worker, and the record of the workers leaves
  the conversation on the pass of the dock that reads the board.

## Which programs an orchestrator can start

- The chip `Workers` under the field of the conversation holds the programs
  that the person lets this orchestrator start. The person changes it at any
  time. A new conversation holds none.
- A request for a program that the chip holds starts the worker with no press.
- A request for a program that the chip does not hold starts nothing. The
  conversation shows one row with the name of the program, the first line of
  the task and the controls `Start`, `Always for this work item` and
  `Not now`. `Start` starts this one worker. `Always for this work item` adds
  the program to the chip and starts the worker. `Not now` starts nothing. The
  row then reads `Started`, `Started; <program> is allowed for this work item`
  or `Not started`, and the orchestrator gets one line that tells it what the
  person pressed. A row that a restart loaded as open takes no press and reads
  `Not started`, because the program that asked is gone.
- A second request of a program that the person did not start is `not_allowed`:
  one work item asks the person once for each program.
- The chip reads `not chosen`, the name of one program, or `<N> programs`. A
  press on it opens a menu with one row for each agent preset of the settings
  whose program this computer has, with a check mark for each program that the
  chip holds. A press on a row adds the program or removes it and leaves the
  menu open. The escape key and a press outside the menu close it.

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
  the task is done or when it cannot continue. The whole prompt line is one line
  of the terminal: each control character of the task becomes a space.
- The answer of `worker_start` names the local id of the panel of the worker,
  and `workers` answers the local id, the preset, the kind of the program, the
  state `working` or `idle`, and the last report of each worker, cut to 400
  characters.
- Horizon types a message of the orchestrator into the terminal of the worker
  as one prompt line, as the first prompt line is typed.
- A report goes to the program of the orchestrator as one turn, through its
  bridge: the first line of the turn is
  `Report of your worker <program> (<panel id>):` and the text of the report
  follows on the next line. A report that arrives while a turn runs waits for the
  end of that turn, in the queue that the wake line of a card uses. Reports keep
  their order, at most 8 of them wait, and a ninth is `busy`.
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
| Reports of one orchestrator that wait for a turn | 8 |

A request outside a bound is refused whole and changes nothing. The refusal codes
of these operations are `invalid_request` for a text outside its bound,
`unknown_preset` for a name that the settings do not hold, `too_many_workers`,
`not_allowed`, `not_found` for a worker that this orchestrator did not start,
`too_soon`, `busy` and `caller_unavailable`.

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
- The front desk starts no worker. A panel that the person opened is no worker,
  so `worker_report` answers `caller_unavailable` in it.
- The chip of the workers is the choice of the person, and it is stored with the
  record of the conversation of the workspace. A record of an older build holds
  no field and loads with an empty chip.
- The records of the workers are state of the run: a start of Horizon holds no
  worker until its orchestrator starts one.
- Not guaranteed: that a worker does what the task says, and that a report is
  true. The person reads the panels.

## Not built yet

- A worker on a rented machine.
- A stop of a worker by the orchestrator. The person closes the panel.
