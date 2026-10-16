# Upkeep

Upkeep is one agent on a cloud worker that keeps dependency pull requests moving.
The agent reviews the pull requests that bots such as Dependabot open. It fixes what
an update broke. It squash-merges a pull request when every gate passes for its exact
head. This page describes the parts that exist now.

## The Upkeep board

The `upkeep` panel kind is the board of one Upkeep. The board shows these items:

- The agent, the schedule, the worker and the number of repositories.
- **Next run**: the time to the next planned run. The card shows **Running** while a
  run continues. This includes a run that a change of a pull request started between
  the planned runs. The card shows **Paused** when the schedule stops. While the
  daily maintenance pass continues, the line under **Running** starts with **Daily
  maintenance**.
  - **Late**: the planned run is more than 10 minutes overdue.
  - **No report**: a run still shows as running after two schedule periods, and
    after at least three hours, or five hours for the daily maintenance pass. The
    worker can be stopped.
- **Last 24 hours**: one bar for each hour, and the total of merged pull requests.
  The bar of an hour includes the runs that changes started.
  A red bar shows a failed run. An outline shows a skipped run. A green part shows
  merges. A cyan mark under a bar shows an hour with the daily maintenance pass.
- The pull requests in groups by state: **Blocked**, **Fixing**, **Needs review**,
  **Checks running**, **Ready to merge**, **Other** (a state that this Horizon does
  not know) and **Merged**. The **Merged, 7 days** chip shows the pull requests that
  merged in the last seven days. The other chips show no merged rows.
- Chips that filter the list: **Open**, **Needs you** (Blocked and Needs review),
  **Fixing**, **Checks running**, **Ready to merge**, **Merged, 7 days**, and
  **Other** only when a row has an unknown state.
- **Latest run**: the last lines that the agents of the repositories wrote about
  their work.
