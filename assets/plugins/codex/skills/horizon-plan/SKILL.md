---
name: horizon-plan
description: >-
  Plan one task as three to twelve todos on one live Horizon card, then do the todos yourself or give
  them to helper agents on this computer or on a machine of the machine list, and keep the card true
  with the state, the owner, the place and the time of each todo. Use it when the person describes a
  task or names a GitHub issue and wants the work done in several steps, or when the person asks to
  see the plan of a task.
---

# One task on one card

The person describes a task, or names a GitHub issue. You write the plan as three
to twelve todos, you show the plan as one card, and you ask for a go. After the
go you do the todos yourself, or you give them to helper agents on this computer
and on the machines of the machine list. The card stays the one place where the
person reads the task: each todo with its state, the agent that does it, the
place, and the time. A todo that needs an answer of the person is a row with
buttons. The card ends with what is done and what is left.

The card contract is in `horizon-block`: the document of a card, its operations,
`next_click`, `open_terminal`, `open_panel` and the machine records. The
worktrees of a repository are in `horizon-worktree`. This skill adds the plan and
the rules of the work.

## The plan file

The plan file is the one source of the task. You draw the card from it, and a
coordinator that starts again reads it and continues. It is a JSON file of one
fixed shape.

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

Never run `git add` on the plan folder, never commit it, and never write the plan
file of a task into another folder. The plan file is a record of the work, not a
file of the product.

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
  "todos": [
    {"id": "t1",
     "title": "Read the issue and the failing test",
     "who": "coordinator",
     "where": "this computer",
     "state": "done",
     "check": "cargo test -p horizon-ui login_redirect",
     "check_result": "pass",
     "started_at": "2026-11-14T07:10:30Z",
     "ended_at": "2026-11-14T07:14:02Z",
     "progress": "progress-t1.md"}
  ],
  "question": null
}
```

- `plan_version` is `1`.
- `task` is one line of at most 120 characters.
- `issue` is `<owner>/<repository>#<number>`, or `null` when the task is no
  issue.
- `state` is `plan` while the card asks for a go, `running`, `stopped` or `done`.
- `created_at`, `updated_at`, `started_at` and `ended_at` are ISO 8601 times in
  UTC, or `null` when the event did not happen.
- `card_panel_id` is the `panel_id` of the card of this plan, or `null` before
  the card exists.
- `helpers` holds one entry per helper agent that the plan uses, with its name,
  its place and, on a machine, the command of its agent program. `command` is
  absent for a helper on this computer.
- `todos` holds 3 to 12 entries. `id` is `t1` to `t12`. `who` is `coordinator`,
  a name of `helpers`, or `you` for a todo that waits for the person. `where` is
  `this computer` or the name of a machine of `machines`. `state` is `waiting`,
  `now`, `done`, `needs_you` or `failed`. `check` is the one command that proves
  the todo, and it is empty only when the todo is a question to the person.
  `check_result` is `pass`, `fail` or `null`. `progress` is the file name of the
  progress note of the todo, beside the plan file.
- `question` is `null`, or one object with `todo`, `text` and `answers`. Each
  answer has an `id` of `answer.<todo>.<choice>` and a `label`.

Only you write `plan.json`, and one coordinator writes it: a helper never does.
A helper writes only its own progress note. Write the whole file at each change,
and write it before you draw the card that reports the change.

### What survives, and what does not

| Event | `plan.json` and the notes | The card | The helpers |
|---|---|---|---|
| The agent program of the coordinator restarts in its panel | yes, on disk | yes, and the agent of that panel owns it again | A helper on a machine keeps its session; a helper panel is the person's |
| Horizon restarts | yes, on disk | yes: the document of a card comes back with its panel, and the panel keeps its place, so your panel owns the card again | A session of a machine survives the restart; a helper panel starts again from its preset |
| The person's other computer opens the task | only when the plan folder is there: a clone of the same branch, or a copy of the folder | no: a card belongs to one Horizon | no: a session of a machine belongs to the connection of one computer |
| The person closes the panel of the coordinator | yes, on disk | the card stays, its rows and buttons are disabled, and each badge reads `Last seen: ` | A helper on a machine keeps running; the person closes a helper panel |

The clicks that wait and the selected row of a card are not saved. The output of a
helper between two progress notes is not saved either: the files hold the plan,
the notes and the work, and nothing else.

## The card

