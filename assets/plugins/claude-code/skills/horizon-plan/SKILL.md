---
name: horizon-plan description: >-
  Do one task of the person on one live Horizon card, as a short path for a task of one todo or as a
  plan of three to twelve todos that you do yourself or give to helper agents on this computer or on a
  machine of the machine list, and keep the card true with the state, the place, the evidence and the
  history of each todo. Use it when the person describes a task or names a GitHub issue and wants the
  work done, or when the person asks to see the plan of a task.
---

# One task on one card

The person describes a task, or names a GitHub issue. You choose the path, you
draw the task as one card, and you do the todos yourself, or you give them to
helper agents on this computer and on the machines of the machine list. The card
stays the one place where the person reads the task: each todo with its state,
its place, its evidence, and the history of the plan.

The card contract is in `horizon-block`: the document of a card, its operations,
`next_click`, `open_terminal`, `open_panel` and the machine records. The
worktrees of a repository are in `horizon-worktree`. This skill adds the path,
the plan file and the rules of the work.

## Choose the path

Use the **short path** when the task is one todo: one change, one command or one
answer, such as `fix #123`, `rename this`, or `why does this test fail`. A task
of two todos takes the short path too.

Use the **full plan** when the task has three or more separate todos, when it
uses more than one agent or more than one machine, or when the person asks for a
plan.

Both paths write the same plan file and hold the same evidence. The short path
has one todo, no `Go` question and one small card.

## The short path

Start at once. Do not draw a plan of three todos and do not ask for a go. Do not
ask a question before the work: do the todo, and disturb the person only when
the decision is theirs, as `Disturb the person only when it is theirs` says.

Draw one card with a `heading` of the task and the place, one state line, and
one `buttons` row. Change it when the todo ends, with the result and its
evidence:

```json
{"components": [
  {"type": "heading", "text": "Fix the login redirect",
   "subtitle": "Issue #1987 · this computer", "badge": {"text": "Running", "tone": "info"}},
  {"type": "progress", "label": "0 of 1 done · 0 needs you", "percent": 0,
   "detail": "t1 now"},
  {"type": "board",
   "columns": [{"label": "Change", "tag": "1", "value": "~20 m", "current": true}],
   "rows": [{"label": "coordinator · this computer"}],
   "items": [{"column": 0, "row": 0, "title": "Fix the login redirect",
              "state": "now", "value": "~20 m",
              "note": "check: cargo test -p auth redirect"}]},
  {"type": "caption", "text": "AGENTS.md · 1 at once · this computer · rent: no"},
  {"type": "buttons", "items": [{"id": "stop", "label": "Stop", "tone": "danger"}]}
]}
```

```json
{"components": [
  {"type": "heading", "text": "Fix the login redirect",
   "subtitle": "Issue #1987 · this computer", "badge": {"text": "Done", "tone": "ok"}},
  {"type": "progress", "label": "1 of 1 done · 0 needs you", "percent": 100,
   "detail": "done"},
  {"type": "board",
   "columns": [{"label": "Change", "tag": "1", "value": "18 m"}],
   "rows": [{"label": "coordinator · this computer"}],
   "items": [{"column": 0, "row": 0, "title": "Fix the login redirect",
              "state": "done", "value": "18 m", "note": "12 tests pass"}]},
  {"type": "log", "entries": [
    {"at": "07:28", "text": "t1 done: 12 tests pass"}]},
  {"type": "caption", "text": "AGENTS.md · 1 at once · this computer · rent: no"},
  {"type": "buttons", "items": [{"id": "close", "label": "Close"}]}
]}
```

A short path that cannot finish ends as a full plan does: the state `stopped`,
one line of what is done and what is left, and one question.

## The plan file

The plan file is the one source of the task. You draw the card from it, and a
coordinator that starts again reads it and continues. It is a JSON file of one
fixed shape, and `history.jsonl` beside it holds the events.

| Task | The plan folder |
|---|---|
| The task has a repository | `<worktree>/.horizon-plan/` |
| The task has no repository | `~/.horizon/plans/<slug>/` |

`<slug>` is the task in letters, digits and `-`, at most 40 characters, for
example `fix-login-redirect`.

The plan folder of a repository task is inside the worktree and git ignores it.
Do this before the first write:

1. `git check-ignore -q .horizon-plan/plan.json` answers `0` when git already
   ignores the folder. Continue with step 3.
2. When it answers another code, append the line `.horizon-plan/` to the exclude
   file of this clone:

   ```sh
   printf '%s\n' '.horizon-plan/' >> "$(git rev-parse --git-path info/exclude)"
   ```

   That file is local to the clone and no commit holds it. In a linked worktree,
   `git rev-parse --git-path info/exclude` names the exclude file of the main
   clone.
3. `git status --porcelain` names no file of the plan folder.

Never run `git add` on the plan folder, never commit it, and never write the
plan file of a task into another folder. The plan file is a record of the work,
not a file of the product.

### The shape