- **Daily maintenance**: how the last daily maintenance pass ended, and the last
  lines that its agent wrote. The section shows only when a pass started or its
  agent wrote lines. See
  [The daily maintenance pass](#the-daily-maintenance-pass).

A click on a row opens its pull request page in the browser panel **Pull request**
next to the board. The board uses the same panel for the next row. A browser panel
that the board did not open is never used, also when it has the same name. After a
click, the view fits the workspace, so the board and the page are both in view. In a
workspace with a layout preset, the preset places the new panel.

The board of an Upkeep cloud has three buttons in its header. A board that shows
another status document, for example one that a config file opens, has no buttons.

- **Run now** starts a run with the next exchange, which starts at once. It is
  unavailable while a run continues and while the Upkeep is paused. After a click,
  it stays unavailable on every board of that Upkeep until the status shows a run
  that is newer than the newest run at the click, for at most 2 minutes.
- **Pause** stops the planned runs with the next exchange. A run in progress
  finishes. **Resume** starts the planned runs again.
- **Repositories** opens the repository picker of New Upkeep with the list of the
  Upkeep. **Save** adds and removes only the repositories that the person changed,
  on the list that is saved at that time. Thus a change that another part of
  Horizon made in the meantime stays. An Upkeep keeps at least one repository. The
  next run uses the new list. A saved repository that the app does not list now
  stays chosen, also after **Clear**, and the editor shows how many there are.

The board reads the status document that the panel `command` names.
When the command is empty, it reads `~/.horizon/upkeep/status.json`. A config file
can open a board:

```yaml
workspaces:
- name: Upkeep
  terminals:
  - name: Upkeep
    kind: upkeep
    command: /path/to/status.json
```

## New Upkeep

The toolbar **Menu** shows **New Upkeep** under **CLOUD**. The row is available
when cloud workspaces are ready. It opens the **New Upkeep** dialog:

- **Repositories**: one tab for each owner, a filter, and one row for each
  repository. Each row shows the open pull requests of the selected bots. By
  default, the list shows only the repositories with open pull requests.
- **Bots**: Dependabot and Renovate. A change of bots counts the pull requests
  again.
- **Schedule**: every hour, every 4 hours or once a day.
- **Checkouts**: how the worker gets the code. **Only when needed** is the default.
- **Instructions**: the text that each run starts from. **Reset to default** puts
  back the default text (see [The default instructions](#the-default-instructions)).
- **Summary**: the number of repositories, the open pull requests, and a warning
  when Codex has no login to give to the worker.

Horizon gets the repositories from GitHub with the GitHub sign-in of Cloud
settings. The list has only the repositories that the GitHub App can reach. The
pull request counts come from a GitHub search, on a separate thread. When the
search fails, the list shows the repositories without counts, a yellow line with the
cause, and **Try again**.

**Start Upkeep** does these steps:

1. It uses the checkout of the first repository in `<owner>/<name>` of the folder
   for clones: `~/github` or another code folder in the home folder when it exists,
   else `~/Horizon`. When the checkout is not there, it clones the repository with
   the GitHub sign-in. The dialog shows **Starting Upkeep...** until the clone is
   complete. The clone of a large repository can take minutes. **Cancel** closes
   the dialog, but the start continues. When the start fails, the dialog shows
   again with the cause.
2. It saves the choice in `~/.horizon/upkeep/<cloud>/upkeep.json`, and the marker
   `first-run` next to it. The marker holds the token of the first **Run now**.
3. It makes the workspace **Upkeep** with the cloud card and the board.
4. It starts a quick-start cloud on Hetzner. The cloud has no idle stop, because
   the loop must run while this computer sleeps. The first exchange with the
   worker sends **Run now**, so the first run starts when the loop is installed.
   The exchange that delivered **Run now** removes the `first-run` marker; Horizon
   never removes it before that. Thus a restart of Horizon before
   that exchange does not lose the first run. When Horizon cannot remove the
   marker, each later exchange tries again. A restart sends the same token, so it
   does not start a second run.

**Start Upkeep** is unavailable until Codex has a login to give to the worker: the
ChatGPT sign-in or an OpenAI API key in Cloud settings. Deployment uses a bound API key
file before the sign-in and refuses one that it cannot read, so such a file blocks
**Start Upkeep** also when a sign-in exists. It is also unavailable while
the dialog searches for repositories and after the search for the repositories
failed, so a choice is always checked against the latest list of GitHub. When only
the count of the open pull requests failed, the list is new and **Start Upkeep**
stays available.

Upkeep needs a saved session, a Hetzner project in Cloud settings and a GitHub
sign-in. The dialog shows the cause when a step fails, and keeps the choice.

## The default instructions

The default instructions ask an agent for the checks that a change needs and no
more. Each repository's `AGENTS.md` and the procedures it links decide which checks
apply; Horizon has no list of packages or checks of its own. For each pull request
the agent does these steps:

1. It reads the repository's `AGENTS.md` and the procedures it links.
2. It reads the manifest and the resolved dependency diff, transitive changes
   included.
3. It checks CI, the required review, open feedback and mergeability for the
   current head.
4. It does an extra acceptance check only when the repository's rules make it
   apply to this change and CI has not already done that verification.
5. It fixes failures that the update caused, does again the checks whose inputs
   changed, and squash-merges with `--match-head-commit`.
6. It checks the automatic workflows that the merge starts, and cleans up.

These rules follow from the steps:

- A green update that the rules do not treat as a UI change merges once review and
  mergeability pass. The agent does not clone, rebuild, run CI locally, start the
  application or run a UI smoke only because a dependency changed.
- A change that the rules treat as a UI change, also when it arrives through a
  transitive dependency, gets the repository's documented UI acceptance. A label
  such as "patch" never exempts it. Compilation and unit tests never replace a
  visual check that the rules require.
- When the worker cannot do a required check, the pull request stays blocked. The
  agent names the missing capability, for example `needs UI acceptance: no
  desktop`. The agent never waives the check.
- Evidence stays valid while its inputs are unchanged: the head, the base and the
  checked build. A fix or a new base makes the evidence for the changed inputs
  stale, and the agent does those checks again.

An Upkeep that was started with earlier default instructions keeps them; it does
not get these steps by itself.

## Horizon and the worker

While Horizon runs, it does one exchange with the worker of each ready Upkeep
cloud every 30 seconds, and every 8 seconds while a run continues. It reads the
saved Upkeeps in `~/.horizon/upkeep` first; when that read fails, it tries again
after 30 seconds and does not keep an incomplete list. After three failed reads in a
row, it leaves out a saved choice that it cannot read, so that choice does not stop
the other Upkeeps. A saved choice that is missing, refused or not valid JSON is
always left out. Each exchange
runs on a separate thread. It uses the saved choice of that moment and holds the
lock of that choice until it ends, so a change of the repositories waits for an
exchange of the same Upkeep under way and the next exchange carries it. The lock
is also a file lock, `settings.lock` next to the saved choice, so this is also true
for two Horizon windows that use the same home. Other
Upkeeps do not wait. An exchange never sends a
list, or allows a request, for a repository that a save removed. The exchange does
these steps:

1. As root, it sets the Upkeep policy of the worker's GitHub access, as the
   section [What the agent can do on GitHub](#what-the-agent-can-do-on-github)
   describes. When the worker refuses it, Horizon logs the cause; the loop then
   starts no agent, and a **Run now** waits for an exchange that sets it. A paused
   worker drops that **Run now**, as it drops any.
2. It reads the GitHub access requests on the worker. It allows the requests of
   the loop, as the section
   [Trust boundary of the loop's GitHub access](#trust-boundary-of-the-loops-github-access)
   describes. The cloud card shows all other requests to the person. It never
   shows a request that Horizon allows without the person.
3. It does the exchange of the section [The worker loop](#the-worker-loop).
4. It writes the status document of the board to
   `~/.horizon/upkeep/<cloud>/status.json`. When this write fails, Horizon logs
   the cause and writes the document again with the next exchange. It does not
   send **Run now** or **Pause** again, because the worker has them.

After the loop runs and survived its start, Horizon opens the panel
**Upkeep agent**. This Codex panel shows the tmux session of the loop.

## Trust boundary of the status document

The agent on the worker writes the status document. Horizon copies it to this
computer. The board treats the document as untrusted input.

The board guarantees these items:

- It reads the document on a separate thread, not on the UI thread. It looks for a
  change every two seconds.
- It reads at most 4 MiB and refuses a larger document.
- It reads every entry of a list but keeps at most twice the limit while it reads.
  Then it keeps the most important entries, whatever their order in the document.
- It keeps at most these entries:
  - 500 pull requests. Open pull requests come before merged pull requests.
  - The newest 1,500 runs.
  - The newest 100 activity lines.
  - 1,000 repository names.
- It ignores runs and activity lines that are more than one day in the future.
- It cuts each text to 300 characters.
- It parses the document into a fixed schema. It never runs a part of the document.
  - It ignores unknown fields.
  - A field of the wrong type gets its default value.
  - It leaves out a list entry that it cannot parse, for example a pull request
    without a repository or a number.
  - It shows an unknown state as **Other** and an unknown check state as
    **No checks**.
  - It refuses a time that is out of range in UTC.
  - It refuses a whole document that is nested more than 128 levels deep, the
    limit of the JSON parser.
- It puts runs and activity lines in time order before it keeps the newest. Thus
  the document can list them in any order.
- Each bar shows all runs that started in its hour, up to now. The merged total
  counts the same runs. A run that starts later than now does not count yet.
- A row opens a page only when its URL is exactly
  `https://github.com/<owner>/<name>/pull/<number>`. The owner, name and number
  must be those of the row, and the repository must be a plain `owner/name`.
  Other URLs open nothing.
- When a newer document is not valid or the file is gone, the board keeps the last
  valid status. A yellow line tells the cause. The board reads the file every two
  seconds and compares its content, so it sees every change, also one that keeps the
  time and length of the file.

The board does not guarantee that the status is true. The agent writes it, and an
agent can make errors. Check a merge on GitHub before you act on it.

## The worker loop

Horizon keeps the Upkeep loop on the cloud worker with an exchange. An exchange is
one SSH command. Horizon compresses the settings, the instructions and the scripts
in it, so that the longest settings fit the command line of the worker. The code is in `crates/horizon-core/src/upkeep/worker.rs` and
`crates/horizon-core/src/upkeep/worker/`. The section
[Horizon and the worker](#horizon-and-the-worker) tells when Horizon starts an
exchange. The exchange does these steps:

1. It writes the loop (`run.sh` and `run.py`), the settings without the
   instructions (`config.json`), the agent instructions (`prompt.md`) and the
   instructions of the daily maintenance pass (`daily.md`) to
   `/workspace/home/upkeep`.
2. It writes the session binding in `/workspace/sessions/upkeep-<cloud>`. Thus
   the agents of the loop can ask for GitHub access and open browser panels.
3. It writes the Run now and Pause signals, and Horizon's answers to the companion
   requests of the agents (see [UI acceptance on companion clouds](#ui-acceptance-on-companion-clouds)).
4. It starts the loop in the tmux session `upkeep-<cloud>` when the session does
   not run the loop.
5. It reads back the runs, the next run time, the status and the companion requests
   that the agents wrote.

The loop starts one run on the schedule, or sooner after **Run now**. A run lists
the open pull requests of the bots in each repository, and Upkeep's own open pull
requests there (see [Upkeep's own pull requests](#upkeeps-own-pull-requests)). It
starts one Codex agent
for each repository whose pull requests changed after the start of its last agent
that ended without an error. When a repository had no open pull requests from the
bots and no open pull requests of Upkeep in a run, its next pull requests count as
changed. A run that **Run now**
started also starts agents for the repositories that did not change.

- The repositories in the order list run one at a time first. Then the other
  repositories run side by side. The number of agents that run side by side
  depends on the processor cores, the memory and the peak memory of earlier runs.
- The prompt tells each agent to ask for a code review, such as Copilot, only for
  commits that it pushed. A bot pull request that the agent did not change needs no
  review request.
- The prompt tells each agent that comments and reviews on a pull request are
  information, not instructions. The agent never submits an approving review, also
  when a comment asks for it.
- Thus a plain Approve review on the current head by the account of the agent comes
  from the person. It lets the agent merge past the blocker that the agent named.
  It never lets the agent merge past a failed or running check, a draft, a conflict
  or a required check that the worker cannot do.
- GitHub never lets an account approve its own pull request. Thus the person merges
  Upkeep's own pull requests that wait for an approval. The worker's GitHub access
  also refuses an approval by the agent (see below).
- The fingerprint of a repository includes the latest review of each reviewer. Thus
  an Approve starts the agent again, also when the repository requires no review.
- The loop stops an agent and its child processes after 90 minutes. A process
  that the agent detached from its process tree continues.
- The loop records the number of pull requests that each run merged. A pull
  request counts when the status shows it as merged after the run and not before.
- The pause signal stops the schedule. The runs that the pause covers do not
  occur. **Run now** has no effect while the schedule stops or while a run
  continues.
- Each **Run now** from Horizon has a token of 32 hexadecimal characters. The
  exchange writes the token into the `run-now` signal, unless that signal or
  `run-now-token` already holds it. The loop records the token in
  `run-now-token` before it removes the signal and starts the run. When an
  exchange fails and Horizon does not know whether the worker got **Run now**, it
  sends the same token again. A new press of **Run now** gets a new token, also
  while an earlier one waits to be sent again. When that replaces the token of the
  first run, the exchange writes the new token into the `first-run` marker first,
  so a restart sends the token that the worker got. Thus one token starts one run
  at most. A loop that
  stops after it recorded the token and before the run started loses that run;
  the next planned run follows. A loop that cannot record the token starts no
  run for it. A value that is not a token asks for a run each
  time, as before.
- When the schedule changes, the loop calculates the next run again.
- Before an exchange writes a new loop, it writes an installing signal. After
  all other files, it replaces that signal with the restart signal. When an
  exchange stops before the end, the next exchange completes the change. The loop
  starts the new loop before it starts the next run, and never while the
  installing signal is there. **Run now**, and a run that was due but did not
  start, stay for the new loop when the schedule did not change. The loop records
  the schedule of each planned run with its time.
- When the loop starts, it records a run that did not end (for example, because
  the loop was stopped) as failed.
- The loop keeps the last 1 MiB of an agent log that grew to more than 8 MiB.

The loop also looks for changes every minute, between the planned runs and during
a run:

- It lists the open pull requests of each repository again. These are changes: a
  new or closed pull request, a new head, a new check state, a new review decision
  or a new mergeability. A rebase gives a new head. The person's Approve
  can give a new review decision. When the pull requests of a repository changed
  after the start of its last agent, the loop starts the agent of that repository
  at once.
- Between the planned runs, such an agent starts in a run of its own, with the kind
  `change` in `runs.jsonl`. During a run, it joins that run, until the next planned
  run would start.
- A repository whose agent runs or waits in a run is not started again. The loop
  looks at its changes after that agent ended.
- A repository whose agent failed starts again when its pull requests change, or in
  the next planned run. This is also true after an update of the loop: the loop
  keeps what each agent started with in `started.json`.
- An agent that pushes or merges changes its pull requests, so the next look after
  it ended usually starts it once more. That agent finds the new state, for example
  checks that run on its push.
- The planned runs stay. They find what the checks every minute missed, and they
  start an agent again after it failed. The daily maintenance pass still follows
  the first planned run of a UTC day. The loop does not look for changes during the
  daily maintenance pass.
- A run of a change does not take **Run now** or the planned run. When either is
  due when the run of the change ends, it starts next, also after an update of the
  loop. Later planned times that passed during the run of the change are recorded
  as skipped.
- Each look makes one GitHub search for each bot, one for the branch prefix
  `upkeep/` and one for the label `upkeep`. Each search covers at most 20
  repositories. A look sees the same as the listing of a run.
- One search costs about 3 of the 5,000 points an hour that GitHub gives the
  worker's account. A look every minute at 16 repositories with one bot uses about
  540 points an hour.
- A run lists at most 200 open pull requests of one bot in a repository. When a
  look finds more, it leaves that repository to the planned run.
- The worker's GitHub access refuses a whole search that names one repository
  without a grant. The next looks search each repository of that search on its
  own, and the others go back into one search once their own search works. The
  repository without a grant gets its agent in the planned run, which asks for
  access.
- When GitHub answers with a rate limit or a secondary rate limit, the loop stops
  that look. It waits 2 minutes before the next look. After each further limit, it
  waits twice as long, up to 30 minutes. Only a look or a run that reads the pull
  requests of a repository ends these longer waits. A run that meets a rate limit lists no more
  repositories and starts no agent for the repositories that it did not list. The
  next look starts them when their pull requests changed, and after **Run now**
  also when they did not.
- While the schedule stops, the loop does not look for changes, and a run in
  progress starts no more agents for changes. After **Resume**, the next look
  starts the agents of the repositories that changed meanwhile.
- When a look finds no open pull request of the bots in a repository any more, the
  board drops its open pull requests at once.
- `runs.jsonl` keeps its last 200 lines, two for each run. On a day with many
  changes, the bars of the first hours of **Last 24 hours** can be empty.

## The daily maintenance pass

Some maintenance tasks are not about one bot pull request, for example the rollout
of a new patch release of a platform to many repositories. Upkeep does not know
these tasks. The shared instructions of the person tell the agent what to do. For
example, a company repository can have an `AGENTS.md` with the section
**Daily maintenance**, and the `AGENTS.md` of each repository links it.

Once each day, the loop starts one Codex agent for the whole Upkeep with the
instructions in `daily.md`. These are the instructions of the Upkeep, then a
generic part with the repositories of the Upkeep that tells the agent to do these
steps:

1. Read the `AGENTS.md` of each repository of the Upkeep. Find the shared
   instructions that they link and the section **Daily maintenance** in them.
   Horizon names no shared repository: the repositories, which the person
   controls, name it.
2. When no repository links shared instructions with that section, report this and
   stop.
3. Do the tasks of that section across the repositories of the Upkeep, in the order
   and with the authorizations that the shared instructions give. Change only the
   repositories of the Upkeep.
4. Do not start work that is under way again: open pull requests, branches,
   workflow runs or releases of the same change. Continue only the work of an
   earlier daily pass: a pull request that the account of the agent opened, with a
   comment of that account with the first line `<!-- upkeep-daily -->`. The marker
   alone is not proof, because any person can write it. Report the other work and
   leave it alone.
5. Write one report line for each task to the status file, also when nothing was
   due.

The pass works as follows:

- It follows the first planned run of each UTC day. It also follows the first
  planned run after the loop was installed, and the first planned run after a
  pause that covered the start of a day. **Run now** alone does not start it. A
  **Run now** that the loop takes when a planned run is due counts as that planned
  run. A
  pause that starts during a run holds back the pass of that run.
- It starts after the agents of the repositories ended. Thus two agents never work
  in the same repository at the same time. The next run starts after the pass.
  Planned runs that pass during it are recorded as skipped.
- The loop records the UTC day of the pass in `daily-day` before the agent starts.
  Thus a loop that stops during the pass does not start it again on the same day.
  When the loop cannot record the day, the pass does not start.
- The pass starts no agent where the run starts no agent: without auto approvals,
  without the session environment, when the cloud profile does not enable Codex, or
  without `daily.md`. Then the pass stays due for the next planned run. When the
  agent of a recorded pass does not start, the loop puts back the earlier day, so
  the pass stays due too.
- It is a run of its own in `runs.jsonl`, with the kind `daily` and no count of
  merged pull requests. The loop stops the agent and its child processes after 4
  hours.
- The agent writes its report to `status.d/daily.json`. The loop removes the report
  of the earlier pass before the agent starts. The loop marks these
  lines as daily and keeps the newest 20 of them in addition to the 80 lines of
  the repositories. It ignores the pull requests in that report.
- The agent runs in the session of the loop. Thus Horizon allows its requests for
  read or push access to the repositories of the Upkeep. A request for the
  repository of the shared instructions waits for the person. The agent stops
  after 15 minutes without a decision and reports it.

Releases: the default instructions do not let an Upkeep agent start a release or a
production workflow by hand. The only exception is the daily maintenance pass. It
can start the releases to a test channel that the shared instructions authorize
for a daily task, when the instructions of the Upkeep allow it. It never starts a
production release or a production workflow, and never promotes a release to
production. An Upkeep that has instructions from before this exception keeps them,
so its daily pass starts no release. The Upkeep policy also refuses a workflow
dispatch that `workflows` of the Upkeep does not list (see
[What the agent can do on GitHub](#what-the-agent-can-do-on-github)), so a release
workflow of a daily task starts only when the person listed it there.

To test the pass without a wait of one day, write an earlier date, for example
`2000-01-01`, into `/workspace/home/upkeep/daily-day` as the agent account. The pass
then follows the next planned run. There is no setting for this.

## Trust boundary of the worker loop

The worker loop runs as the agent account. The agent can change all files in
`/workspace/home/upkeep`, the loop included. Thus the exchange treats each file in
that folder as untrusted input.

Untrusted input:

- The files in `/workspace/home/upkeep`, `/workspace/sessions/upkeep-<cloud>` and
  `/workspace/session-data/upkeep-<cloud>`. The agent can replace a file with a
  link, a pipe or a folder.
- The status, the runs and the next run time that the exchange reads back.
- The content of the pull requests on GitHub: titles, bodies, comments, code,
  check logs and pages that the agent opens.

Trusted input:

- The settings of the person: the repositories, the bots, the schedule, the
  worker name and the instructions. Horizon examines them before it sends them.

The exchange and the loop guarantee these items:

- The exchange runs as the account that owns `/workspace/home`. On a worker that
  isolates agents, Horizon starts the exchange through
  `horizon-worker-tailnet agent`, as the agent account. On other workers, the
  exchange and the loop run as root, as the agent panels do there. The loop then
  starts no agent, because the approval policy for root is always "ask". The
  exchange stops with an error when it runs as a different account from the
  owner of `/workspace/home`. Thus root never writes into a folder that the agent
  account controls.
- The exchange opens each folder without links and makes sure that the folder
  belongs to its account. It does not use a folder that is a link.
- The exchange writes each file to a new temporary file with a random name. Then
  it moves the temporary file into place. When the agent replaced a file with a
  link, the exchange replaces the link. It never writes through the link.
- The exchange stops with an error when a folder has the name of a file that it
  writes.
- The exchange reads only regular files. It does not read a link, a pipe, a
  device or a folder. It reads at most 1 MiB of status and the last 256 KiB of
  runs. It sends back no status when the answer is more than 3 MiB.
- From each run, the exchange sends back only an integer start time, an integer
  end time and a short outcome text. It sends back the next run time only when it
  is an integer in the range of Horizon. Thus a run file of the agent cannot make
  the answer invalid.
- Horizon accepts these settings only:
  - 1 to 50 repositories. Each repository is a GitHub `owner/name` that is
    listed one time.
  - An order list that has only repositories of the list.
  - 1 to 20 bots. Each bot is a GitHub login, `name[bot]` or `app/name`.
  - The Codex agent.
  - A schedule from every 15 minutes to every 24 hours.
  - Instructions of at most 8 KiB and a worker name of at most 200 bytes,
    without control characters. The instructions can have line breaks and tabs.
  - A cloud ID of letters, digits, `-` and `_`, and a revision of 40 or 64
    lowercase hexadecimal characters.
- The loop merges the reports of the agents into one status. It keeps only list
  entries that are valid JSON objects of at most 2 KiB and 32 levels. It keeps at
  most 300 pull requests and the newest 80 activity lines. Open pull requests come
  first, one from each repository in turn, then the merged pull requests. Thus the
  status stays below the 1 MiB that the exchange reads, and Horizon can parse it.
- When a repository has no open pull requests from the bots, the loop removes
  its open pull requests from the status. It keeps the merged pull requests.
- The live panel of the loop shows lines from the logs of the agents. The loop
  removes control characters from these lines. Thus text from a pull request
  cannot send a control sequence to the terminal panel.
- The loop examines the repositories and the bots in `config.json` again. It
  leaves out the names that Horizon does not accept. Thus no name can start an
  option of `gh`, or change a file name or the agent instructions.
- The loop reads `run-now` and `run-now-token` only as regular files, without
  following a link and without waiting on a pipe, and at most 64 bytes of each.
  It writes `run-now-token` through a new temporary file with a random name.
- The loop gives the instructions to Codex on the standard input. Thus
  instructions that start with `-` are not options.
- The loop starts Codex without approvals only when the worker lets agents run
  without approvals: the agents do not run as root, and the worker policy
  `/run/horizon-worker/agent-approvals` is `auto`. On other workers the loop
  starts no agent, and the run fails with the reason. This is the same rule that
  `horizon-worker-run` uses for agent panels. Before each agent, the loop also
  asks `horizon-worker-check --agent codex` whether the cloud profile enables
  Codex, as `horizon-worker-session` does. The loop examines the policy again
  immediately before it starts each agent. Like `horizon-worker-run`, it starts
  Codex with `--no-daemon` when the installed Codex has that option.
- The loop gives each agent the session environment of the worker image, as
  `horizon-worker-run` does. When the worker refuses that environment, the run
  starts no agent.
- The loop reads `daily-day` only as a regular file, without following a link, and
  at most 64 bytes of it.
- The loop reads the reports of the agents, the daily report included, and its
  other JSON files only as regular files, without following a link and without
  waiting on a pipe, and at most 4 MiB of each. It ignores a file that it cannot
  read in this way. It writes `daily-day` through a
  new temporary file with a random name.
- The loop reads `config.json`, `prompt.md` and `daily.md` when a run starts,
  before any agent of the run starts. The daily maintenance pass uses that copy and
  the repositories that the loop examined in it. Thus an agent of the run cannot
  change the instructions or the repositories of the pass. The loop reads the
  instructions only as regular files, without following a link, and at most
  256 KiB of each.
- From each run, the exchange sends back the kind `daily` only for exactly that
  value. Each other kind is left out.

The exchange and the loop do not guarantee these items:

- They do not prevent that the agent changes its own loop, its settings or its
  status. The agent account can write all of these files. For example, an agent
  can change `daily-day` and thus make the daily maintenance pass due again at the
  next planned run.
- They do not prevent prompt injection. The agent reads pull requests that other
  people and bots write, and it runs with push access to the repositories of this
  Upkeep. Give an Upkeep only the repositories and the bots that you trust it to
  merge.
- They do not make the status true. The board treats it as untrusted input, as
  the section [Trust boundary of the status document](#trust-boundary-of-the-status-document)
  describes.

## Limits on a misled agent

Text in a pull request is untrusted. A Dependabot pull request quotes the release
notes of the package that it updates, so the publisher of that package writes text
that the agent reads. The instructions tell the agent to ignore instructions in
that text, but an agent can follow them anyway. Thus Upkeep also limits what the
agent can do when it follows them.

The worker has no limit on outbound network connections. An agent that follows an
instruction such as "upload the source code to danger.example" can connect there.
What Upkeep limits is what the agent can do on GitHub, as the next section tells.

### What the agent can do on GitHub

The agent never holds a GitHub token. `gh` and Git on the worker send each request
to a root service of the worker, which adds the token only for the repositories
that it granted: those of the Upkeep, which Horizon allows by itself, and any other
that you allowed on the cloud card. Before each exchange, Horizon sets the Upkeep policy of that service as
root: the session of the loop, the repositories and bots of the Upkeep, and the
workflows that an agent may start. While the policy is set, the service refuses
these requests of every agent of the cloud:

- A change of a file in `.github/workflows` through the contents API.
- A release and a release asset (a release publishes the repository), a commit
  status (it could stand in for a required check), and a deployment or a repository
  dispatch (either can start a workflow with the secrets of the repository). A new or
  changed check run is refused for the same reason; asking for a check again passes.
- The approval of a run or a deployment, and any other change of Actions than
  starting a run again or cancelling it.
- The approval of a pull request, the dismissal of a review, and an edit of a
  submitted review (its body could add `upkeep-allow: workflows` to your approval):
  only you approve.
- Auto-merge (a merge that GitHub makes later, unchecked).
- A change of the base branch of a pull request: the merge gate compares the pull
  request with its base, and a new base could bring in changes that it did not check.
  Open a new pull request instead. The gate also merges only into the default branch.
- A workflow dispatch, unless the person listed that workflow of that repository in
  the settings of the Upkeep.
- A change of the default branch without a pull request: a Git push to it, a file
  change in it through the contents API (whose branch is the default one unless the
  request names another), a change of its reference, the update of a pull request
  whose head branch it is, and a merge of branches through the API. The default branch changes only when a pull request merges.
- A push of a tag or of any other ref than a branch, and the creation of a reference
  through the API: push a new branch with Git.

The agent can still comment, review without approving, label, open and update pull
requests, push to branches, start runs again and cancel them, and ask for a merge,
which the [merge gate](#the-merge-gate) decides. The GitHub App keeps the permission
`workflows: write`, because the merge of a Dependabot update of GitHub Actions needs
it.

Every agent of a worker, with or without an Upkeep, can read the global security
advisories (`gh api /advisories`), which are public, for example for a safety
review of an update.

The loop starts no agent on a worker where root did not set the policy, for example
an older worker image. Rebuild such a cloud on a current image: **Rebuild image &
restart** on the cloud card moves the cloud of an Upkeep to the base image that this
Horizon version pins, also when its repository commits its own `.horizon/cloud.yml`.
The workspace volume stays, so the loop, its files and the policy copy stay.

The refusals go to `/workspace/.horizon-gate/refused.jsonl`, a log that only root
can change, at most 30 lines a minute: a refusal beyond that is refused but not
logged. The exchange reads back the last 20 lines, and the board shows them under
**Latest run** with the activity lines of the agent. The agent writes those lines,
so it can push the refusals out of view on the board; the log keeps them until it
reaches 256 KiB, when it starts again empty.

The workflows that an agent may start are in `workflows` of
`~/.horizon/upkeep/<cloud>/upkeep.json`, at most 20, each in a repository of the
Upkeep. The dialog does not show them yet:

```json
"workflows": [{"repository": "acme/api", "workflow": "release-test.yml"}]
```

A listed workflow runs with the secrets of its repository, and the agent chooses
its inputs. It starts only on the default branch, so it runs the workflow file of
that branch, not a version that an agent pushed to another branch. List only workflows that you let the agent start. The shared
instructions of the repositories decide when the agent starts one.

The policy does not stop these:

- A push to another branch than the default one, also one that changes
  `.github/workflows`. GitHub runs the workflows of a pushed branch for its `push`
  events, with the secrets of the repository.
- A change of code that a workflow runs, for example a build script or a local
  action in `.github/actions`. Only `.github/workflows` is protected.

### The merge gate

The root service, not the agent, decides the last step of a merge. A merge of a pull
request through the API or GraphQL (`gh pr merge`) reaches GitHub only when GitHub
shows all of these. Nothing lifts them:

- A GraphQL merge is the only change of its request.
- The merge names the head commit (`gh pr merge --match-head-commit <head>`), and
  that commit is the head of the pull request now. GitHub then merges only that head.
- The pull request is open, not a draft, and on the default branch of the repository.
  The agent can write every other branch, and GitHub moves a pull request to another
  base when its base branch is deleted, also after the gate read it.
- GitHub reports the pull request ready to merge (`clean`): the required checks and
  reviews passed, and the branch is up to date where a rule requires that.
- Every check run on the head completed as success, neutral or skipped, and the
  commit statuses all passed.

These blockers also stop a merge, until you approve that exact head:

- `author`: neither a listed bot nor your account on its own `upkeep/` branch in
  the same repository opened the pull request.
- `no-checks`: the head has no check run and no commit status.
- `workflows`: a file in `.github/workflows` changes, and not every commit of the
  pull request is the work of a listed bot (the bot wrote it, and GitHub or the bot
  committed it with a signature that GitHub verified, as for Dependabot). The gate
  compares the base commit with the head commit, so a push during the check changes
  nothing; a comparison that GitHub cuts short keeps the blocker. The agent
  can push to a bot's branch, and `author` sees only who opened the pull request.

An **Approve** review of your account on the head lifts `author` and `no-checks`,
while it is your latest Approve or Request changes review of the pull request. It
lifts `workflows` only when its body has the line `upkeep-allow: workflows`: a
workflow change is the riskiest thing to merge unread. The agent works as your
account too, but the policy refuses each approval that it tries, so an approval is
yours. GitHub does not let an account approve its own pull request; for such a pull
request the refusal asks you to merge it yourself.

Otherwise the merge does not reach GitHub, and the refusal names the blockers and
says "Approve to merge", or "Approve with the line upkeep-allow: workflows". The
refusal shows on the board.

## Trust boundary of the shared instructions

The daily maintenance pass does the tasks of the shared instructions that the
repositories of the Upkeep link.

Trusted input:

- The `AGENTS.md` of the repositories of the Upkeep. The person controls these
  repositories and gives the Upkeep push access to them.
- The shared instructions that they link. The person controls that repository and
  allows the read access of the agent on the cloud card.

Untrusted input:

- The report lines of the daily pass. The board shows them like the other activity
  lines, with the same limits.
- The content of pull requests, issues, release pages and other pages that the
  agent reads during a task.

Horizon guarantees these items:

- Horizon names no shared repository. It does not allow a read of a repository
  outside the Upkeep without the person.
- The generic part of the instructions never lets the agent start a production
  release or a production workflow, or promote a release to production.
- The pass changes the repositories of the Upkeep only after their agents ended,
  so no two agents of the Upkeep work in the same repository.

Horizon does not guarantee these items:

- It does not examine the shared instructions. A task can do all that the
  repositories of the Upkeep allow the agent to do.
- It does not prevent that an `AGENTS.md` links shared instructions of another
  person. The person sees the repository in the read request on the cloud card.
- It does not prevent work in a repository by an agent or a person outside the
  Upkeep at the same time. Step 4 of the pass makes the agent report such work.

## Upkeep's own pull requests

Upkeep agents open pull requests too. The daily maintenance pass opens them for a
rollout. The agents of the repositories open them for a change that a gate needs,
for example a preview for a bot pull request. Each of these pull requests has a branch name that starts with
`upkeep/`, and the label `upkeep` when the repository allows labels. The prompts and
the default instructions tell the agents to do this.

- The listing of a run and each look also find Upkeep's own open pull requests in
  each repository of the Upkeep. These have a branch that starts with `upkeep/` or
  the label `upkeep`. A change to one of them, such as a new commit or a check
  result, counts as a change for the skip rule.
- The agent of the repository applies the same gates as to the bots' pull requests.
  It fixes checks and rebases only when necessary. It keeps the order and the
  release steps of the shared instructions, and merges with `--match-head-commit`.
  It merges one only when the instructions of the Upkeep ask
  it to merge Upkeep's own pull requests, as the default instructions do. Thus an
  Upkeep with instructions from before this change carries them up to **Ready to
  merge** and does not merge them. A pull request that changes CI or a deployment
  waits for the person. The generic part of the prompt says this too. Thus changed
  instructions cannot remove it.
- The agent reports them with the author `Upkeep`. The board shows them as
  ordinary rows.
- The daily maintenance pass reports a task across repositories in one line, for
  example `<task>: <repository> merged and deployed, 8 to go`.
- The agents of the repositories merge their pull requests side by side, and never
  during the daily pass. The shared instructions can give an order across
  repositories. Then the daily pass keeps that order itself. It opens the pull
  request of a repository only after the one before it merged.

### Trust boundary of the mark

The agent sets the mark, and any person who can push a branch or set a label can
set it too. Thus:

- Only pull requests in the repositories of the Upkeep count. The loop never lists
  pull requests of other repositories. It ignores a marked pull request from a fork,
  because Upkeep pushes its branches to the repository itself. Thus only an account
  that can push to the repository or set its labels can mark a pull request.
- The agents apply the same gates to a marked pull request as to a bot pull request.
- An agent merges a marked pull request only when the GitHub account of the agent
  opened it, unless the shared instructions authorize its author. Otherwise it does
  not check out, build or run the code of the pull request, does not push to it, and
  reports it as blocked.
- The daily pass continues a pull request only when its own account opened it and
  wrote its `<!-- upkeep-daily -->` comment.
- The author `Upkeep` on the board comes from the status that the agent writes. It
  is not proof of the author. Check the author on GitHub.

## Trust boundary of the loop's GitHub access

The agents of the loop ask for GitHub access with the `github_access` tool, as
other agents on a worker do. The worker records each request with the session of
the agent. Horizon reads the requests on the worker. A request is untrusted input:
an agent on the worker can write its reason, and an agent in another session of
the cloud can ask too.

Horizon allows a request without the person only when all of these are true:

- The request comes from the session `upkeep-<cloud>` of the loop.
- The request asks for read or push access.
- The repository is in the list of the Upkeep, with no difference other than
  the case of letters.
- The worker can allow the request for that session only (`task` is true).

Horizon then allows the request for that session only (**Allow for this task**).
The cloud card shows every other request, and the person decides it.

Horizon guarantees these items:

- It never allows admin access, and never access for the whole cloud.
- It never allows a repository that is not in the list.

A worker image with `horizon-worker-github-upkeep-loop` applies these rules
itself, also while no Horizon runs. It uses the session and the repositories of
the Upkeep policy that Horizon sets as root before each exchange (see
[What the agent can do on GitHub](#what-the-agent-can-do-on-github)). Only root
can read and write the policy, so the agent account cannot change it. The worker
guarantees these items:

- It allows only for the task, never admin access, and never for the whole cloud.
- It examines the policy again immediately before it allows a request, so it never
  allows a repository that the current policy does not list. A policy that it
  cannot read allows nothing.
- A new policy ends the loop's task grants for each repository that it does not
  list. This includes access that the person gave the loop on the cloud card for
  another repository: the person allows it again when the loop asks.
- After a container restart, it starts the loop again as the agent account, never
  as root.
- Disconnecting GitHub (`horizon-worker-github clear`) removes the policy. Until
  Horizon sets it again, the strictest policy holds.

Until the next exchange succeeds, the worker allows the loop's requests for the
list that it has, also for a repository that the person removed in Horizon.

Horizon and the worker do not guarantee these items:

- They do not isolate the sessions of the cloud. All agent sessions of a worker
  run as the same account, so an agent in another session can act in the session
  of the loop and use its access. Use the Upkeep cloud only for the Upkeep.
- An agent with push access can push to the branches of the repository. The
  repository rules on GitHub, such as branch protection, still apply.

## Trust boundary of New Upkeep

The repository names come from the GitHub API through the sign-in of the person.
Horizon accepts only plain `owner/name` names and refuses the whole answer when a
name is not valid. The counts of open pull requests can be too low: GitHub search
gives at most 1,000 results. They only put the list in order.

The settings that **Start Upkeep** saves pass the checks of the worker loop
(`Config::validate`) before Horizon saves them or sends them to a worker.

## UI acceptance on companion clouds

Some repositories require a check of their user interface for some updates, for
example a youpark update of Avalonia. The repository's AGENTS.md tells when the check
applies and how to do it. The Upkeep worker cannot do such a check itself, because it
has no desktop, GPU or image of the repository. The agent does the check on a
[companion cloud](companion-controller.md) of the repository. The code is in
`crates/horizon-core/src/cloud_runtime/companions/inventory.rs`,
`crates/horizon-core/src/upkeep/worker/companions.rs` and
`crates/horizon-ui/src/app/cloud_panel/production/companions/agent/upkeep.rs`.

- An Upkeep cloud declares one companion for each of its repositories whose checkout
  on this computer (`<owner>/<name>` in the code folder for clones, as **New Upkeep**
  clones it) has a
  committed profile with the name `ui-acceptance` in `.horizon/cloud.yml`. The alias
  is the repository name in lower case, with a hyphen in place of each character other
  than a letter or a digit, for example `youpark`. Other repositories declare nothing.
- The owner makes the companion cloud once: **New cloud** from the repository's
  checkout with the `ui-acceptance` profile, in the workspace of the Upkeep. The owner
  starts it once, stops it, and checks it on the card of the Upkeep cloud. Thus the
  owner decides which repository can start a cloud, and on which provider.
- The agent asks for the companion with the file
  `/workspace/home/upkeep/companions/<alias>.json`, for example
  `{"action": "ensure_ready", "operation_id": "<new UUID>"}`. The action is
  `ensure_ready` or `stop`. The exchange sends at most four requests to Horizon, and
  removes a request when Horizon has answered it as done.
- Horizon does the request as it does the request of an agent panel (see the tool
  `cloud_companion_ensure_ready`): the operation runs on the card of the companion
  cloud. Horizon writes the answer of each alias to
  `/workspace/home/upkeep/companions-status.json` with the next exchange: the
  operation ID, the phase, whether it is done, and a message or an error.
- The agent then uses the companion with `horizon-cloud-worker companions inspect`,
  SSH and Git: it fetches the pull request, does the procedure of the repository on
  the desktop of the companion, which the person watches on its card, and copies the
  evidence back. Then it asks for `stop`, also when the check failed.
- When an exchange shows that no run is active, or the Upkeep cloud has not been ready
  for five minutes, Horizon stops each companion that a run of this Upkeep started. A
  failed exchange stops nothing. When the Ensure Ready that started the companion has not
  finished, Horizon first sends it again so that the companion's card finishes it. When
  the Stop fails or the companion refuses it, Horizon sends a new one ten minutes later,
  up to six times. Horizon keeps this record only while it runs.
- The agent puts the screenshots and logs of the check on the orphan branch
  `upkeep/evidence` of the repository, under `pr-<number>/<short head>/`, and links
  them from its one comment on the pull request. Nobody merges this branch. The branch
  holds only screenshots and logs from UI acceptance, never secrets. When a pull
  request closes, the agent removes its folder.
- When the companion is not declared or not checked, or when the owner's Horizon does
  not run, the agent leaves the pull request blocked and gives the reason. A companion
  needs the owner's Horizon: the worker cannot start a cloud by itself.

## Trust boundary of companion requests

Untrusted input:

- Each companion request and file in `/workspace/home/upkeep/companions`. The agent
  account writes them, and an agent can follow instructions from a pull request.
- The text that the agent puts on the `upkeep/evidence` branch and in its comment.

Trusted input:

- The saved settings of the Upkeep, the checkouts on this computer and the `ui-acceptance`
  profiles that they commit.
- The owner's checkbox on the card and the cloud records of Horizon.

Horizon guarantees these items:

- A request can only start or stop a companion of the Upkeep's own cloud that the owner
  checked. The source of each request is that cloud; the request has no field for it.
  Horizon examines the alias, the action and the operation ID again before it acts.
- The exchange reads only regular files with a valid alias and at most 1 KiB, opened
  without following links, and sends at most four requests. It removes a request that
  Horizon answered as done, unless the agent changed it after the exchange read it.
- A request outside a run starts nothing, and the loop removes the requests of earlier
  runs when a run or the daily maintenance pass starts. A run or pass that cannot remove
  them does not start. A request with an operation ID that Horizon answered as done
  is not sent again, and Horizon sends one request for each alias at a time.
- The answers that Horizon sends back have a limited set of fields and a message of at
  most 120 characters without control characters, for at most four aliases.

Horizon does not guarantee these items:

- It does not limit what the agent does on the companion. The companion grants trusted
  shell access, as [the companion controller](companion-controller.md) describes.
- It does not stop a companion that a run started when Horizon restarted during that
  run, when the Upkeep cloud's worker stops answering while it shows as ready, when the
  Stops fail six times in a row (for example after the repository's `ui-acceptance`
  profile is gone), or when the Upkeep is removed. The agent's own `stop` request still stops it.
- It does not know when a run really ends: the agent account writes the run records
  that the exchange reads. An agent that writes a record of a run in progress keeps its
  requests allowed and delays the stop at the end of the run. It still can start only a
  companion that the owner checked.
- It does not examine the files on the `upkeep/evidence` branch.

## Agent requests

An agent in a Horizon agent panel can read and change the Upkeep of its workspace
with the MCP tool `cloud_upkeep`. The code is in
`crates/horizon-browser-control/src/manifest/upkeep.rs`,
`crates/horizon-browser-mcp/src/controller/upkeep.rs` and
`crates/horizon-ui/src/app/upkeep/agent.rs`. The operations are these:

- `status`: the repositories, the bots, the schedule, the pause state, the recent
  runs and the pull requests.
- `discover`: the repositories that the GitHub app reaches, with the number of open
  pull requests from the bots.
- `create`: Horizon shows **New Upkeep** in the workspace of the agent, with the
  settings of the agent filled in and a note that an agent proposed them for that
  workspace, with its name.
- `add`: Horizon shows the repository list of the Upkeep, with the repositories of
  the agent added.
- `remove`: Horizon removes repositories from the Upkeep.
- `run_now`, `pause` and `resume`: Horizon sends the **Run now** or **Pause**
  signal with its next exchange with the worker, as the board does. The answer is
  `requested`.

## Trust boundary of agent requests

An agent can follow instructions from a page, a pull request or a file that it read.
Thus Horizon treats each `cloud_upkeep` request as untrusted input.

Untrusted input:

- Each field of the request: the operation, the repositories, the bots, the schedule,
  the checkouts and the instructions.
- The identity of the agent panel that sent the request.

Trusted input:

- The decisions of the person in **New Upkeep** and in the repository list.
- The saved settings of each Upkeep and the cloud records of Horizon.

Horizon guarantees these items:

- Only an agent panel on this computer can send a request. Horizon answers a request
  only when its host is this Horizon and its actor is an agent panel on the board. A
  panel of a cloud, such as the **Upkeep agent** panel, shows an agent on the worker:
  Horizon refuses a request with its identity.
- A request acts only on the Upkeep whose cloud is in the workspace of the agent
  panel. A request has no field for a workspace or a cloud.
- Before a request changes something, Horizon examines again that the agent panel is
  in the workspace that it asked from and that the cloud of the Upkeep is in that
  workspace. Thus a session switch or a closed panel stops the request. A `remove`
  save that started before the change finishes: it can wait some seconds for the lock
  of the settings while an exchange with the worker runs.
- An agent on a cloud worker cannot use `cloud_upkeep`. The worker refuses the
  request. Thus the Upkeep agent, which reads pull requests that other people write,
  cannot change its own repositories, its schedule or its pause state.
- `create` and `add` do not change an Upkeep. They show the person the dialog with the
  proposal of the agent. Only the person starts an Upkeep, rents a worker, or adds a
  repository, and thus gives the Upkeep agent push access to it. The answer is
  `awaiting_person`.
- The repository list shows each repository of an `add` proposal in its own row with a
  tick, also when the list of the GitHub App does not hold it. Thus the person sees
  every repository that **Save** adds.
- Both dialogs name the workspace of the proposal, because they can open while the
  person looks at another workspace. **Start Upkeep** of a proposal refuses when that
  workspace is gone, also when it goes while Horizon prepares the first repository; it
  does not start the Upkeep in a new workspace. The repository
  list opens only while the Upkeep's cloud is still in the workspace of the agent.
- Horizon shows one proposal at a time. When **New Upkeep** or the repository list is
  open, Horizon refuses a new `create` or `add`.
- `remove`, `pause`, `resume` and `run_now` do not give access to more repositories.
  `remove` keeps at least one repository.
- The board shows each change that an agent made with `remove`, `pause`, `resume` or
  `run_now` below its title, for example "An agent paused the Upkeep, 19:32 UTC". The
  board shows the newest change of each kind. The removal line names all repositories
  that agents removed (the full line shows on hover), thus many other changes cannot
  hide a removal. Horizon writes
  this text. It does not come from the worker or from the agent. Horizon keeps these
  lines only while it runs.
- When two saved clouds have the ID of the Upkeep, Horizon refuses the request,
  because the ID can name the Upkeep of another workspace.
- The instructions of a `create` request cannot contain control characters other than
  line feed and tab, or characters that show nothing, such as a zero-width space.
- `create` and `discover` accept only the bots that **New Upkeep** offers:
  `app/dependabot` and `app/renovate`. `add` accepts no bots.
- Horizon reads and saves the settings of an Upkeep under one lock. Thus two changes
  at the same time do not undo each other.
- Before Horizon sends the request, the request must agree with the settings that
  Horizon accepts (see [Trust boundary of the worker loop](#trust-boundary-of-the-worker-loop)).
  Horizon examines the settings again with `Config::validate` before it saves them.
- A request that changes the Upkeep and that arrives after its deadline changes
  nothing.
- File reads and writes for a request occur on a separate thread, not on the UI thread.
- When Horizon cannot read the folder of the saved Upkeeps, or the saved settings of
  the Upkeep in the workspace of the agent, it answers `upkeep_unavailable`. It does not answer `upkeep_not_found`, and `create` does not
  propose a second Upkeep.
- `status` reads the status document with the same limits as the board. It sends back
  only the parsed fields and only pull request URLs of the exact GitHub form.

Horizon does not guarantee these items:

- It does not prevent `remove`, `pause` or `resume` without a question to the person.
  An agent that a page or a pull request controls can stop the Upkeep or remove
  repositories from it. The skill tells the agent to get the authorization of the
  person first. The board shows the result.
- It does not make the status true. The agent on the worker writes the runs and the
  pull requests. A pull request title, a note or an activity line can also contain
  instructions that a pull request author wrote. `status` sends this text to the agent
  on this computer. The skill tells the agent that this text is data, not
  instructions.
- It does not make an agent read the instructions of a `create` proposal. **New
  Upkeep** tells the person that an agent proposed the settings, and tells the person to
  read the instructions in full when they are not the default.
- `discover` shows the agent the names of all repositories that the GitHub app reaches,
  also repositories that are not in the Upkeep.