Make one card at the start, titled with the task, keep the `panel_id` that
`create` answers, and change the card with `update`. The person reads one card
that changes. One agent owns at most eight cards in one workspace.

The parts of the card, in this order:

1. `heading`: `text` is the task. `subtitle` is the issue, when the task has one,
   and the place of the work, for example `Issue #1987 · this computer`. `badge`
   is the state of the plan: `Plan`, `Running`, `Stopped` or `Done`.
2. `progress`: `label` is the summary line, `percent` is the share of the todos
   that are done, and `detail` names the todo that runs now, for example `t4 now`.
3. `caption` with `text` `Todos`, then one `list` with `selection` `none` and one
   row for each todo.
4. The block of a todo that needs the person: a `caption` with `text`
   `Needs you`, one `text` with the question, and one `buttons` with the answers.
5. `caption` `What is done` with one `text`, and `caption` `What is left` with one
   `text`, when the plan is stopped or done.
6. One `buttons` row with `Stop`, tone `danger`, last.

### The row of a todo

The row form is `todo · who · where · time`:

```json
{"id": "t4", "title": "Change the redirect target",
 "subtitle": "helper-2 · build-1", "value": "8 m",
 "badge": {"text": "Now", "tone": "info"}}
```

- `id` is the id of the todo, so a row and a line of the plan file name the
  same todo. The rows of a plan take no press: the person answers with the
  buttons of the block that needs them.
- `title` is the todo.
- `subtitle` is `<who> · <where>`: the coordinator or the helper, and `this
  computer` or the machine name.
- `value` is the time. Write the time that the plan file measured when the todo
  started or ended, such as `38 s`, `8 m` or `1 h 12 m`. Write the estimate with
  `~` in front when the todo waits, such as `~5 m`. Write `waits` for a todo
  that waits for the person, because nothing runs.
- `badge` is the state: `Done` with tone `ok`, `Now` with tone `info`, `Waiting`
  with tone `neutral`, `Needs you` with tone `warn`, `Failed` with tone `danger`.

### The summary line

`progress.label` is `<done> of <all> done · <needs> needs you`, for example
`3 of 8 done · 1 needs you`. Write `1 needs you` and `2 need you`, and `0 needs
you` when no todo waits for the person. `percent` is the rounded share of the
done todos.

### The bounds of a card

A card holds 64 components, 32 rows in one `list`, 16 steps in one `steps`, 8
buttons in one `buttons`, 24 rows in one `kv`, 512 characters in one text field
and 32 KiB of serialized document. A plan of 3 to 12 todos fits in one `list`
with room to spare.

A `steps` component draws `done`, `now` and `waiting` with a short time at the
right edge, and its bound is 16. It holds no owner and no place, so the todos of
a plan go in a `list`. Use one `steps` only for a run whose todos have one owner
and one place, and never for the card of the plan itself.

### The layouts of 3, 8 and 12 todos

The plan, before the go:

```json
{"components": [
  {"type": "heading", "text": "Fix the login redirect",
   "subtitle": "Issue #1987 · this computer", "badge": {"text": "Plan", "tone": "info"}},
  {"type": "progress", "label": "0 of 3 done · 0 needs you", "percent": 0,
   "detail": "no todo runs"},
  {"type": "caption", "text": "Todos"},
  {"type": "list", "selection": "none", "rows": [
    {"id": "t1", "title": "Read the issue and the failing test",
     "subtitle": "coordinator · this computer", "value": "~12 m",
     "badge": {"text": "Waiting"}},
    {"id": "t2", "title": "Change the redirect target",
     "subtitle": "helper-1 · this computer", "value": "~20 m",
     "badge": {"text": "Waiting"}},
    {"id": "t3", "title": "Run the test and the full check",
     "subtitle": "coordinator · this computer", "value": "~15 m",
     "badge": {"text": "Waiting"}}]},
  {"type": "text", "text": "Three todos, one at a time, on this computer. The plan file is in .horizon-plan/ of the worktree, and git ignores it. Nothing outside the worktree changes."},
  {"type": "buttons", "items": [
    {"id": "go", "label": "Go", "tone": "primary"},
    {"id": "change", "label": "Change the plan"},
    {"id": "stop", "label": "Stop", "tone": "danger"}]}
]}
```

Eight todos, three done and one that needs the person:

```json
{"components": [
  {"type": "heading", "text": "Fix the login redirect",
   "subtitle": "Issue #1987 · this computer", "badge": {"text": "Running", "tone": "info"}},
  {"type": "progress", "label": "3 of 8 done · 1 needs you", "percent": 38,
   "detail": "t4 now"},
  {"type": "caption", "text": "Todos"},
  {"type": "list", "selection": "none", "rows": [
    {"id": "t1", "title": "Read the issue and the failing test",
     "subtitle": "coordinator · this computer", "value": "4 m",
     "badge": {"text": "Done", "tone": "ok"}},
    {"id": "t2", "title": "Add the redirect case to the test",
     "subtitle": "coordinator · this computer", "value": "6 m",
     "badge": {"text": "Done", "tone": "ok"}},
    {"id": "t3", "title": "Check the route table",
     "subtitle": "helper-1 · build-1", "value": "9 m",
     "badge": {"text": "Done", "tone": "ok"}},
    {"id": "t4", "title": "Change the redirect target",
     "subtitle": "helper-2 · this computer", "value": "8 m",
     "badge": {"text": "Now", "tone": "info"}},
    {"id": "t5", "title": "Choose the address of the redirect",
     "subtitle": "you · this computer", "value": "waits",
     "badge": {"text": "Needs you", "tone": "warn"}},
    {"id": "t6", "title": "Update the route notes",
     "subtitle": "coordinator · this computer", "value": "~5 m",
     "badge": {"text": "Waiting"}},
    {"id": "t7", "title": "Run the full check",
     "subtitle": "coordinator · build-1", "value": "~20 m",
     "badge": {"text": "Waiting"}},
    {"id": "t8", "title": "Write the report of the task",
     "subtitle": "coordinator · this computer", "value": "~5 m",
     "badge": {"text": "Waiting"}}]},
  {"type": "caption", "text": "Needs you"},
  {"type": "text", "text": "t5: Which address must the redirect use?"},
  {"type": "buttons", "items": [
    {"id": "answer.t5.account", "label": "The account address", "tone": "primary"},
    {"id": "answer.t5.last", "label": "The last address"},
    {"id": "answer.t5.skip", "label": "Skip this todo"}]},
  {"type": "buttons", "items": [{"id": "stop", "label": "Stop", "tone": "danger"}]}
]}
```

Twelve todos, all done:

```json
{"components": [
  {"type": "heading", "text": "Fix the login redirect",
   "subtitle": "Issue #1987 · this computer", "badge": {"text": "Done", "tone": "ok"}},
  {"type": "progress", "label": "12 of 12 done · 0 needs you", "percent": 100,
   "detail": "1 h 12 m"},
  {"type": "caption", "text": "Todos"},
  {"type": "list", "selection": "none", "rows": [
    {"id": "t1", "title": "Read the issue and the failing test",
     "subtitle": "coordinator · this computer", "value": "4 m",
     "badge": {"text": "Done", "tone": "ok"}},
    {"id": "t2", "title": "Add the redirect case to the test",
     "subtitle": "coordinator · this computer", "value": "6 m",
     "badge": {"text": "Done", "tone": "ok"}},
    {"id": "t3", "title": "Check the route table",
     "subtitle": "helper-1 · build-1", "value": "9 m",
     "badge": {"text": "Done", "tone": "ok"}},
    {"id": "t4", "title": "Change the redirect target",
     "subtitle": "helper-2 · this computer", "value": "8 m",
     "badge": {"text": "Done", "tone": "ok"}},
    {"id": "t5", "title": "Choose the address of the redirect",
     "subtitle": "you · this computer", "value": "2 m",
     "badge": {"text": "Done", "tone": "ok"}},
    {"id": "t6", "title": "Update the route notes",
     "subtitle": "coordinator · this computer", "value": "5 m",
     "badge": {"text": "Done", "tone": "ok"}},
    {"id": "t7", "title": "Run the full check",
     "subtitle": "coordinator · build-1", "value": "20 m",
     "badge": {"text": "Done", "tone": "ok"}},
    {"id": "t8", "title": "Write the report of the task",
     "subtitle": "coordinator · this computer", "value": "5 m",
     "badge": {"text": "Done", "tone": "ok"}},
    {"id": "t9", "title": "Read the review of the change",
     "subtitle": "helper-1 · build-1", "value": "3 m",
     "badge": {"text": "Done", "tone": "ok"}},
    {"id": "t10", "title": "Fix the two findings of the review",
     "subtitle": "coordinator · this computer", "value": "6 m",
     "badge": {"text": "Done", "tone": "ok"}},
    {"id": "t11", "title": "Run the full check again",
     "subtitle": "coordinator · build-1", "value": "4 m",
     "badge": {"text": "Done", "tone": "ok"}},
    {"id": "t12", "title": "Write the summary of the task",
     "subtitle": "coordinator · this computer", "value": "1 m",
     "badge": {"text": "Done", "tone": "ok"}}]},
  {"type": "caption", "text": "What is done"},
  {"type": "text", "text": "Done: the redirect target, the new test, the route notes, the review and two rounds of the full check on build-1. Each todo passed the check that the plan names."},
  {"type": "caption", "text": "What is left"},
  {"type": "text", "text": "Left: the branch is not pushed and no pull request exists. Press Push the branch when you want them."},
  {"type": "buttons", "items": [
    {"id": "push", "label": "Push the branch", "tone": "primary"},
    {"id": "close", "label": "Close"}]}
]}
```