```json
{
  "plan_version": 1,
  "task": "Fix the login redirect",
  "issue": "example-org/example-repo#1987",
  "state": "running",
  "created_at": "2026-11-14T07:10:00Z",
  "updated_at": "2026-11-14T07:42:11Z",
  "card_panel_id": "<the panel_id that create answered>",
  "helpers": [
    {"name": "helper-1", "where": "this computer"},
    {"name": "helper-2", "where": "build-1",
     "command": "<an agent program on that machine>"}
  ],
  "waves": [
    {"name": "Understand"},
    {"name": "Change"}
  ],
  "todos": [
    {"id": "t1",
     "title": "Read the issue and the failing test",
     "who": "coordinator",
     "where": "this computer",
     "wave": 1,
     "state": "done",
     "added": false,
     "check": "cargo test -p horizon-ui login_redirect",
     "check_result": "pass",
     "evidence": {"kind": "check",
                  "command": "cargo test -p horizon-ui login_redirect",
                  "result": "12 tests pass"},
     "started_at": "2026-11-14T07:10:30Z",
     "ended_at": "2026-11-14T07:14:02Z",
     "progress": "progress-t1.md"}
  ],
  "question": null
}
```

- `plan_version` is `1`. `task` is one line of at most 120 characters. `issue` is
  `<owner>/<repository>#<number>`, or `null` when the task is no issue.
- `state` is `plan` while the card asks for a go, `running`, `stopped` or `done`.
  The short path starts at `running`.
- `created_at`, `updated_at`, `started_at` and `ended_at` are ISO 8601 times in
  UTC, or `null` when the event did not happen.
- `card_panel_id` is the `panel_id` of the card of this plan, or `null` before
  the card exists.
- `helpers` holds one entry per helper agent, with its name, its place and, on a
  machine, the command of its agent program.
- `waves` holds 1 to 8 entries, in the order they run, each with a `name` of at
  most 20 characters such as `Understand` or `Change`. The wave of one row runs
  before the next one: a wave starts when every todo of the wave before it is
  done.
- `todos` holds 1 to 12 entries. `id` is `t1` to `t12`. `who` is `coordinator`,
  a name of `helpers`, or `you` for a todo that waits for the person. `where` is
  `this computer` or the name of a machine. `state` is `waiting`, `now`, `done`,
  `needs_you` or `failed`. `added` is `true` for a todo that came after the go.
  `check` is the one command that proves the todo, and it is empty only when the
  todo is a question to the person. `check_result` is `pass`, `fail` or `null`.
  `evidence` is one entry, as `Evidence` says, or `null`. `progress` is the file
  name of the progress note of the todo, beside the plan file.
- `question` is `null`, or one object with `todo`, `text` and `answers`. Each
  answer has an `id` of `answer.<todo>.<choice>` and a `label`.

Only you write `plan.json`, and one coordinator writes it: a helper never does.
A helper writes only its own progress note. Write the whole file at each change,
and write it before you draw the card that reports the change.

### What survives, and what does not

| Event | The files | The card | The helpers |
|---|---|---|---|
| The agent program of the coordinator restarts in its panel | they stay | the agent of that panel owns it again | A helper on a machine keeps its session; a helper panel is the person's |
| Horizon restarts | they stay | the document comes back with its panel | A session of a machine survives; a helper panel starts again from its preset |
| The person's other computer opens the task | only when the plan folder is there | no: a card belongs to one Horizon | no: a session of a machine belongs to one computer |
| The person closes the panel of the coordinator | they stay | the card stays, its rows and buttons are disabled, and each badge reads `Last seen: ` | A helper on a machine keeps running; the person closes a helper panel |

The clicks that wait and the selected row are not saved, and neither is the
output of a helper between two notes: the files hold the plan, the history, the
notes and the work.

## The policy of the repository

Before the plan, read `AGENTS.md` of the repository of the task for the section
`## Horizon plan`:

- `at_once`: how many todos may run at once.
- `places`: where work may run: `this computer`, names of machines of the
  machine list, or `rent a machine`.
- `checks`: the checks that every todo must pass.

A place that the section does not name is out of the plan. When the file holds
no such section, two todos run at once, work runs on this computer only, and you
rent nothing. Whatever the section says, a rented machine needs a go of the
person on a card that states its price.

A todo whose place is `rent a machine` uses the recipe of that repository: the
section `## Horizon machine` of `AGENTS.md`, read with
`references/repository-machine.md` of `horizon-machines`. The recipe gives the
resources, the tools, the warm-up and the check of the machine, and a helper on
that machine works in the clone that the recipe brought. A repository without
that section gets the one question of that page before any machine is rented.

When the section is absent, give this block to the person to paste:

```markdown
## Horizon plan

- at_once: 2
- places: this computer
- checks: cargo fmt --all -- --check, cargo test --workspace
```

## The card

Make one card at the start, titled with the task, keep the `panel_id` that
`create` answers, and change the card with `update`. One agent owns at most eight
cards in one workspace.

One task has one card, at every size. The short path draws the same card with a
board of one column and one row, and a task that grows to a plan updates that
same card to the wide document. Never make a second card for the same task.

A plan of three or more todos asks for the wide card: `"width": "wide"` beside
`components`. A task of one or two todos keeps the narrow card of today. The form
follows the real width of the card, so the same document draws the board as a
grid on a wide card and as one list on a narrow one.

