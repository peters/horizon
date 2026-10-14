# Upkeep

Upkeep is one agent on a cloud worker that keeps dependency pull requests moving.
The agent reviews the pull requests that bots such as Dependabot open. It fixes what
an update broke. It squash-merges a pull request when every gate passes for its exact
head. This page describes the parts that exist now.

## The Upkeep board

The `upkeep` panel kind is the board of one Upkeep. The board shows these items:

- The agent, the schedule, the worker and the number of repositories.
- **Next run**: the time to the next run. The card shows **Running** while a run
  continues and **Paused** when the schedule stops.
  - **Late**: the planned run is more than 10 minutes overdue.
  - **No report**: a run still shows as running after two schedule periods, and
    after at least three hours. The worker can be stopped.
- **Last 24 hours**: one bar for each hour, and the total of merged pull requests.
  A red bar shows a failed run. An outline shows a skipped run. A green part shows
  merges.
- The pull requests in groups by state: **Blocked**, **Fixing**, **Needs review**,
  **Checks running**, **Ready to merge**, **Other** (a state that this Horizon does
  not know) and **Merged**. The **Merged, 7 days** chip shows the pull requests that
  merged in the last seven days. The other chips show no merged rows.
- Chips that filter the list: **Open**, **Needs you** (Blocked and Needs review),
  **Fixing**, **Checks running**, **Ready to merge**, **Merged, 7 days**, and
  **Other** only when a row has an unknown state.
- **Latest run**: the last lines that the agent wrote about its work.

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
  back the default text.
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