### How often you change the card

Draw the card after each change of the state of a todo, and at most one time in
ten seconds. When two changes fall inside ten seconds, keep the later state in
the plan file and draw the card after the ten seconds. Never draw the same
document twice, never draw on a timer, and never redraw a card that did not
change: a card that Horizon draws costs the person a frame, and a loop that
redraws an unchanged card costs the person every frame.

### The answer of the person

A todo with the state `needs_you` holds `question` in the plan file. Draw its
question and its answers as buttons. The answer arrives as `component_id` such as
`answer.t5.account`. Write the answer into the plan file, set the todo to `now`,
and go on.

When two or more todos need the person at the same time, ask the first one on the
card one time, keep the summary line true with the number of all of them, and ask
the next after the answer. A card holds 8 buttons in one `buttons` row, so ask
two questions of up to four answers on two rows when both wait.

## The go

1. Write `plan.json` with the state `plan` and the todos in the order of the
   work.
2. Draw the card of the plan with the buttons `Go`, `Change the plan` and
   `Stop`.
3. Call `next_click` with no `panel_id` and `timeout_millis: 300000`, and leave
   out every other field.
4. On `go`, set the state of the plan to `running` and start the first todo.
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
2. Do the work yourself, or give it to a helper.
3. Run the check of the todo, or read the result of the check that the helper
   ran.
4. Set `check_result`, the state `done` or `failed`, and `ended_at`. Write the
   plan file and draw the card.

A todo is done when its own check passed and you ran that check or read its
result. It is not done because a helper says so.

When a todo fails, set its state to `failed` and write one line in the progress
note with the reason. Draw it on the card. Go on with the todos that do not
depend on it, and say on the card what the failure leaves undone.

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

#### 1. The sub-agent tool of your own agent program

The coordinator hands the todo to a sub-agent of its own program, in its own
panel. This way needs no panel, no machine and no key. The sub-agent writes the
progress note and returns its result, and the coordinator reads the note after
the call returns.

Limit: the coordinator is inside one tool call while the sub-agent runs, so the
card of the plan cannot change during it, and a press of the person waits in the
queue of the card.

#### 2. A second agent panel on this computer

Use `open_panel` with a preset of the person, and give the helper its own
worktree with the `horizon-worktree` skill:

```json
{"operation": "open_panel", "preset": "<the helper preset of the settings>",
 "directory": "/home/person/repo-wt/fix-login-redirect-t4", "title": "helper t4"}
```

`open_panel` copies the command, the arguments and the kind of the preset, and it
sends no prompt. The helper therefore learns its todo from a file: write the
brief at the fixed path `.horizon-plan/BRIEF.md` of the helper's worktree, and
keep one helper preset whose arguments make its agent program read that file at
start. Ask the person for that preset one time when the settings hold none: the
message of `unknown_preset` lists the names of the presets, and you show that
list and ask again.

- The helper reads `.horizon-plan/BRIEF.md` and writes
  `.horizon-plan/progress-<todo>.md` in its own worktree. You read that file on
  this computer, with no connection.
- An agent cannot read, write or close a panel that it opened. The person closes
  the panel of the helper.
- One agent panel opens at most eight panels in ten minutes. A `rate_limited`
  answer names the seconds to wait.
- A panel opened this way waits for a person to type when the preset names no
  brief file. Say that on the card, and use way 1 or way 3 for that todo.

#### 3. A helper on a machine of the machine list

The coordinator starts the agent program on the machine itself, over its own SSH
login, in a session that survives the connection. Read the SSH form and the
machine key in `horizon-block`. Use it for every command of your own:

```sh
ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
    -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
    -o BatchMode=yes -o ConnectTimeout=10 <user>@<address> '<command>'
```

1. Make the clone and the worktree of the helper on the machine, and copy the
   brief to it:

   ```sh
   scp -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
       -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
       -o BatchMode=yes .horizon-plan/BRIEF.md \
       <user>@<address>:<clone>/.horizon-plan/BRIEF.md
   ```

2. Start the agent program in a session of `tmux`, so the work survives the
   connection:

   ```sh
   ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
       -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
       -o BatchMode=yes -o ConnectTimeout=10 <user>@<address> \
       'cd <clone> && tmux new -d -s helper-t4 "<the agent program> <the brief argument>"'
   ```

   Take the command of the agent program from `helpers` of the plan file. Ask
   the person for it one time when the plan holds none.

3. Show the session to the person with `open_terminal`, so the person can read
   the helper while it works:

   ```json
   {"operation": "open_terminal", "key": "<the machine key of that machine>",
    "ssh": {"host": "203.0.113.10", "user": "root"},
    "remote_command": "tmux new -A -s helper-t4", "title": "helper t4"}
   ```

   The panel belongs to the person after it opens. You cannot read it, write to
   it or close it.

### Read the progress of a helper

Each todo has one progress note beside the plan file of the helper's worktree. The
helper appends one short line at each step: what it did, what passed, what it
waits for. Read it:

- way 1: after the call of the sub-agent returns;
- way 2: read `.horizon-plan/progress-<todo>.md` of the helper's worktree;
- way 3: read the same file over the same SSH login:

  ```sh
  ssh -i <private_key_path> -o IdentitiesOnly=yes -o IdentityAgent=none \
      -o UserKnownHostsFile=<known_hosts_path> -o StrictHostKeyChecking=accept-new \
      -o BatchMode=yes -o ConnectTimeout=10 <user>@<address> \
      'cat <clone>/.horizon-plan/progress-t4.md'
  ```

When a note asks a question, write it into `question` of the plan file, set the
todo to `needs_you`, and draw it. When a note reports the check, run that check
yourself or read its result, and set the todo to `done` or `failed`.

Read the note on each state change of the plan, and at no other time: a read on a
timer costs a connection and gives the person nothing.

## Stop and continue

### The person presses Stop

1. Read the press with `next_click`. A press waits in the queue of the card for
   five minutes when you are inside a tool call, so a Stop does not interrupt a
   command that already runs.
2. Set the state of the plan to `stopped`, and the state of each todo that runs
   to `waiting`.
3. Stop what you started: a session of `tmux` that you started on a machine
   stops with `tmux kill-session -t <session>` in the same SSH form. A sub-agent
   of your own program stops with the tool of that program when it has one.
4. A panel of a helper is the panel of the person: you cannot close it, and the
   person closes it.
5. Draw the card of the stopped plan. It says what stopped, what is left
   running, and what waits:

```json
{"components": [
  {"type": "heading", "text": "Fix the login redirect",
   "subtitle": "Issue #1987 · this computer", "badge": {"text": "Stopped", "tone": "warn"}},
  {"type": "progress", "label": "3 of 8 done · 1 needs you", "percent": 38,
   "detail": "stopped"},
  {"type": "text", "text": "Stopped: the helper of t4 on build-1 and the check of t7. Left running: nothing of this plan. The plan file keeps the state of each todo."},
  {"type": "buttons", "items": [
    {"id": "continue", "label": "Continue", "tone": "primary"},
    {"id": "close", "label": "Close"}]}
]}
```

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

Never delete the plan file or a progress note of an older run. Name the folder of
the old file on the card when you start a new plan.

## A task that is a GitHub issue

1. Read the issue with `gh issue view <number> --repo <owner>/<repository>`, and
   write its repository and number into `issue` of the plan file.
2. Name the issue on the card, in the `subtitle` of the heading.
3. Write to GitHub only after a press of a button that says what it writes: `Post
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
- An agent cannot close a panel that it opened, cannot read a terminal that it
  opened, and cannot send a prompt to a panel that it opened.
- `open_panel` sends no prompt. A helper that a panel starts learns its todo from
  the brief file, and only when its preset names that file.
- One coordinator writes `plan.json`. A helper that writes it breaks the plan.
- The card is plain text that Horizon draws. It is not the state of the work: the
  plan file is.