The parts of the card, in this order:

1. `heading`: `text` is the task. `subtitle` is the issue, when the task has one,
   and the branch or the place, for example
   `Issue #1987 · example-org/shop · branch fix-login-redirect`. `badge` is the
   state of the plan: `Plan`, `Running`, `Stopped` or `Done`.
2. `progress`: `label` is the summary line, `percent` is the share of the todos
   that are done, and `detail` names the time and the todo that runs now, for
   example `27 min so far · about 30 left`.
3. One `callout`, and only one. Before the go it has the tone `neutral`, the
   `caption` `Nothing starts before Go` and a `text` that says what starts and
   what it costs, such as `8 todos in 5 waves, on 3 computers. Nothing is
   rented.`; its answers are `Go` with tone `primary`, `Change the plan` and
   `Stop` with tone `danger`. While the plan runs and a todo needs the person it
   has the tone `warn`, the `caption` `Needs you · t5 · waits 3 min`, the
   question as `text` and the answers of that question. In every other state the
   card holds no callout at all.

   The question is never under the board and never below the fold: it is the
   strip above the board. Before the go the callout holds those three buttons,
   and the card holds no other row of buttons: an id belongs to one button.
4. `board`: the waves as columns and the workers as rows, as `The board of a
   plan` says.
5. `log`: the last five events of `history.jsonl`, each with its time, the newest
   first. The `log` of every update holds the last five events, and the update at
   the end of the plan holds them too: a done card whose `log` holds one line
   while the history holds six tells the person that five events never happened.
   When the plan has six events, the card shows five of them, and the rest stays
   in `history.jsonl`.
6. `caption`: the policy of the repository in one line, such as
   `AGENTS.md · 2 at once · this computer, build-1 · rent: ask`.
7. One `buttons` row: `Change the plan`, `Stop` with tone `danger`, and `Show
   helper terminals` with the id `terminal.helper-1` while a helper runs in a
   session that `open_terminal` can show.

`Show helper terminals` is the only way that a helper terminal opens: never call
`open_terminal` while the plan runs, and never leave the person to find the
session by hand. When the person presses it, open the terminal of each helper
that runs in a session and answer the press with the panel ids.

### The board of a plan

A column is one wave, in the order of the plan file. A row is one worker: the
person who does the work and the place where it runs. Every todo names its wave
and its worker, so one item of the board shows one todo at the place where it
runs and the wave that runs it. The waves of one row run one after the other;
the todos of one wave may run at once, as far as `at_once` allows.

Write the columns from `waves` of the plan file:

- `label` is the name of the wave, such as `Understand`.
- `tag` is the number of the wave, from `1` to `8`.
- `note` is `in parallel` when the wave holds more than one todo.
- `value` is the estimate of the wave, such as `~9 m`, or its time when the wave
  is done, such as `9 m`.
- `current` is `true` for the wave of the todo that runs now, and for no other
  column.

Every update sends every column with its `tag` and its `value`, and every item
with its `value`. Never draw a board of the running plan whose columns hold no
tag or no value, and never send an item without one: a column that loses its tag
loses the number of its wave, and a column or an item that loses its value loses
the one fact that the person reads a board for, which is how long the work takes.
A wave keeps its `tag` for the whole plan. Its `value` starts as the estimate
with `~` in front while the wave waits, and it becomes the real time, without the
`~`, as soon as the wave finished.

Write the rows from the todos, one row for each `who` and `where` that a todo
names, in this order: the coordinator, each helper in the order of `helpers`, and
`You` last when a todo waits for the person.

- `label` is `<who> · <where>`, such as `helper-1 · build-1`. The place stays in
  the label, because the narrow card draws only the label with the evidence.
- `subtitle` holds one fact that the label does not: the kind of the agent
  program in neutral words, such as `another agent program`, the size of the
  machine, such as `16 vCPU`, or `you answer on this card` for the person. A row
  whose label already holds every fact keeps no subtitle.

Write one item for each todo:

- `column` is the number of its wave minus one, and `row` is the index of its
  worker in `rows`.
- `title` is the todo.
- `state` is the state of the todo in the plan file: `waiting`, `now`, `done`,
  `needs_you` or `failed`.
- `value` is the time that the plan file measured, such as `8 m`, or the estimate
  with `~` in front while the todo waits, such as `~5 m`, or `waits 3 m` for a
  todo that waits for the person.
- `note` is the evidence of the todo in one line, such as `12 tests pass` or
  `check: cargo test --workspace`.
- `tag` is `added` for a todo that came after the go; every other item keeps no
  tag.

A cell of the board holds at most four items. When one wave holds more todos of
one worker than that, split the wave in the plan file; never drop a todo.

### The bounds of a card

A card holds 64 components, 8 columns and 8 rows of one `board` with 32 items and
4 items in one cell, 4 answers in one `callout`, 16 entries in one `log`, 8
buttons in one `buttons`, 24 rows in one `kv`, 512 characters in one text field
and 32 KiB of serialized document. One board holds one current column, and every
item names a cell that its columns and rows hold.