1. It reads the GitHub access requests on the worker. It allows the requests of
   the loop, as the section
   [Trust boundary of the loop's GitHub access](#trust-boundary-of-the-loops-github-access)
   describes. The cloud card shows all other requests to the person. It never
   shows a request that Horizon allows without the person.
2. It does the exchange of the section [The worker loop](#the-worker-loop).
3. It writes the status document of the board to
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
one SSH command. The code is in `crates/horizon-core/src/upkeep/worker.rs` and
`crates/horizon-core/src/upkeep/worker/`. The section
[Horizon and the worker](#horizon-and-the-worker) tells when Horizon starts an
exchange. The exchange does these steps:

1. It writes the loop (`run.sh` and `run.py`), the settings without the
   instructions (`config.json`) and the agent instructions (`prompt.md`) to
   `/workspace/home/upkeep`.
2. It writes the session binding in `/workspace/sessions/upkeep-<cloud>`. Thus
   the agents of the loop can ask for GitHub access and open browser panels.
3. It writes the Run now and Pause signals.
4. It starts the loop in the tmux session `upkeep-<cloud>` when the session does
   not run the loop.
5. It reads back the runs, the next run time and the status that the agents wrote.

The loop starts one run on the schedule, or sooner after **Run now**. A run lists
the open pull requests of the bots in each repository. It starts one Codex agent
for each repository whose pull requests changed after the start of its last agent
that ended without an error. When a repository had no open pull requests from the
bots in a run, its next pull requests count as changed. A run that **Run now**
started also starts agents for the repositories that did not change.

- The repositories in the order list run one at a time first. Then the other
  repositories run side by side. The number of agents that run side by side
  depends on the processor cores, the memory and the peak memory of earlier runs.
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

The exchange and the loop do not guarantee these items:

- They do not prevent that the agent changes its own loop, its settings or its
  status. The agent account can write all of these files.
- They do not prevent prompt injection. The agent reads pull requests that other
  people and bots write, and it runs with push access to the repositories of this
  Upkeep. Give an Upkeep only the repositories and the bots that you trust it to
  merge.
- They do not make the status true. The board treats it as untrusted input, as
  the section [Trust boundary of the status document](#trust-boundary-of-the-status-document)
  describes.

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

## Native qualification on a test cloud

Some pull requests need a native qualification: the repository's `AGENTS.md`
requires a person to watch the application in a Horizon native VNC Device panel,
for example an Avalonia update. The Upkeep worker cannot do this. It runs the
quick-start image, which does not have the toolchain of the repository. An Upkeep
agent can thus ask Horizon for a test cloud.

A test cloud is a short-lived cloud from the repository's own `.horizon` profile, on
the head commit of the pull request, in the workspace of the Upkeep. The person can
watch it as a local test: the agent panel of the test cloud and a Device panel on
its desktop show next to the board.

1. The agent writes a request file in `/workspace/home/upkeep/native/requests/`, a
   JSON object with `repository`, `pull_request`, `head` (the full head commit) and
   `reason`. The prompt of each run tells the agent how. The next exchange reads the
   newest 8 request files of at most 4 KiB each.
2. Horizon examines the request (see
   [Trust boundary of the test cloud](#trust-boundary-of-the-test-cloud)). Horizon
   starts one test cloud at a time for each Upkeep. At most 8 other requests wait,
   in their order. Horizon tests each head of a pull request one time. When a test
   ended without `passed` or `failed` (refused, blocked or expired), the same
   request starts a new test after one hour, at most two tests for each head.
   Horizon remembers the last 256 tested heads; a refused request, which rented no
   cloud, is the first to be forgotten.
3. Horizon gets the head commit into the checkout of the repository on this
   computer, as **Start Upkeep** does for its first repository. The network fetch
   runs in a staging repository with HTTPS only, so the Git configuration of the
   checkout never applies to the request that carries the GitHub sign-in. It starts the test
   cloud **Test NAME #NUMBER** on Hetzner with the default profile of the repository
   at that commit.
4. When the cloud is ready, Horizon opens the panel **Native test agent** (Codex, or
   Claude when the profile has no Codex) and the desktop viewer. When the agent
   panel has been open for 15 seconds and shows no new output, Horizon pastes the
   task into it: read `AGENTS.md`, run its native qualification on the worker
   desktop `:99`, and write the evidence to `/workspace/home/native-test`. The
   image must start the agent past its first-run dialogs; for Claude Code, the
   worker tools of Horizon do this since #1230. In an older image the task goes
   into a dialog, and the test ends at the time limit as expired. The task counts
   as given when the Enter after it reaches the panel; when a session switch or a
   restart comes first, Horizon gives it again.
5. Every 30 seconds, Horizon reads `result.json` in that folder. When the agent wrote
   it, or when the time limit of 120 minutes from the start ends, Horizon copies the
   evidence to `~/.horizon/upkeep/<cloud>/native/` on this computer and closes the
   test cloud, which deletes its server and volume.
6. Horizon writes a report of each test to `/workspace/home/upkeep/native/results/`
   on the Upkeep worker, one file for each pull request, also while the test waits
   or runs: the state (`queued`, `starting`, `running`, `passed`, `failed`,
   `blocked`, `refused` or `expired`), a message, and for the newest two reports
   the report of the agent and the names of the evidence files.

Horizon keeps the tests of each Upkeep in `~/.horizon/upkeep/<cloud>/native.json`.
After a restart, it continues the test that ran, or ends it at its time limit.

### Trust boundary of the test cloud

Untrusted input:

- The request. An agent on the Upkeep worker writes it, and the agent reads text
  from pull requests.
- The code of the pull request. The test cloud builds and runs it.
- The result, the report and the evidence that the agent on the test cloud writes.

Horizon starts a test cloud only when all of these are true:

- The repository is in the list of the Upkeep.
- GitHub, read with the GitHub sign-in of this computer, shows the pull request as
  open, with the head commit of the request. The head branch is in the same
  repository, not in a fork. One of the bots of the Upkeep opened it, and its
  author is the bot of a GitHub App (a login that ends in `[bot]`): an account in the
  list of authors that is a person or a machine user gets no test cloud.
- The default profile of the repository has a desktop, no GPU, and Codex or Claude
  without approvals.
- The build of the profile uses only files in `.horizon`, and the pull request does
  not change a file in `.horizon` since its branch left the base branch. Thus the
  image that Docker builds on this computer comes from files of the base branch.
  The code of the pull request runs only on the test cloud.

Horizon guarantees these items:

- The test cloud gets no GitHub access and no Git or BrowserStack credential from
  Cloud settings. A marker in the folder of the cloud (`isolated`) keeps them away
  for every deployment of it, also a resume or a rebuild. The cloud gets the source
  as a pack from this computer. It gets no credential of the Upkeep. Horizon checks
  no companion of the profile for the test cloud. Only the person can check one, on
  the card of the cloud, so no agent can start one.
- Horizon closes the test cloud when the qualification ends, and at the latest at
  the time limit; the last copy of the evidence before the close takes at most 3
  minutes. When the cloud did not start, or the person closed it, the test ends as
  blocked. When the deletion fails, the close dialog asks the person, as for any
  cloud. Horizon starts the cloud only after the ledger on disk names it, keeps it
  there until its card is gone, and after a restart closes it once more. While a
  test cloud that Horizon closes still has its card, also after a failed close,
  no other test cloud of the Upkeep starts.
  Horizon follows a test while the session that holds the Upkeep's cloud or the
  test cloud is open. When the person closes the Upkeep's cloud while a test runs,
  the test still ends in that run of Horizon; after a restart, the person closes
  the test cloud on its card.
- Horizon reads the result folder only as the owner of `/workspace/home`, without
  following a link, and only regular files. It keeps at most 12 files of known
  kinds (pictures, recordings and text) besides `result.json` and `report.md`,
  each at most 2 MiB and 16 MiB in all. It
  parses `result.json` into an outcome and a summary of at most 300 characters,
  and cuts the report to 4 KiB. It removes control characters from both.
- The request reaches no command line: Horizon uses only the repository name of the
  Upkeep's list, the number and the full commit.

Horizon does not guarantee these items:

- The result is true. An agent writes it, and code of the pull request runs next to
  that agent. Look at the evidence before you rely on a pass.
- The code of the pull request cannot read the login of the agent on the test cloud.
  The same is true for every cloud that runs a pull request.
- The code of the pull request cannot send data out. The test cloud has no limit on
  outbound connections. Thus the code can send the login of the agent anywhere. This
  is why Horizon starts a test cloud only for a pull request that a bot of the Upkeep
  opened in the repository itself and that does not change `.horizon`.
- The person can watch the whole run. The Device panel shows only the desktop
  `:99` of the test cloud.