A plan of twelve todos in eight waves on five rows, with a question and five
history lines, fits: 7 components of 64, 12 items of 32, 8 waves of 8, 5 rows of
8, one item in each of its cells, 3 answers of 4 and 5 entries of 16.

### The cards of 3, 8 and 12 todos

The plan, before the go:

```json
{"width": "wide", "components": [
  {"type": "heading", "text": "Fix the login redirect",
   "subtitle": "Issue #1987 · this computer", "badge": {"text": "Plan", "tone": "info"}},
  {"type": "progress", "label": "0 of 3 done · 0 needs you", "percent": 0,
   "detail": "no todo runs"},
  {"type": "callout", "caption": "Nothing starts before Go",
   "text": "3 todos in 2 waves, on 2 computers. Nothing is rented.",
   "buttons": [
     {"id": "go", "label": "Go", "tone": "primary"},
     {"id": "change", "label": "Change the plan"},
     {"id": "stop", "label": "Stop", "tone": "danger"}]},
  {"type": "board",
   "columns": [
     {"label": "Understand", "tag": "1", "value": "~12 m"},
     {"label": "Change", "tag": "2", "note": "in parallel", "value": "~35 m"}],
   "rows": [
     {"label": "coordinator · this computer"},
     {"label": "helper-1 · build-1", "subtitle": "16 vCPU"}],
   "items": [
     {"column": 0, "row": 0, "title": "Read the issue and the failing test",
      "value": "~12 m", "note": "check: cargo test -p horizon-ui login_redirect"},
     {"column": 1, "row": 1, "title": "Change the redirect target",
      "value": "~20 m", "note": "check: cargo test -p horizon-ui"},
     {"column": 1, "row": 0, "title": "Run the test and the full check",
      "value": "~15 m", "note": "check: cargo test --workspace"}]},
  {"type": "caption", "text": "AGENTS.md · 2 at once · this computer, build-1 · rent: no"}
]}
```

Eight todos, three done under the policy, one added, one that needs the person,
and the history:

```json
{"width": "wide", "components": [
  {"type": "heading", "text": "Fix the login redirect",
   "subtitle": "Issue #1987 · example-org/shop · branch fix-login-redirect",
   "badge": {"text": "Running", "tone": "info"}},
  {"type": "progress", "label": "3 of 8 done · 1 needs you", "percent": 38,
   "detail": "27 min so far · about 30 left"},
  {"type": "callout", "tone": "warn", "caption": "Needs you · t5 · waits 3 min",
   "text": "Which address must the redirect use?",
   "buttons": [
     {"id": "answer.t5.account", "label": "The account address", "tone": "primary"},
     {"id": "answer.t5.last", "label": "The last address"},
     {"id": "answer.t5.skip", "label": "Skip this todo"}]},
  {"type": "board",
   "columns": [
     {"label": "Understand", "tag": "1", "value": "4 m"},
     {"label": "Prepare", "tag": "2", "note": "in parallel", "value": "~9 m"},
     {"label": "Change", "tag": "3", "note": "in parallel", "value": "~10 m", "current": true},
     {"label": "Verify", "tag": "4", "value": "~20 m"},
     {"label": "Report", "tag": "5", "value": "~5 m"}],
   "rows": [
     {"label": "coordinator · this computer"},
     {"label": "helper-1 · build-1", "subtitle": "16 vCPU"},
     {"label": "helper-2 · build-2", "subtitle": "32 vCPU"},
     {"label": "You · this computer", "subtitle": "you answer on this card"}],
   "items": [
     {"column": 0, "row": 0, "title": "Read the issue and the failing test",
      "state": "done", "value": "4 m", "note": "1 failing test found"},
     {"column": 1, "row": 1, "title": "Add the redirect case to the test",
      "state": "done", "value": "6 m", "note": "12 tests pass"},
     {"column": 1, "row": 2, "title": "Check the route table",
      "state": "done", "value": "9 m", "note": "9 rows, 1 wrong"},
     {"column": 2, "row": 1, "title": "Change the redirect target",
      "state": "now", "value": "8 m", "note": "edits routes/login.rs"},
     {"column": 2, "row": 3, "title": "Choose the address of the redirect",
      "state": "needs_you", "value": "waits 3 m", "note": "t4 goes on with the default"},
     {"column": 2, "row": 0, "title": "Update the route notes",
      "state": "waiting", "value": "~5 m", "note": "check: docs build"},
     {"column": 3, "row": 1, "title": "Run the full check",
      "state": "waiting", "value": "~20 m", "note": "check: cargo test --workspace"},
     {"column": 4, "row": 0, "title": "Write the report of the task",
      "state": "waiting", "value": "~5 m", "note": "check: 3 lines and the evidence", "tag": "added"}]},
  {"type": "log", "entries": [
    {"at": "07:31", "text": "t3 done on build-2, 9 rows"},
    {"at": "07:28", "text": "t5 waits for you: the address is your choice"},
    {"at": "07:26", "text": "t2 done on build-1, 12 tests pass"},
    {"at": "07:20", "text": "t8 added: the report was missing"},
    {"at": "07:10", "text": "You pressed Go"}]},
  {"type": "caption", "text": "AGENTS.md · 2 at once · this computer, build-1, build-2 · rent: ask"},
  {"type": "buttons", "items": [
    {"id": "change", "label": "Change the plan"},
    {"id": "stop", "label": "Stop", "tone": "danger"},
    {"id": "terminal.helper-1", "label": "Show helper terminals"}]}
]}
```

Twelve todos in eight waves on five rows, eleven done, one that needs the
person, and the history:

```json
{"width": "wide", "components": [
  {"type": "heading", "text": "Fix the login redirect",
   "subtitle": "Issue #1987 · example-org/shop · branch fix-login-redirect",
   "badge": {"text": "Running", "tone": "info"}},
  {"type": "progress", "label": "11 of 12 done · 1 needs you", "percent": 92,
   "detail": "52 min so far · your answer is left"},
  {"type": "callout", "tone": "warn", "caption": "Needs you · t12 · waits 2 min",
   "text": "May I push the branch fix-login-redirect and open a pull request?",
   "buttons": [
     {"id": "answer.t12.push", "label": "Push and open the PR", "tone": "primary"},
     {"id": "answer.t12.wait", "label": "Not now"}]},
  {"type": "board",
   "columns": [
     {"label": "Understand", "tag": "1", "value": "4 m"},
     {"label": "Prepare", "tag": "2", "note": "in parallel", "value": "9 m"},
     {"label": "Change", "tag": "3", "note": "in parallel", "value": "13 m"},
     {"label": "Fix", "tag": "4", "value": "7 m"},
     {"label": "Verify", "tag": "5", "note": "in parallel", "value": "24 m"},
     {"label": "Review", "tag": "6", "note": "in parallel", "value": "9 m"},
     {"label": "Report", "tag": "7", "value": "5 m"},
     {"label": "Push", "tag": "8", "value": "waits 2 m", "current": true}],
   "rows": [
     {"label": "coordinator · this computer"},
     {"label": "helper-1 · build-1", "subtitle": "16 vCPU"},
     {"label": "helper-2 · build-2", "subtitle": "32 vCPU"},
     {"label": "helper-3 · build-1", "subtitle": "another agent program"},
     {"label": "You · this computer", "subtitle": "you answer on this card"}],
   "items": [
     {"column": 0, "row": 0, "title": "Read the issue and the failing test",
      "state": "done", "value": "4 m", "note": "1 failing test found"},
     {"column": 1, "row": 1, "title": "Add the redirect case to the test",
      "state": "done", "value": "6 m", "note": "12 tests pass"},
     {"column": 1, "row": 2, "title": "Check the route table",
      "state": "done", "value": "3 m", "note": "9 rows, 1 wrong"},
     {"column": 2, "row": 1, "title": "Change the redirect target",
      "state": "done", "value": "8 m", "note": "12 tests pass"},
     {"column": 2, "row": 2, "title": "Update the route notes",
      "state": "done", "value": "5 m", "note": "the notes name one address"},
     {"column": 3, "row": 3, "title": "Fix the two findings of the review",
      "state": "done", "value": "7 m", "note": "412 tests pass"},
     {"column": 4, "row": 1, "title": "Run the full check",
      "state": "done", "value": "20 m", "note": "412 tests pass"},
     {"column": 4, "row": 0, "title": "Write the report of the task",
      "state": "done", "value": "4 m", "note": "4 lines in the PR", "tag": "added"},
     {"column": 5, "row": 2, "title": "Read the review of the change",
      "state": "done", "value": "3 m", "note": "2 findings"},
     {"column": 5, "row": 0, "title": "Check the two findings",
      "state": "done", "value": "6 m", "note": "412 tests pass"},
     {"column": 6, "row": 0, "title": "Write the report of the task",
      "state": "done", "value": "5 m", "note": "the report is in the PR"},
     {"column": 7, "row": 4, "title": "Name the branch of the push",
      "state": "needs_you", "value": "waits 2 m", "note": "your push is your own choice"}]},
  {"type": "log", "entries": [
    {"at": "09:12", "text": "t12 waits: a push is the person's"},
    {"at": "09:08", "text": "t11 done on build-1, 412 tests pass"},
    {"at": "09:01", "text": "t10 done, the two findings are checked"},
    {"at": "08:55", "text": "t9 done, 2 findings"},
    {"at": "08:40", "text": "t8 added by the person in the agent panel"}]},
  {"type": "caption", "text": "AGENTS.md · 2 at once · this computer, build-1, build-2 · rent: ask"},
  {"type": "buttons", "items": [
    {"id": "change", "label": "Change the plan"},
    {"id": "stop", "label": "Stop", "tone": "danger"},
    {"id": "terminal.helper-1", "label": "Show helper terminals"}]}
]}
```

The done card is the same card with the badge `Done`, no callout, the summary
line at 100 percent, every item `done`, the log of the last five events, and one
`buttons` row with `Close` and `Show helper terminals`.

### How often you change the card

Draw the card after each change of the state of a todo, and at most one time in
ten seconds. When two changes fall inside ten seconds, keep the later state in
the plan file and draw the card after the ten seconds. Never draw the same
document twice, never draw on a timer, and never redraw a card that did not
change: a card that Horizon draws costs the person a frame, and a loop that
redraws an unchanged card costs the person every frame.

## The go

1. Write `plan.json` with the state `plan` and the todos in the order of the
   work.
2. Draw the card of the plan with the buttons `Go`, `Change the plan` and
   `Stop`, and state the place of each todo and the policy of the repository.
3. Call `next_click` with no `panel_id` and `timeout_millis: 300000`, and leave
   out every other field.
4. On `go`, set the state of the plan to `running` and start the first todo.
   Start no more todos at once than `at_once` allows.
5. On `change`, read the words of the person in the panel, rewrite the plan, and
   draw it again. The go is not given for the old plan.
6. On `stop`, set the state of the plan to `stopped` and say on the card what is
   left.
7. On `timed_out`, call `next_click` again while the card matters.

A press of `go` is the only permission to run a command, to write a file of the
task and to start a helper. Say that on the card of the plan.

## Do a todo

For each todo, in this order:

1. Set its state to `now` and its `started_at` to the current time. Write the
   plan file and draw the card.
2. Do the work yourself, or give it to a helper. 3. Run the check of the todo,
and the checks that the policy of the repository
   names, or read the result of the check that the helper ran.
4. Write one `evidence` entry. Set `check_result`, the state `done` or `failed`,
   and `ended_at`. Write the plan file, append one line to `history.jsonl`, and
   draw the card.

A todo is done when its own check passed and one evidence entry is in the plan
file. It is not done because a helper says so, and not because the work looks
finished.

When a todo fails, set its state to `failed`, write one line in the progress
note with the reason, and append it to the history. Draw it on the card. Go on
with the todos that do not depend on it, and say on the card what the failure
leaves undone.

## Evidence

`evidence` is one entry per done todo, and the card shows its one line:

```json
{"kind": "check", "command": "cargo test -p horizon-ui login_redirect",
 "result": "12 tests pass"}
```

- `kind: "check"` needs `command` and `result`, one line of the command and one
  line of its result. A check with no result line is no evidence.
- `kind: "picture"` and `kind: "recording"` need `path` of a file inside the plan
  folder, such as `.horizon-plan/shot-t4.png`. A card draws no image and opens no
  file: its row names the path, and the person opens the file. Name the folder of
  the plan in the PR or in the report, so a file stays findable.
- `kind: "summary"` needs `text` of at most three lines, for a todo whose result is
  a decision and not a command.

## Disturb the person only when it is theirs

Every question to the person goes on the plan card, as the `callout` of the
`Needs you` state, and the answer arrives as a press of one of its buttons. Never
ask in the question box of your agent program: the card is the one place where
the person reads the task, and the terminal of the coordinator can be off the
view, in another workspace or under other panels. A question in that terminal can
wait an hour while the card says that the plan runs. A helper follows the same
rule and writes its question into its progress note, where the coordinator reads
it and draws it on the card.

After the go, ask nothing between the todos. The coordinator decides every other
question itself and writes the decision into the history. A row of state
`needs_you` is allowed for one of these decisions only:

- money: a rented machine, a paid service, a cost;
- a deletion: a branch, a file, a machine, a store;
- a publication: a push, a pull request, a comment, a message to other people;
- a credential, a token or a login;
- a choice between two behaviours of the product.

Everything else goes into the history as one line, such as `t6: the shorter line
won, no question`.

A plan that cannot reach its goal stops with the state `stopped`. Its callout
says why and asks the one question, its log says what is done and what is left,
and its items keep the state that the plan file gives them:

```json
{"width": "wide", "components": [
  {"type": "heading", "text": "Fix the login redirect",
   "subtitle": "Issue #1987 · this computer", "badge": {"text": "Stopped", "tone": "warn"}},
  {"type": "progress", "label": "3 of 8 done · 1 needs you", "percent": 38,
   "detail": "stopped at t5"},
  {"type": "callout", "tone": "warn", "caption": "Needs you · t5 · stopped",
   "text": "The helper of t4 on build-1 stopped with the link. Continue on this computer?",
   "buttons": [
     {"id": "resume", "label": "Continue on this computer", "tone": "primary"},
     {"id": "answer.t5.account", "label": "The account address"},
     {"id": "answer.t5.last", "label": "The last address"}]},
  {"type": "board",
   "columns": [
     {"label": "Understand", "tag": "1", "value": "4 m"},
     {"label": "Prepare", "tag": "2", "note": "in parallel", "value": "9 m"},
     {"label": "Change", "tag": "3", "note": "in parallel", "value": "~10 m"}],
   "rows": [
     {"label": "coordinator · this computer"},
     {"label": "helper-1 · build-1", "subtitle": "16 vCPU"}],
   "items": [
     {"column": 0, "row": 0, "title": "Read the issue and the failing test",
      "state": "done", "value": "4 m", "note": "1 failing test found"},
     {"column": 1, "row": 1, "title": "Add the redirect case to the test",
      "state": "done", "value": "6 m", "note": "12 tests pass"},
     {"column": 1, "row": 0, "title": "Check the route table",
      "state": "done", "value": "3 m", "note": "9 rows, 1 wrong"},
     {"column": 2, "row": 1, "title": "Change the redirect target",
      "state": "failed", "value": "3 m", "note": "the link to build-1 stopped"}]},
  {"type": "log", "entries": [
    {"at": "08:02", "text": "t4 failed: the link to build-1 stopped"},
    {"at": "07:58", "text": "left: t5 to t8, and no test chooses the address"},
    {"at": "07:52", "text": "t3 done on this computer, 9 rows, 1 wrong"},
    {"at": "07:48", "text": "t2 done on build-1, 12 tests pass"},
    {"at": "07:40", "text": "You pressed Go"}]},
  {"type": "caption", "text": "AGENTS.md · 2 at once · this computer, build-1 · rent: no"},
  {"type": "buttons", "items": [
    {"id": "change", "label": "Change the plan"},
    {"id": "stop", "label": "Stop", "tone": "danger"}]}
]}
```

## Add a todo while the plan runs

A plan holds at most twelve todos. Add one when the work shows a missing step,
or when the person asks for it in the agent panel. Mark it with `added: true`,
give it the next free id, and start its subtitle with `added`.

A new todo needs a go only when it changes the goal of the task, costs money or
publishes something. Any other new todo starts at once, and the history holds
the line that says who added it and why.

## The history

The plan folder holds `history.jsonl`, one event per line:

```json
{"at": "2026-11-14T07:20:11Z", "todo": "t8", "event": "added by the person in the agent panel: the report was missing", "answer": null}
```

- `at` is the time of the event, `todo` is the id of the todo or `plan`, and
  `event` is one line of what happened.
- `answer` is what the person answered, or `null`. A line of a question carries
  the answer in a second line with the same `todo`.

Append one line at each change of a todo, at each question and at each answer.
Never rewrite a line. The card shows the last five lines as one `log`, newest
first, with the clock time of the event and its words in one line: `07:31` and
`t3 done on build-2, 9 rows`.

### The time of the card is this computer's time

`at` of a log entry, and every other time that the card draws, is the local time
of this computer, such as `07:31` or `27 min so far`. Read the clock with one
command before you write a time:

```sh
date +%H:%M
```

Never write a time in UTC on the card because the plan file holds it in UTC: a
person reads the card beside their own clock, and a card that shows `11:49` while
the person reads `12:49` says that the plan is an hour behind. A plan file and the
history keep the time with its zone, as `2026-11-14T07:20:11Z`, because those
files travel to another computer; the card converts that time to the local time
of this computer when it draws it.

## Give a todo to a helper

Use a helper when the todo can run beside you. One writer per worktree: a helper
of a worktree is the only agent that writes files of that worktree, and the
coordinator writes the plan file of its own worktree alone. A helper that works
on another machine gets its own clone or its own worktree there.

### The brief of a helper

Write the brief before the helper starts. It holds:

- the todo, in its own words, and the one task it must finish;
- the files and the folders that it owns, and the rule that it changes nothing
  outside them;
- the check that proves the todo, exactly as the plan file holds it;
- the path of the progress note that it writes, and the rule that it writes that
  note only;
- the rule that it never writes `plan.json`, never pushes and never opens a pull
  request.

The brief is a file, and it is not a card: a helper has no `block_panel` of the
plan. Name the plan folder in the brief so a helper that starts again finds the
plan file.

### The three ways to start a helper

1. **The sub-agent tool of your own agent program.** The coordinator hands the
   todo to a sub-agent of its own program, in its own panel. This way needs no
   panel, no machine and no key. The sub-agent writes the progress note and
   returns its result, and the coordinator reads the note after the call returns.
   The coordinator is inside one tool call while the sub-agent runs, so the card
   of the plan cannot change during it.
2. **A second agent panel on this computer.** Use `open_panel` with a preset of
   the person and its own worktree. Read the preset first with
   `{"operation": "presets"}`: it answers the name and the kind of every preset
   that `open_panel` accepts. Use a preset whose kind is the kind that runs you,
   because a helper of your own kind reads the same brief and the same rules.
   Another kind only when the section `## Horizon plan` of the repository names
   it, for example `helpers: codex`:

   ```json
   {"operation": "open_panel", "preset": "<the helper preset of the settings>",
    "directory": "/home/person/repo-wt/fix-login-redirect-t4", "title": "helper t4",
    "prompt": "Read .horizon-plan/BRIEF.md and do todo t4 only."}
   ```

   `prompt` is one line of at most 512 characters that Horizon types into the
   panel one time, when the program of the panel is ready. It works for a preset
   of an agent kind only. Write the long brief to `.horizon-plan/BRIEF.md` of the
   helper's worktree, and keep the line short: it names the brief and the todo.
   Ask the person for that preset one time when `presets` answers none of that
   kind: the message of `unknown_preset` lists the names of the presets.

   - An agent cannot read, write or close a panel that it opened. The person
     closes the panel of the helper.
   - One agent panel opens at most eight panels in ten minutes. A `rate_limited`
     answer names the seconds to wait.
   - When the preset starts no agent program, `prompt` is refused: the line
     would go into a shell or a command that reads no sentence.
3. **A helper on a machine of the machine list.** The coordinator starts the
agent
   program on the machine itself, over its own SSH login, in a session that
   survives the connection. Read the machine key and the SSH form in
   `horizon-block` and use it for every command of your own:

   ```sh
   ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
       -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
       -o BatchMode=yes -o ConnectTimeout=10 <user>@<address> '<command>'
   ```

   Make the clone and the worktree of the helper on the machine, copy the brief
   with `scp -p` over the same options, and start the agent program in a `tmux`
   session so the work survives the connection:

   ```sh
   'cd <clone> && tmux new -d -s helper-t4 "<the agent program> <the brief argument>"'
   ```

   Take the command of the agent program from `helpers` of the plan file, and ask
   the person for it one time when the plan holds none. Show the session to the
   person with `open_terminal` and `"remote_command": "tmux new -A -s helper-t4"`,
   so the person reads the helper while it works. That panel belongs to the person
   after it opens, and you cannot read it, write to it or close it.

### Read the progress of a helper

Each todo has one progress note beside the plan file of the helper's worktree.
The helper appends one short line at each step: what it did, what passed, what
it waits for. Read it:

- way 1: after the call of the sub-agent returns;
- way 2: read `.horizon-plan/progress-<todo>.md` of the helper's worktree;
- way 3: read the same file over the same SSH login, with `cat`.

When a note asks a question that belongs to the person, write it into `question`
of the plan file, set the todo to `needs_you`, and draw it. When a note reports
the check, run that check yourself or read its result, write the evidence, and
set the todo to `done` or `failed`. Read the note on each state change of the
plan, and at no other time.

## Stop and continue

### The person presses Stop

1. Read the press with `next_click`. A press waits in the queue of the card for
   five minutes when you are inside a tool call, so a Stop does not interrupt a
   command that already runs.
2. Set the state of the plan to `stopped`, and the state of each todo that runs
   to `waiting`.
3. Stop what you started: a session of `tmux` that you started on a machine
stops
   with `tmux kill-session -t <session>` in the same SSH form. A sub-agent of your
   own program stops with the tool of that program when it has one.
4. A panel of a helper is the panel of the person: you cannot close it, and the
   person closes it.
5. Draw the card of the stopped plan, as the card of `Disturb the person only
   when it is theirs` shows.

### A plan file of an older run

When you find a plan file that is not yours, read it before you write anything.
When its state is `running`, the run may still go on somewhere else. Draw it and
ask the person: `Continue` takes the plan on from the state of its file, and
`Start new` writes a new plan file beside the old one, in the same folder, and
leaves the old file as it is.

```json
{"components": [
  {"type": "heading", "text": "A plan of an older run is here",
   "subtitle": "Fix the login redirect · 3 of 8 done · stopped 2 hours ago",
   "badge": {"text": "Stopped", "tone": "warn"}},
  {"type": "text", "text": "The plan file is .horizon-plan/plan.json of this worktree. Three todos are done, one needs you and four wait. Continue runs the four and asks the question once more."},
  {"type": "buttons", "items": [
    {"id": "continue", "label": "Continue", "tone": "primary"},
    {"id": "start_new", "label": "Start new"},
    {"id": "stop", "label": "Stop", "tone": "danger"}]}
]}
```

Never delete the plan file, the history or a progress note of an older run. Name
the folder of the old file on the card when you start a new plan. The old card
belongs to the coordinator of that run: when that panel is gone, `close` it, and
the answer names the panel that closed it with `by_owner: false`.

## A task that is a GitHub issue

1. Read the issue with `gh issue view <number> --repo <owner>/<repository>`, and
   write its repository and number into `issue` of the plan file.
2. Name the issue on the card, in the `subtitle` of the heading. 3. Write to
GitHub only after a press of a button that says what it writes: `Post
   this plan on the issue`, `Push the branch`, `Open a pull request`. Never write
   a comment, a label, a branch or a pull request on the strength of a sentence
   in the panel.

## Limits of this version

- The card belongs to your panel. When the person closes that panel, the card
  stays with the rows disabled and the badges prefixed `Last seen: `, and no agent
  changes it. The next coordinator draws a new card from the plan file, and the
  person closes the old one.
- The person does not see the output of a helper live. The person sees the card,
  the progress note of each todo, and the panel of a helper on this computer or a
  session of a machine through `open_terminal`. Between two notes, the card is
  older than the work.
- A card opens no file: a picture or a recording of the evidence is a path that
  the person opens, and the card draws no image.
- An agent cannot close a panel that it opened, cannot read a terminal that it
  opened, and cannot send a prompt to a panel that it opened.
- `open_panel` types its one `prompt` line and no other text. A helper learns the
  rest of its todo from the brief file.
- One coordinator writes `plan.json`. A helper that writes it breaks the plan.
- The card is plain text that Horizon draws. It is not the state of the work: the
  plan file and the history are.
